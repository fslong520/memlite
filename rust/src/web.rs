//! 内置 Web 客户端：`memlite web [--port 8789]`——本地回环单页管理界面。
//!
//! 零框架：std TcpListener 手写 HTTP 子集（GET/POST + Content-Length），
//! 只绑 127.0.0.1（明文记忆库，绝不监听外网）。API：
//!   GET  /api/status      → 库状态（data_dir/条数/文件/嵌入配置）
//!   GET  /api/search?q=   → 检索（复用 CLI 的两档自动回退逻辑）
//!   GET  /api/show?id=    → 单条全文
//!   POST /api/remember    → 存（走敏感闸）
//!   POST /api/forget      → 删
//!   POST /api/sync        → git 同步
//! 写接口要求同源 Origin（防浏览器任意站点 CSRF 打本地端口）。

use crate::config;
use crate::querylog::{Entry, Hit, Log};
use crate::sensitive;
use crate::store::Store;
use crate::sync as syncmod;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

pub fn serve(data_dir: std::path::PathBuf, port: u16) -> Result<(), String> {
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr).map_err(|e| format!("绑定 {addr} 失败: {e}"))?;
    println!("memlite web: http://{addr}/  （数据目录 {}）", data_dir.display());
    // 检索日志常驻：起服务载一次，每次检索就地增量（次数/统计不再读全文件）
    let log = Arc::new(Mutex::new(Log::load(&data_dir)));
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let dir = data_dir.clone();
                let log = log.clone();
                std::thread::spawn(move || {
                    let _ = handle(s, &dir, &log);
                });
            }
            Err(e) => eprintln!("accept 失败: {e}"),
        }
    }
    Ok(())
}

fn handle(
    mut stream: TcpStream,
    data_dir: &std::path::Path,
    log: &Arc<Mutex<Log>>,
) -> Result<(), String> {
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut req_line = String::new();
    reader.read_line(&mut req_line).map_err(|e| e.to_string())?;
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();

    // 头
    let mut content_len = 0usize;
    let mut origin: Option<String> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            break;
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some(v) = l.strip_prefix("Content-Length:") {
            content_len = v.trim().parse().unwrap_or(0);
        }
        if let Some(v) = l.strip_prefix("Origin:") {
            origin = Some(v.trim().to_string());
        }
    }

    // 写接口同源校验（Origin 存在且非本机回环即拒；无 Origin 视为非浏览器，放行）
    let unsafe_origin = origin.as_deref().map(|o| {
        !(o.starts_with("http://127.0.0.1") || o.starts_with("http://localhost"))
    }).unwrap_or(false);
    if unsafe_origin && method == "POST" {
        return resp(&mut stream, 403, r#"{"error":"跨站请求被拒"}"#);
    }

    let body = if content_len > 0 {
        let mut buf = vec![0u8; content_len];
        reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
        String::from_utf8_lossy(&buf).into_owned()
    } else {
        String::new()
    };

    match (method.as_str(), path.split('?').next().unwrap_or(&path)) {
        ("GET", "/") => resp_html(&mut stream, PAGE),
        ("GET", "/api/status") => api_status(data_dir, log, &mut stream),
        ("GET", "/api/search") => {
            let q = url_param(&path, "q").unwrap_or_default();
            let limit: usize = url_param(&path, "limit")
                .and_then(|v| v.parse().ok())
                .unwrap_or(20);
            // 档位：auto（语义优先、失败回词面）| bm25（强制词面）
            let mode = url_param(&path, "mode").unwrap_or_default();
            api_search(data_dir, log, &q, limit, mode == "bm25", &mut stream)
        }
        ("GET", "/api/show") => {
            let id = url_param(&path, "id").unwrap_or_default();
            api_show(data_dir, log, &id, &mut stream)
        }
        ("GET", "/api/list") => {
            let limit: usize = url_param(&path, "limit").and_then(|v| v.parse().ok()).unwrap_or(30);
            let offset: usize = url_param(&path, "offset").and_then(|v| v.parse().ok()).unwrap_or(0);
            api_list(data_dir, limit, offset, &mut stream)
        }
        ("GET", "/api/querylog") => {
            let limit: usize = url_param(&path, "limit").and_then(|v| v.parse().ok()).unwrap_or(50);
            let top: usize = url_param(&path, "top").and_then(|v| v.parse().ok()).unwrap_or(10);
            api_querylog(log, limit, top, &mut stream)
        }
        ("GET", "/api/sync-status") => {
            let v = syncmod::status(data_dir);
            resp(&mut stream, 200, &serde_json::to_string(&v).unwrap())
        }
        ("GET", "/api/export") => api_export(data_dir, &mut stream),
        ("GET", "/api/config") => api_config(&mut stream),
        ("POST", "/api/remember") => api_remember(data_dir, &body, &mut stream),
        ("POST", "/api/update") => api_update(data_dir, &body, &mut stream),
        ("POST", "/api/forget") => api_forget(data_dir, &body, &mut stream),
        ("POST", "/api/sync") => api_sync(data_dir, &mut stream),
        ("POST", "/api/compact") => api_compact(data_dir, &mut stream),
        ("POST", "/api/reembed") => api_reembed(data_dir, &mut stream),
        ("GET", "/api/versions") => {
            let id = url_param(&path, "id").unwrap_or_default();
            api_versions(data_dir, &id, &mut stream)
        }
        ("GET", "/api/tags") => api_tags(data_dir, &mut stream),
        ("GET", "/api/projects") => api_projects(data_dir, &mut stream),
        ("GET", "/api/inject") => api_inject_probe(&mut stream),
        ("POST", "/api/inject") => api_inject_do(&body, &mut stream),
        _ => resp(&mut stream, 404, r#"{"error":"not found"}"#),
    }
}

fn url_param(path: &str, key: &str) -> Option<String> {
    let q = path.split_once('?')?.1;
    for kv in q.split('&') {
        if let Some((k, v)) = kv.split_once('=') {
            if k == key {
                return Some(percent_decode(v));
            }
        }
    }
    None
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => { out.push(b); i += 3; }
                    None => { out.push(bytes[i]); i += 1; }
                }
            }
            b'+' => { out.push(b' '); i += 1; }
            b => { out.push(b); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn resp(stream: &mut TcpStream, code: u16, json: &str) -> Result<(), String> {
    let reason = if code == 200 { "OK" } else if code == 403 { "Forbidden" } else if code == 404 { "Not Found" } else if code == 500 { "Internal Server Error" } else { "Bad Request" };
    let body = json.as_bytes();
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    stream.write_all(body).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())
}

fn resp_html(stream: &mut TcpStream, html: &str) -> Result<(), String> {
    let body = html.as_bytes();
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    stream.write_all(body).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())
}

fn api_status(
    data_dir: &std::path::Path,
    log: &Arc<Mutex<Log>>,
    stream: &mut TcpStream,
) -> Result<(), String> {
    let s = Store::open(data_dir);
    let vidx = crate::vectors::VectorIndex::open(data_dir);
    let ecfg = crate::embed::embedding_config();
    let files: Vec<serde_json::Value> = s
        .file_stats()
        .into_iter()
        .map(|(f, b)| serde_json::json!({"file": f, "bytes": b}))
        .collect();
    let ql = log.lock().map(|l| l.total()).unwrap_or(0);
    let v = serde_json::json!({
        "data_dir": data_dir.to_string_lossy(),
        "project_dir": config::find_project_dir().map(|p| p.to_string_lossy().into_owned()),
        "records": s.count(),
        "vectors": vidx.count(),
        "bad_lines": s.bad_lines,
        "files": files,
        "queries": ql,
        "embedding": {"configured": ecfg.is_some()},
    });
    resp(stream, 200, &serde_json::to_string(&v).unwrap())
}

/// 检索：与 CLI recall 同逻辑（语义优先、项目级空自动回退用户级）；顺手落 query_log
fn api_search(
    data_dir: &std::path::Path,
    log: &Arc<Mutex<Log>>,
    q: &str,
    limit: usize,
    bm25_only: bool,
    stream: &mut TcpStream,
) -> Result<(), String> {
    if q.trim().is_empty() {
        return resp(stream, 400, r#"{"error":"q 为空"}"#);
    }
    let t0 = std::time::Instant::now();
    let (backend, hits) = crate::recall_search(data_dir, q, limit, "", bm25_only);
    let user_dir = config::user_data_dir();
    let (backend, hits, fallbacked) = if hits.is_empty() && data_dir != user_dir {
        let (b2, h2) = crate::recall_search(&user_dir, q, limit, "", bm25_only);
        if !h2.is_empty() { (b2, h2, true) } else { (backend, hits, false) }
    } else {
        (backend, hits, false)
    };
    // 落日志：进程内常驻视图就地增量；文件写失败只回执不拦检索
    let mut log_write_err = String::new();
    if let Ok(mut l) = log.lock() {
        let entry = Entry {
            ts: crate::store::now_ts(),
            query: q.to_string(),
            backend: backend.to_string(),
            limit,
            fallback: if fallbacked { "user" } else { "" }.into(),
            source: "web".into(),
            ms: t0.elapsed().as_millis() as u64,
            hits: hits
                .iter()
                .map(|(sc, r)| Hit { id: r.id.clone(), score: *sc, title: r.title.clone() })
                .collect(),
        };
        if let Err(e) = l.append(data_dir, entry) {
            log_write_err = format!("（检索日志写入失败：{e}）");
        }
    }
    let arr: Vec<serde_json::Value> = hits
        .iter()
        .map(|(sc, r)| {
            serde_json::json!({
                "score": (sc * 10000.0).round() / 10000.0,
                "id": r.id, "title": r.title, "ts": r.ts,
                "content": r.content, "tags": r.tags, "project": r.project
            })
        })
        .collect();
    let v = serde_json::json!({
        "backend": backend,
        "fallback": if fallbacked {"user"} else {""},
        "hits": arr,
        "log_err": log_write_err,
    });
    resp(stream, 200, &serde_json::to_string(&v).unwrap())
}

fn api_show(
    data_dir: &std::path::Path,
    log: &Arc<Mutex<Log>>,
    id: &str,
    stream: &mut TcpStream,
) -> Result<(), String> {
    let s = Store::open(data_dir);
    match s.resolve(id) {
        Some(r) => {
            let count = log.lock().map(|l| l.count_of(&r.id)).unwrap_or(0);
            let mut v = serde_json::to_value(r).unwrap();
            v["recall_count"] = serde_json::json!(count);
            resp(stream, 200, &serde_json::to_string(&v).unwrap())
        }
        None => resp(stream, 404, r#"{"error":"记忆不存在"}"#),
    }
}

/// 检索日志：汇总 + 最近流水（UI「检索日志」页取数）
fn api_querylog(
    log: &Arc<Mutex<Log>>,
    limit: usize,
    top: usize,
    stream: &mut TcpStream,
) -> Result<(), String> {
    let Ok(l) = log.lock() else {
        return resp(stream, 500, r#"{"error":"日志锁失效"}"#);
    };
    let v = serde_json::json!({
        "stats": l.stats(top),
        "recent": l.recent_json(limit),
    });
    resp(stream, 200, &serde_json::to_string(&v).unwrap())
}

/// 列最近（空查询时的默认视图，带分页）：按 ts 降序
fn api_list(data_dir: &std::path::Path, limit: usize, offset: usize, stream: &mut TcpStream) -> Result<(), String> {
    let s = Store::open(data_dir);
    let mut recs = s.all();
    recs.sort_by(|a, b| b.ts.partial_cmp(&a.ts).unwrap_or(std::cmp::Ordering::Equal));
    let total = recs.len();
    let items: Vec<serde_json::Value> = recs
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|r| serde_json::to_value(r).unwrap())
        .collect();
    let v = serde_json::json!({"total": total, "offset": offset, "limit": limit, "items": items});
    resp(stream, 200, &serde_json::to_string(&v).unwrap())
}

/// 编辑（同 id 追加新版本，末写胜出；敏感闸与 CLI 同尺）
fn api_update(data_dir: &std::path::Path, body: &str, stream: &mut TcpStream) -> Result<(), String> {
    let v: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return resp(stream, 400, r#"{"error":"body 不是 JSON"}"#),
    };
    let id = v["id"].as_str().unwrap_or("").trim().to_string();
    if id.is_empty() {
        return resp(stream, 400, r#"{"error":"缺 id"}"#);
    }
    let mut s = Store::open(data_dir);
    let Some(old) = s.resolve(&id).map(|r| r.clone()) else {
        return resp(stream, 404, r#"{"error":"记忆不存在"}"#);
    };
    let new_title = v["title"].as_str().map(|t| t.trim().to_string()).unwrap_or_else(|| old.title.clone());
    let new_content = v["content"].as_str().map(|c| c.trim().to_string()).unwrap_or_else(|| old.content.clone());
    let new_tags: Vec<String> = match v["tags"].as_array() {
        Some(a) => a.iter().filter_map(|t| t.as_str()).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
        None => old.tags.clone(),
    };
    if new_content.is_empty() {
        return resp(stream, 400, r#"{"error":"content 不能为空"}"#);
    }
    let hits = sensitive::scan(&format!("{new_title}\n{new_content}"));
    if !hits.is_empty() {
        let e = serde_json::json!({"error": format!("命中敏感信息（{}）", hits.join("、"))});
        return resp(stream, 400, &e.to_string());
    }
    if let Err(e) = s.put_with_id(&old.id, &new_content, &new_title, &new_tags, &old.project) {
        return resp(stream, 400, &serde_json::json!({"error": e}).to_string());
    }
    let mut embedded = false;
    if let Some(cfg) = crate::embed::embedding_config() {
        if let Ok(vec) = crate::embed::embed(&cfg, &format!("{new_title}\n{new_content}")) {
            embedded = crate::vectors::VectorIndex::open(data_dir).put(&old.id, &vec).is_ok();
        }
    }
    let out = serde_json::json!({"id": old.id, "title": new_title, "embedded": embedded});
    resp(stream, 200, &out.to_string())
}

/// 导出全库明文 JSON（管理用；含 tombstone 前的存活记录）
fn api_export(data_dir: &std::path::Path, stream: &mut TcpStream) -> Result<(), String> {
    let s = Store::open(data_dir);
    let arr: Vec<serde_json::Value> = s.all().iter().map(|r| serde_json::to_value(r).unwrap()).collect();
    let body = serde_json::to_string(&arr).unwrap();
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Disposition: attachment; filename=\"memlite-export.json\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    stream.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())
}

/// 配置查看（api_key 脱敏）
fn api_config(stream: &mut TcpStream) -> Result<(), String> {
    let mut merged = crate::config::embedding_merged();
    if let Some(k) = merged["api_key"].as_str() {
        if !k.is_empty() {
            merged["api_key"] = serde_json::json!("***");
        }
    }
    let v = serde_json::json!({
        "embedding": merged,
        "user_config": crate::config::user_config_path().to_string_lossy(),
        "project_dir": crate::config::find_project_dir().map(|p| p.to_string_lossy().into_owned()),
    });
    resp(stream, 200, &serde_json::to_string(&v).unwrap())
}

/// 压实：只留存活重写回单文件
fn api_compact(data_dir: &std::path::Path, stream: &mut TcpStream) -> Result<(), String> {
    let mut s = Store::open(data_dir);
    match s.compact() {
        Ok(before) => {
            let v = serde_json::json!({"lines_before": before, "records": s.count()});
            resp(stream, 200, &v.to_string())
        }
        Err(e) => resp(stream, 400, &serde_json::json!({"error": e.to_string()}).to_string()),
    }
}

/// 回填缺失向量（须嵌入 API 在配；增量只算没有的）
fn api_reembed(data_dir: &std::path::Path, stream: &mut TcpStream) -> Result<(), String> {
    let Some(cfg) = crate::embed::embedding_config() else {
        return resp(stream, 400, r#"{"error":"未配置嵌入 API"}"#);
    };
    let s = Store::open(data_dir);
    let mut vidx = crate::vectors::VectorIndex::open(data_dir);
    let (mut done, mut failed) = (0u32, 0u32);
    for r in s.all() {
        if vidx.has(&r.id) {
            continue;
        }
        match crate::embed::embed(&cfg, &format!("{}\n{}", r.title, r.content)) {
            Ok(v) => {
                if vidx.put(&r.id, &v).is_ok() {
                    done += 1;
                }
            }
            Err(_) => failed += 1,
        }
    }
    let out = serde_json::json!({"embedded": done, "failed": failed, "vectors": vidx.count(), "total": s.count()});
    resp(stream, 200, &out.to_string())
}

/// 同 id 全部历史版本（append-only 日志天然保历史）
fn api_versions(data_dir: &std::path::Path, id: &str, stream: &mut TcpStream) -> Result<(), String> {
    let s = Store::open(data_dir);
    let target = s.resolve(id).map(|r| r.id.clone());
    let Some(rid) = target else {
        return resp(stream, 404, r#"{"error":"记忆不存在"}"#);
    };
    let vs = s.versions(&rid);
    let arr: Vec<serde_json::Value> = vs.iter().map(|r| serde_json::to_value(r).unwrap()).collect();
    resp(stream, 200, &serde_json::json!({"id": rid, "versions": arr}).to_string())
}

/// 标签频次（top 100）
fn api_tags(data_dir: &std::path::Path, stream: &mut TcpStream) -> Result<(), String> {
    let s = Store::open(data_dir);
    let arr: Vec<serde_json::Value> = s.tag_counts().into_iter().take(100)
        .map(|(t, n)| serde_json::json!({"tag": t, "count": n})).collect();
    resp(stream, 200, &serde_json::json!({"tags": arr}).to_string())
}

/// project 频次
fn api_projects(data_dir: &std::path::Path, stream: &mut TcpStream) -> Result<(), String> {
    let s = Store::open(data_dir);
    let arr: Vec<serde_json::Value> = s.project_counts().into_iter()
        .map(|(p, n)| serde_json::json!({"project": p, "count": n})).collect();
    resp(stream, 200, &serde_json::json!({"projects": arr}).to_string())
}

/// 注入探测（各软件 rules/agents 载体现状）
fn api_inject_probe(stream: &mut TcpStream) -> Result<(), String> {
    resp(stream, 200, &serde_json::to_string(&crate::inject::probe()).unwrap())
}

/// 注入执行：{target, remove?:bool, force?:bool}——正文取 ~/.qoder/rules/AGENTS.md 全文
fn api_inject_do(body: &str, stream: &mut TcpStream) -> Result<(), String> {
    let v: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return resp(stream, 400, r#"{"error":"body 不是 JSON"}"#),
    };
    let target = v["target"].as_str().unwrap_or("").to_string();
    if target.is_empty() {
        return resp(stream, 400, r#"{"error":"缺 target"}"#);
    }
    let out = if v["remove"].as_bool().unwrap_or(false) {
        crate::inject::remove(&target)
    } else {
        crate::inject::inject(&target, v["force"].as_bool().unwrap_or(false), false)
    };
    resp(stream, 200, &out.to_string())
}

fn api_remember(data_dir: &std::path::Path, body: &str, stream: &mut TcpStream) -> Result<(), String> {
    let v: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return resp(stream, 400, r#"{"error":"body 不是 JSON"}"#),
    };
    let content = v["content"].as_str().unwrap_or("").trim().to_string();
    let title = v["title"].as_str().unwrap_or("").trim().to_string();
    let tags: Vec<String> = v["tags"]
        .as_array()
        .map(|a| a.iter().filter_map(|t| t.as_str()).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect())
        .unwrap_or_default();
    if content.is_empty() {
        return resp(stream, 400, r#"{"error":"content 为空"}"#);
    }
    // 敏感闸与 CLI 同一把尺
    let hits = sensitive::scan(&format!("{title}\n{content}"));
    if !hits.is_empty() {
        let e = serde_json::json!({"error": format!("命中敏感信息（{}），改存位置指针", hits.join("、"))});
        return resp(stream, 400, &e.to_string());
    }
    // 缺省标题截正文首句（与 CLI 一致）
    let title = if title.is_empty() {
        let head = content.lines().next().unwrap_or("");
        let mut cut = head;
        for sep in ["。", "！", "？", ". ", "! ", "? "] {
            if let Some(p) = head.find(sep) {
                let cand = &head[..p];
                if cand.len() < cut.len() { cut = cand; }
            }
        }
        cut.chars().take(40).collect::<String>()
    } else {
        title
    };
    let mut s = Store::open(data_dir);
    match s.put(&content, &title, &tags, "") {
        Ok(rid) => {
            // 有嵌入 API 顺手写侧车
            let mut embedded = false;
            if let Some(cfg) = crate::embed::embedding_config() {
                if let Ok(vec) = crate::embed::embed(&cfg, &format!("{title}\n{content}")) {
                    embedded = crate::vectors::VectorIndex::open(data_dir).put(&rid, &vec).is_ok();
                }
            }
            let v = serde_json::json!({"id": rid, "title": title, "embedded": embedded, "records": s.count()});
            resp(stream, 200, &v.to_string())
        }
        Err(e) => resp(stream, 400, &serde_json::json!({"error": e}).to_string()),
    }
}

fn api_forget(data_dir: &std::path::Path, body: &str, stream: &mut TcpStream) -> Result<(), String> {
    let v: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => return resp(stream, 400, r#"{"error":"body 不是 JSON"}"#),
    };
    let id = v["id"].as_str().unwrap_or("").to_string();
    let mut s = Store::open(data_dir);
    let target = s.resolve(&id).map(|r| r.id.clone());
    match target {
        Some(rid) => {
            let r = s.delete(&rid);
            match r {
                Ok(()) => resp(stream, 200, &serde_json::json!({"deleted": rid}).to_string()),
                Err(e) => resp(stream, 400, &serde_json::json!({"error": e}).to_string()),
            }
        }
        None => resp(stream, 404, r#"{"error":"记忆不存在"}"#),
    }
}

fn api_sync(data_dir: &std::path::Path, stream: &mut TcpStream) -> Result<(), String> {
    match syncmod::sync(data_dir) {
        Ok(r) => {
            let v = serde_json::json!({
                "committed": r.committed, "verified": r.verified,
                "local_sha": r.local_sha, "remote_sha": r.remote_sha,
                "repo": r.repo.to_string_lossy(),
            });
            resp(stream, 200, &v.to_string())
        }
        Err(e) => resp(stream, 400, &serde_json::json!({"error": e}).to_string()),
    }
}

/// 单页 UI：照 respire deb 客户端（shell-ui 构建产物）蓝本重写——
/// 左栏三组导航（记忆/运行/设置）＋页签、明暗双主题、面板/统计卡/徽标/浮层/吐司一整套，
/// 数据层接本文件上方的 /api/*。全部内联，零外链零 CDN（本地回环明文库不联网）。
const PAGE: &str = r##"<!DOCTYPE html>
<html lang="zh"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>memlite 记忆</title>
<style>
:root{--bg:#f2f3f5;--panel:#fff;--text:#27292d;--muted:#82858d;--faint:#a9acb4;--line:#eaecef;--line-strong:#dfe1e6;--soft:#f7f8fa;--hover:#eeeef2;--accent:#6856c9;--accent-soft:#eeecf9;--ok:#4c927a;--ok-soft:#e7f1ed;--warn:#c9964c;--warn-soft:#faf1e3;--bad:#c46a6a;--bad-soft:#f9ebeb;--black:#242527;--shadow:0 5px 22px #21242d05,0 1px 4px #21242d03;--shadow-lift:0 18px 60px #24283425,0 4px 10px #24283410;
--ui-font:"LXGW Marker Gothic","霞鹜漫黑","SF Pro Text","PingFang SC","Microsoft YaHei",sans-serif;
--mono:ui-monospace,SFMono-Regular,"SF Mono",Menlo,Consolas,monospace;--r-lg:20px;--r-md:12px;--r-sm:8px}
html[data-theme=dark]{--bg:#1e2024;--panel:#282b30;--text:#e1e3e8;--muted:#949ba8;--faint:#767d8a;--line:#373b44;--line-strong:#43474f;--soft:#30333a;--hover:#3a3c46;--accent:#9a8ae4;--accent-soft:#322e46;--ok:#7fb9a2;--ok-soft:#24352f;--warn:#d8ad6e;--warn-soft:#352e22;--bad:#dc8b8b;--bad-soft:#382626;--black:#44414c;--shadow:0 5px 22px #0000000a;--shadow-lift:0 22px 70px #00000040}
*{box-sizing:border-box}html,body,#root{height:100%}body{margin:0;background:var(--bg);color:var(--text);font-family:var(--ui-font);font-size:14px;font-synthesis:none;-webkit-font-smoothing:antialiased}
h1,h2,h3,p{margin:0}button,input,select,textarea{font:inherit;color:inherit}button{cursor:pointer;border:0;background:none;transition:background .16s,box-shadow .16s,color .16s,border-color .16s}button:disabled{opacity:.4;cursor:default}
input,select,textarea{outline:none}:focus-visible{outline:2px solid var(--accent);outline-offset:3px;border-radius:4px}::selection{background:#ded7fb}
::-webkit-scrollbar{width:8px;height:8px}::-webkit-scrollbar-thumb{background:#c4c5cc66;border-radius:8px}::-webkit-scrollbar-track{background:transparent}
.shell{height:100dvh;display:grid;grid-template-columns:236px minmax(0,1fr);gap:12px;padding:12px;overflow:hidden}
.sidebar{background:var(--panel);border:1px solid var(--line);border-radius:var(--r-lg);box-shadow:var(--shadow);display:flex;flex-direction:column;min-height:0;padding:16px 12px 12px}
.brand{display:flex;align-items:center;gap:10px;padding:0 8px 4px}
.brand-mark{width:30px;height:30px;border-radius:9px;background:#163d35;color:#fff;display:grid;place-items:center;font-size:13px;font-weight:700;letter-spacing:-.5px}
.brand-text strong{display:block;font-size:14px;font-weight:600;letter-spacing:-.2px}.brand-text span{display:block;font-size:11px;color:var(--faint);letter-spacing:.4px;margin-top:1px}
.nav{margin-top:20px;display:flex;flex-direction:column;gap:2px;overflow-y:auto;min-height:0;padding-right:2px}
.nav-item{display:flex;align-items:center;gap:11px;width:100%;text-align:left;padding:9px 11px;border-radius:var(--r-sm);color:var(--muted);font-size:13.5px;position:relative}
.nav-item svg{flex-shrink:0;opacity:.75}.nav-item:hover{background:var(--hover);color:var(--text)}.nav-item.active{background:var(--accent-soft);color:var(--accent);font-weight:600}
.nav-item.active svg{opacity:1}.nav-item.active:before{content:"";position:absolute;left:-12px;top:7px;bottom:7px;width:2px;border-radius:0 2px 2px 0;background:var(--accent)}
.sb-foot{margin-top:auto;padding:12px 4px 2px;border-top:1px solid var(--line);display:flex;flex-direction:column;gap:9px}
.sb-status{display:flex;align-items:center;justify-content:space-between;gap:8px}
.sb-status code{font-family:var(--mono);font-size:11px;color:var(--faint)}
.sb-meta{display:flex;align-items:center;justify-content:space-between;font-size:11.5px;color:var(--faint)}
.sb-actions{display:flex;align-items:center;gap:4px}.sb-link{padding:3px 7px;border-radius:6px;font-size:11.5px;color:var(--faint)}.sb-link:hover{background:var(--hover);color:var(--text)}
.main{display:flex;flex-direction:column;min-width:0;min-height:0;gap:10px}
.content{flex:1;min-height:0;overflow-y:auto;scrollbar-gutter:stable;padding:2px 8px 24px}
.content>.page,.content>#pageRoot>.page{max-width:1180px;margin:0 auto;animation:rise .22s ease}
@keyframes rise{0%{opacity:0;transform:translateY(6px)}to{opacity:1;transform:none}}
.page-head{margin-bottom:16px}.page-head h1{font-size:19px;font-weight:600;letter-spacing:-.4px}
.tabbar{display:flex;gap:2px;flex-wrap:wrap;border-bottom:1px solid var(--line);margin-bottom:16px}
.tab{position:relative;padding:9px 13px 11px;font-size:13.5px;color:var(--muted);border-bottom:2px solid transparent;margin-bottom:-1px}
.tab:hover{color:var(--text)}.tab.active{color:var(--accent);font-weight:600;border-bottom-color:var(--accent)}
.panel{background:var(--panel);border:1px solid var(--line);border-radius:var(--r-lg);box-shadow:var(--shadow);padding:20px 22px}
.panel+.panel{margin-top:14px}
.panel-head{display:flex;align-items:center;justify-content:space-between;gap:14px;margin-bottom:16px}
.panel-head h2{font-size:15px;font-weight:600;letter-spacing:-.2px}.panel-head .hint{font-size:12px;color:var(--faint)}
.panel-desc{font-size:12.5px;color:var(--muted);line-height:1.75;margin:-8px 0 16px;max-width:68ch}
.grid{display:grid;gap:12px}.grid.cols-2{grid-template-columns:repeat(2,minmax(0,1fr))}.grid.cols-3{grid-template-columns:repeat(3,minmax(0,1fr))}.grid.cols-4{grid-template-columns:repeat(4,minmax(0,1fr))}
@media(max-width:1100px){.grid.cols-3,.grid.cols-4{grid-template-columns:repeat(2,minmax(0,1fr))}}
@media(max-width:720px){.grid.cols-2,.grid.cols-3,.grid.cols-4{grid-template-columns:minmax(0,1fr)}}
.stat{background:var(--soft);border:1px solid var(--line);border-radius:var(--r-md);padding:13px 15px}
.stat .label{font-size:11.5px;color:var(--faint);letter-spacing:.3px}
.stat .value{font-family:var(--mono);font-size:21px;font-weight:600;letter-spacing:-.6px;margin-top:6px;font-variant-numeric:tabular-nums}
.stat .value.text{font-family:var(--ui-font);font-size:15px;letter-spacing:-.2px;font-weight:600;word-break:break-all}
.stat .foot{font-size:11.5px;color:var(--muted);margin-top:5px}
.stat.ok{background:var(--ok-soft);border-color:transparent}.stat.warn{background:var(--warn-soft);border-color:transparent}.stat.bad{background:var(--bad-soft);border-color:transparent}.stat.accent{background:var(--accent-soft);border-color:transparent}
.kv{display:grid;grid-template-columns:118px minmax(0,1fr);gap:10px 16px;align-items:baseline}
.kv dt{font-size:12.5px;color:var(--faint)}.kv dd{margin:0;font-size:13.5px;word-break:break-word}.kv dd.mono{font-family:var(--mono);font-size:12.5px}
.rows{border-top:1px solid var(--line)}.row{display:flex;align-items:center;gap:14px;padding:13px 4px;border-bottom:1px solid var(--line)}.row:last-child{border-bottom:0}
.row-main{flex:1;min-width:0}.row-main strong{display:block;font-size:13.5px;font-weight:600}.row-main span{display:block;font-size:12.5px;color:var(--muted);margin-top:4px;line-height:1.6;word-break:break-word}
.row-side{display:flex;align-items:center;gap:8px;flex-shrink:0}
table{width:100%;border-collapse:collapse;font-size:13px}th,td{text-align:left;padding:10px 8px;border-bottom:1px solid var(--line)}
th{font-size:11.5px;color:var(--faint);font-weight:500;letter-spacing:.5px;text-transform:uppercase}td.mono{font-family:var(--mono);font-size:12.5px;font-variant-numeric:tabular-nums}
tbody tr:last-child td{border-bottom:0}tbody tr:hover td{background:var(--soft)}
.btn{display:inline-flex;align-items:center;justify-content:center;gap:7px;border-radius:10px;padding:9px 15px;background:var(--panel);border:1px solid var(--line-strong);font-size:13px;font-weight:500;white-space:nowrap}
.btn:hover{background:var(--hover)}.btn.primary{background:var(--black);border-color:var(--black);color:#fff}.btn.primary:hover{background:#3d3d43}
.btn.accent{background:var(--accent);border-color:var(--accent);color:#fff}.btn.accent:hover{filter:brightness(1.06)}
.btn.danger{color:var(--bad);border-color:#e6c9c9;background:var(--bad-soft)}.btn.ghost{background:transparent;border-color:transparent;color:var(--muted)}.btn.ghost:hover{background:var(--hover);color:var(--text)}
.btn.sm{padding:6px 11px;font-size:12.5px;border-radius:8px}
.field{display:block;margin-bottom:14px}.field>span{display:block;font-size:12.5px;color:var(--muted);margin-bottom:7px}
.field input,.field select,.field textarea{display:block;width:100%;background:var(--soft);border:1px solid var(--line-strong);border-radius:10px;padding:10px 12px;font-size:13.5px}
.field input:focus,.field select:focus,.field textarea:focus{border-color:var(--accent);background:var(--panel)}
.field .help{font-size:11.5px;color:var(--faint);margin-top:6px;line-height:1.6}.field textarea{resize:vertical;min-height:150px;line-height:1.8}
.badge{display:inline-flex;align-items:center;gap:6px;font-size:11.5px;padding:4px 9px;border-radius:20px;background:var(--soft);border:1px solid var(--line);color:var(--muted);white-space:nowrap}
.badge .dot{width:6px;height:6px;border-radius:50%;background:currentColor}
.badge.ok{background:var(--ok-soft);border-color:transparent;color:var(--ok)}.badge.warn{background:var(--warn-soft);border-color:transparent;color:var(--warn)}
.badge.bad{background:var(--bad-soft);border-color:transparent;color:var(--bad)}.badge.accent{background:var(--accent-soft);border-color:transparent;color:var(--accent)}
.badge.mono{font-family:var(--mono);font-variant-numeric:tabular-nums}.pulse{animation:pulse 1.6s ease-in-out infinite}@keyframes pulse{50%{opacity:.3}}
.backdrop{position:fixed;inset:0;z-index:90;background:#27293735;-webkit-backdrop-filter:blur(7px);backdrop-filter:blur(7px);display:flex;align-items:center;justify-content:center;padding:30px;animation:fade .16s ease}
@keyframes fade{0%{opacity:0}to{opacity:1}}
.sheet{background:var(--panel);border:1px solid #ffffff80;width:470px;max-width:100%;max-height:calc(100dvh - 60px);overflow-y:auto;border-radius:18px;box-shadow:var(--shadow-lift);padding:24px;animation:sheet-in .22s cubic-bezier(.2,.7,.3,1)}
.sheet.wide{width:620px}@keyframes sheet-in{0%{opacity:0;transform:translateY(10px) scale(.985)}to{opacity:1;transform:none}}
.sheet>header{display:flex;align-items:flex-start;justify-content:space-between;gap:16px;margin-bottom:18px}.sheet h2{font-size:17px;font-weight:600;letter-spacing:-.3px}
.sheet .lead{font-size:13px;color:var(--muted);line-height:1.75;margin-top:7px}.sheet footer{display:flex;justify-content:flex-end;gap:9px;margin-top:22px}
.toast{position:fixed;left:50%;bottom:26px;transform:translate(-50%);z-index:120;background:#292b30f2;color:#f1f0f5;padding:12px 17px;border-radius:13px;box-shadow:0 8px 25px #25273325;display:flex;align-items:center;gap:10px;font-size:13.5px;max-width:80vw;animation:rise .2s ease}
.toast.bad{background:#4a2b2bf2}.toast.ok{background:#234036f2}
.muted{color:var(--muted);font-size:13px;line-height:1.75}.faint{color:var(--faint);font-size:12px}.mono{font-family:var(--mono);font-variant-numeric:tabular-nums}
.empty{text-align:center;padding:52px 20px;color:var(--muted)}.empty strong{display:block;font-size:15px;color:var(--text);margin-bottom:8px}
.stack{display:flex;flex-direction:column;gap:14px}.inline{display:flex;align-items:center;gap:9px;flex-wrap:wrap}.divider{height:1px;background:var(--line);margin:18px 0}
.se-bar{display:flex;align-items:flex-end;gap:10px}.se-bar .se-field{flex:1;min-width:0;margin:0}
.se-opts{display:flex;align-items:center;gap:12px;flex-wrap:wrap;margin-top:12px;padding-top:12px;border-top:1px solid var(--line)}
.se-opts .se-lab{font-size:11.5px;color:var(--faint);margin-right:-6px}
.se-seg{display:inline-flex;gap:2px;padding:2px;background:var(--soft);border:1px solid var(--line-strong);border-radius:10px}
.se-seg .btn{border:0;background:transparent;padding:5px 12px;font-size:12.5px;border-radius:8px}
.se-seg .btn.on{background:var(--panel);color:var(--accent);font-weight:600;box-shadow:var(--shadow)}
.se-select{background:var(--soft);border:1px solid var(--line-strong);border-radius:10px;padding:6px 9px;font-size:12.5px}
.se-meta{display:flex;align-items:center;gap:8px;flex-wrap:wrap;margin-bottom:12px}
.se-cols{display:grid;grid-template-columns:minmax(300px,350px) minmax(0,1fr);gap:12px;align-items:start}
@media(max-width:1100px){.se-cols{grid-template-columns:minmax(0,1fr)}}
.se-list{border-top:1px solid var(--line);max-height:60vh;overflow-y:auto}
.se-hit{display:flex;align-items:flex-start;gap:10px;width:100%;text-align:left;padding:11px 6px;border-bottom:1px solid var(--line)}
.se-hit:last-child{border-bottom:0}.se-hit:hover{background:var(--soft)}.se-hit.on{background:var(--accent-soft)}
.se-hit .se-id{flex-shrink:0;padding-top:2px;font-family:var(--mono);font-size:11px;color:var(--faint)}.se-hit.on .se-id{color:var(--accent)}
.se-hit .se-title{flex:1;min-width:0;font-size:13px;line-height:1.6;word-break:break-word}.se-hit.on .se-title{font-weight:600}
.se-hit .se-score{flex-shrink:0;font-family:var(--mono);font-size:11.5px;color:var(--muted)}
.se-title-lg{font-size:17px;font-weight:600;letter-spacing:-.3px;line-height:1.5;word-break:break-word}
.se-body{font-family:var(--ui-font);font-size:13.5px;line-height:1.95;margin:14px 0 0;white-space:pre-wrap;word-break:break-word}
.se-actions{display:flex;align-items:center;flex-wrap:wrap;gap:8px;margin-top:15px}
.se-badbox{background:var(--bad-soft);color:var(--bad);border-radius:var(--r-md);padding:11px 13px;font-size:12.5px;line-height:1.75;white-space:pre-wrap;word-break:break-word}
.se-warnbox{background:var(--warn-soft);border-radius:var(--r-md);padding:11px 13px;font-size:12.5px;line-height:1.75}
.chip{display:inline-flex;align-items:center;gap:5px;background:var(--panel);border:1px solid var(--line);border-radius:8px;padding:6px 12px;margin:0 6px 8px 0;cursor:pointer;font-size:13px}
.chip:hover{border-color:var(--accent);color:var(--accent)}.chip .n{color:var(--faint);font-size:12px}
.ver{border-left:3px solid var(--line);padding:6px 0 6px 14px;margin:10px 0}.ver.cur{border-left-color:var(--accent)}
.ver .vh{font-size:12.5px;color:var(--faint)}.ver .vc{font-size:13px;line-height:1.9;margin-top:4px;white-space:pre-wrap}
@media(max-width:900px){.shell{grid-template-columns:56px minmax(0,1fr);padding:8px;gap:8px}.sidebar{padding:12px 6px 8px;align-items:center}
.brand{padding:0 0 8px}.brand-text{display:none}.nav-item{justify-content:center;padding:11px 0}.nav-item span{display:none}.nav-item.active:before{left:-6px}
.sb-foot{width:100%;align-items:center;padding:10px 0 2px}.sb-status code,.sb-meta>span:first-child{display:none}.sb-foot .btn{width:100%;padding:8px 4px;font-size:11px}}
@media(prefers-reduced-motion:reduce){*,*:before,*:after{animation:none!important;transition:none!important}}
</style>
</head><body><div id="root">
<div class="shell">
  <aside class="sidebar">
    <div class="brand"><div class="brand-mark">ml</div><div class="brand-text"><strong>memlite</strong><span>本地记忆库</span></div></div>
    <nav class="nav" id="nav"></nav>
    <div class="sb-foot">
      <div class="sb-status"><span class="badge" id="sbBadge"><i class="dot pulse"></i>载入中</span><code id="sbCode">—</code></div>
      <div class="sb-meta"><span id="sbTier">—</span>
        <span class="sb-actions">
          <button class="sb-link" id="sbSync" title="git 同步">同步</button>
          <button class="sb-link" id="sbTheme" aria-label="切换主题">☾</button>
        </span>
      </div>
    </div>
  </aside>
  <main class="main"><div class="content"><div id="pageRoot"></div></div></main>
</div>
<div id="modal"></div>
<div id="toasts"></div>
</div>
<script>
const $=id=>document.getElementById(id);
function esc(s){return (s==null?'':String(s)).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]))}
function dt(ts){const d=new Date(ts*1000),p=n=>String(n).padStart(2,'0');
  return d.getFullYear()+'-'+p(d.getMonth()+1)+'-'+p(d.getDate())+' '+p(d.getHours())+':'+p(d.getMinutes())}
function day(ts){return new Date(ts*1000).toISOString().slice(0,10)}
async function j(url,opts){const r=await fetch(url,opts);let d=null;try{d=await r.json()}catch(e){}
  if(!r.ok)throw Error((d&&d.error)||('HTTP '+r.status));return d}
function toast(msg,tone){const d=document.createElement('div');d.className='toast '+(tone||'');d.textContent=msg;
  $('toasts').appendChild(d);setTimeout(()=>d.remove(),4200)}
function busy(btn,on,label){if(!btn)return;if(on){btn.dataset.old=btn.textContent;btn.disabled=true;btn.textContent=label||'执行中…'}
  else{btn.disabled=false;if(btn.dataset.old)btn.textContent=btn.dataset.old}}

/* ── 浮层：确认 / 表单（respire 同款 .backdrop+.sheet） ── */
function closeModal(){$('modal').innerHTML=''}
function confirmSheet({title,lead,label,danger}){return new Promise(res=>{
  $('modal').innerHTML=`<div class="backdrop" data-bd><div class="sheet"><header><div><h2>${esc(title)}</h2>${lead?`<div class="lead">${esc(lead)}</div>`:''}</div></header>
    <footer><button class="btn" data-no>取消</button><button class="btn ${danger?'danger':'accent'}" data-yes>${esc(label||'确认')}</button></footer></div></div>`;
  const done=v=>{closeModal();res(v)};
  $('modal').querySelector('[data-bd]').onclick=e=>{if(e.target.hasAttribute('data-bd'))done(false)};
  $('modal').querySelector('[data-no]').onclick=()=>done(false);
  $('modal').querySelector('[data-yes]').onclick=()=>done(true);
})}
function formSheet({title,lead,rec}){return new Promise(res=>{
  $('modal').innerHTML=`<div class="backdrop" data-bd><div class="sheet wide"><header><div><h2>${esc(title)}</h2>${lead?`<div class="lead">${esc(lead)}</div>`:''}</div></header>
    <label class="field"><span>标题</span><input id="fTitle" placeholder="留空自动截正文首句" value="${esc(rec?rec.title:'')}"></label>
    <label class="field"><span>正文</span><textarea id="fBody" placeholder="口令/证件号等敏感信息会拒入">${esc(rec?rec.content:'')}</textarea></label>
    <label class="field"><span>标签</span><input id="fTags" placeholder="逗号分隔" value="${esc(rec?(rec.tags||[]).join(','):'')}"><div class="help">一事一条，标题写事实句：主语＋动作＋关键实体或结果</div></label>
    <footer><button class="btn" data-no>取消</button><button class="btn accent" data-yes>保存</button></footer></div></div>`;
  const out=v=>{const r=v?{title:$('fTitle').value.trim(),content:$('fBody').value.trim(),
    tags:$('fTags').value.split(',').map(s=>s.trim()).filter(Boolean)}:null;closeModal();res(r)};
  $('modal').querySelector('[data-bd]').onclick=e=>{if(e.target.hasAttribute('data-bd'))out(null)};
  $('modal').querySelector('[data-no]').onclick=()=>out(null);
  $('modal').querySelector('[data-yes]').onclick=()=>out(true);
  $('fBody').focus();
})}

/* ── 主题 ── */
function setTheme(t){document.documentElement.dataset.theme=t;
  try{localStorage.setItem('memlite.theme',t)}catch(e){}
  const b=$('sbTheme');if(b)b.textContent=t==='dark'?'☀':'☾'}
(function(){let t=null;try{t=localStorage.getItem('memlite.theme')}catch(e){}
  setTheme(t|| (matchMedia('(prefers-color-scheme:dark)').matches?'dark':'light'))})();

/* ── 导航：三组（照 respire 侧栏） ── */
const ICONS={
 memory:'<svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><rect x="2.5" y="2.5" width="11" height="11" rx="3"/><path d="M5.2 6.4h5.6M5.2 9.6h3.4"/></svg>',
 runtime:'<svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M1.8 8h2.6l1.8-4.4L8.9 12.4l1.7-4.4h2.6"/></svg>',
 settings:'<svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><circle cx="8" cy="8" r="2.4"/><path d="M8 1.6v2M8 12.4v2M1.6 8h2M12.4 8h2M3.6 3.6l1.4 1.4M11 11l1.4 1.4M12.4 3.6 11 5M5 11l-1.4 1.4"/></svg>'};
const GROUPS=[
 {key:'memory',label:'记忆',tabs:[['search','检索'],['timeline','时间线'],['tags','标签 · 项目']]},
 {key:'runtime',label:'运行',tabs:[['status','状态'],['inject','注入'],['sync','同步']]},
 {key:'settings',label:'设置',tabs:[['querylog','检索日志'],['maintain','维护'],['appearance','外观']]}];
let G='memory',T='search';
function go(g,t){G=g;const grp=GROUPS.find(x=>x.key===g);T=t||grp.tabs[0][0];renderShell()}
function renderNav(){
  $('nav').innerHTML=GROUPS.map(g=>`<button class="nav-item ${g.key===G?'active':''}" data-g="${g.key}" title="${esc(g.label)}">${ICONS[g.key]}<span>${esc(g.label)}</span></button>`).join('');
  $('nav').querySelectorAll('[data-g]').forEach(b=>b.onclick=()=>go(b.dataset.g));
}
function renderTabs(){
  const grp=GROUPS.find(x=>x.key===G);
  $('pageRoot').innerHTML=`<div class="page"><div class="page-head"><h1>${esc(grp.label)}</h1></div>
    <div class="tabbar">${grp.tabs.map(([k,l])=>`<button class="tab ${k===T?'active':''}" data-t="${k}">${esc(l)}</button>`).join('')}</div><div id="pane"></div></div>`;
  $('pageRoot').querySelectorAll('[data-t]').forEach(b=>b.onclick=()=>{T=b.dataset.t;renderShell()});
}
const PANES={search:renderSearch,timeline:renderTimeline,tags:renderTags,status:renderStatus,inject:renderInject,sync:renderSync,querylog:renderQuerylog,maintain:renderMaintain,appearance:renderAppearance};
function renderShell(){renderNav();renderTabs();(PANES[T]||renderSearch)()}

/* ── 顶栏状态 ── */
let ST=null;
async function loadStatus(){try{ST=await j('/api/status');
  $('sbBadge').className='badge ok';$('sbBadge').innerHTML='<i class="dot"></i>'+ST.records+' 条';
  $('sbCode').textContent=ST.embedding.configured?'semantic':'bm25';
  $('sbTier').textContent=(ST.project_dir?'项目级':'用户级')+' · 查询 '+ST.queries;
  if(ST.bad_lines)toast('警告：'+ST.bad_lines+' 行损坏','bad');
}catch(e){$('sbBadge').className='badge bad';$('sbBadge').innerHTML='<i class="dot"></i>未连通';toast('状态加载失败：'+e.message,'bad')}}
$('sbTheme').onclick=()=>setTheme(document.documentElement.dataset.theme==='dark'?'light':'dark');
$('sbSync').onclick=()=>go('runtime','sync');

/* ── 记忆 · 检索 ── */
let SR={q:'',backend:'',fallback:'',hits:[],sel:null,loading:false,mode:'auto',limit:20};
async function renderSearch(){
  $('pane').innerHTML=`
  <section class="panel">
    <div class="se-bar">
      <label class="field se-field"><span>查询</span>
        <input id="q" placeholder="关键词、句子，或「项目名+核心词」" value="${esc(SR.q)}" aria-label="查询"></label>
      <button class="btn primary" id="qgo">查询</button>
      <button class="btn" id="addBtn">＋ 新增</button>
    </div>
    <div class="se-opts">
      <span class="se-lab">档位</span>
      <div class="se-seg" id="modeSeg">
        <button class="btn sm ${SR.mode==='auto'?'on':''}" data-mode="auto">自动</button>
        <button class="btn sm ${SR.mode==='bm25'?'on':''}" data-mode="bm25">词面</button>
      </div>
      <span class="se-lab">上限</span>
      <select class="se-select" id="limitSel">${[5,10,20,50].map(n=>`<option value="${n}" ${n===SR.limit?'selected':''}>${n}</option>`).join('')}</select>
      <span class="faint" style="margin-left:auto">每次检索落 query_log（训练/复盘取材）</span>
    </div>
  </section>
  <div class="se-cols" style="margin-top:14px">
    <section class="panel" id="hitsPanel"></section>
    <div class="stack"><section class="panel" id="detailPanel"></section></div>
  </div>`;
  $('q').onkeydown=e=>{if(e.key==='Enter')doSearch()};
  $('qgo').onclick=doSearch;
  $('addBtn').onclick=addMemory;
  $('modeSeg').querySelectorAll('[data-mode]').forEach(b=>b.onclick=()=>{SR.mode=b.dataset.mode;renderSearch()});
  $('limitSel').onchange=e=>{SR.limit=Number(e.target.value)};
  drawHits();drawDetail();
  if(SR.q&&SR.hits.length===0&&!SR.loading){/* 首屏不自动重查，保留上次为空态 */}
}
async function doSearch(){
  const q=($('q').value||'').trim();
  if(!q){toast('请输入查询词','bad');return}
  SR.q=q;SR.loading=true;drawHits();
  try{
    const d=await j('/api/search?q='+encodeURIComponent(q)+'&limit='+SR.limit+'&mode='+SR.mode);
    SR.backend=d.backend;SR.fallback=d.fallback;SR.hits=d.hits||[];SR.sel=SR.hits.length?SR.hits[0].id:null;
    if(d.log_err)toast(d.log_err,'bad');
    if(SR.sel)openDetail(SR.sel,true);else SR.detail=null;
  }catch(e){SR.hits=[];toast('搜索失败：'+e.message,'bad')}
  finally{SR.loading=false;drawHits();drawDetail()}
}
function drawHits(){
  const p=$('hitsPanel');if(!p)return;
  const head=`<div class="panel-head"><div><h2>命中</h2>${SR.q?`<div class="hint">${esc(SR.q)}</div>`:''}</div>
    ${SR.loading?'<span class="badge accent"><i class="dot pulse"></i>查询中</span>':''}</div>`;
  let body='';
  if(!SR.q)body=`<div class="empty"><strong>还没有查询</strong><div class="muted">输入查询词回车，或留空看时间线</div></div>`;
  else if(SR.loading&&!SR.hits.length)body=`<div class="empty"><strong>查询中…</strong></div>`;
  else if(!SR.hits.length)body=`<div class="empty"><strong>没有命中</strong><div class="muted">换 2–3 组词再试（零词面重合的改写靠语义档）</div></div>`;
  else{
    const meta=`<div class="se-meta"><span class="badge accent"><i class="dot"></i>命中 ${SR.hits.length}</span>
      <span class="badge">${SR.backend==='semantic'?'语义':'词面'}</span>
      <span class="badge">上限 ${SR.limit}</span>
      ${SR.fallback?'<span class="badge warn"><i class="dot"></i>项目级无命中已回退用户级</span>':''}</div>`;
    const rows=SR.hits.map(h=>`<button class="se-hit ${h.id===SR.sel?'on':''}" data-id="${esc(h.id)}" title="${esc(h.title)}">
      <span class="se-id">${esc(h.id.slice(0,8))}</span><span class="se-title">${esc(h.title||'(无标题)')}</span>
      <span class="se-score">${h.score.toFixed(3)}</span></button>`).join('');
    body=meta+`<div class="se-list">${rows}</div>`;
  }
  p.innerHTML=head+body;
  p.querySelectorAll('[data-id]').forEach(b=>b.onclick=()=>{SR.sel=b.dataset.id;drawHits();openDetail(b.dataset.id)});
}
async function openDetail(id,keepHit){
  const p=$('detailPanel');if(!p)return;
  p.innerHTML=`<div class="panel-head"><h2>详情</h2><span class="badge accent"><i class="dot pulse"></i>读取中</span></div>`;
  try{
    const r=await j('/api/show?id='+encodeURIComponent(id));
    SR.detail=r;SR.sel=r.id;if(!keepHit)drawHits();drawDetail();
  }catch(e){p.innerHTML=`<div class="panel-head"><h2>详情</h2></div><div class="se-badbox">${esc(e.message)}</div>`}
}
function drawDetail(){
  const p=$('detailPanel');if(!p)return;
  const r=SR.detail;
  if(!r){p.innerHTML=`<div class="panel-head"><h2>详情</h2></div><div class="empty"><strong>选一条看正文</strong><div class="muted">左栏点命中即读全文</div></div>`;return}
  p.innerHTML=`<div class="panel-head"><div><h2>详情</h2><div class="hint mono">${esc(r.id.slice(0,8))}</div></div>
      <div class="inline"><span class="badge accent"><i class="dot"></i>被召回 ${r.recall_count||0} 次</span></div></div>
    <div class="se-title-lg">${esc(r.title||'(无标题)')}</div>
    <div class="se-meta" style="margin-top:10px">
      <span class="badge">${dt(r.ts)}</span>
      ${(r.tags||[]).map(t=>`<span class="badge">${esc(t)}</span>`).join('')}
      ${r.project?`<span class="badge">${esc(r.project)}</span>`:''}
      ${(r.importance?`<span class="badge ${r.importance==='important'?'accent':''}">${esc(r.importance)}</span>`:'')}
      ${(r.kind?`<span class="badge">${esc(r.kind)}</span>`:'')}
    </div>
    <div class="se-body">${esc(r.content)}</div>
    <div class="se-actions">
      <button class="btn sm" id="dVer">版本历史</button>
      <button class="btn sm" id="dEdit">编辑</button>
      <button class="btn sm danger" id="dDel">删除</button>
      <button class="btn sm ghost" id="dCopy">复制 id</button>
    </div>
    <div id="verBox"></div>`;
  $('dVer').onclick=()=>showVersions(r.id);
  $('dEdit').onclick=async()=>{
    const v=await formSheet({title:'编辑记忆',lead:'就地改写同 id 新版本，末写胜出；改前自检三标仍在。',rec:r});
    if(!v)return;
    if(!v.content){toast('正文不能为空','bad');return}
    try{await j('/api/update',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({id:r.id,...v})});
      toast('已更新 '+r.id.slice(0,8),'ok');await openDetail(r.id);loadStatus()}
    catch(e){toast('更新失败：'+e.message,'bad')}
  };
  $('dDel').onclick=async()=>{
    const ok=await confirmSheet({title:`删除「${(r.title||'').slice(0,16)}」？`,lead:'立墓碑：从列表与检索消失，历史版本仍在 JSONL 里可追回。',danger:true,label:'删除'});
    if(!ok)return;
    try{await j('/api/forget',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({id:r.id})});
      toast('已删除','ok');SR.detail=null;loadStatus();
      if(SR.q)doSearch();else{drawDetail();drawHits()}
    }catch(e){toast('删除失败：'+e.message,'bad')}
  };
  $('dCopy').onclick=async()=>{try{await navigator.clipboard.writeText(r.id);toast('完整 id 已复制','ok')}
    catch(e){toast(e.message,'bad')}};
}
async function showVersions(id){
  try{
    const d=await j('/api/versions?id='+encodeURIComponent(id));
    $('verBox').innerHTML=`<div class="divider"></div><div class="faint">版本历史 · ${d.versions.length} 版（append-only 日志天然保历史，最新在末）</div>`+
      d.versions.map((v,i)=>`<div class="ver ${i===d.versions.length-1?'cur':''}"><div class="vh">${dt(v.ts)}${i===d.versions.length-1?'　（当前）':''}</div><div class="vc">${esc((v.content||'').slice(0,300))}</div></div>`).join('');
  }catch(e){toast(e.message,'bad')}
}
async function addMemory(){
  const v=await formSheet({title:'新增记忆',lead:'写前过敏感闸；标题缺省截正文首句 ≤40 字。'});
  if(!v)return;
  if(!v.content){toast('正文不能为空','bad');return}
  try{const d=await j('/api/remember',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(v)});
    toast('已存 '+d.id.slice(0,8)+'「'+d.title+'」','ok');loadStatus();
    SR.q='';SR.hits=[];SR.detail=null;renderSearch();
  }catch(e){toast('入库失败：'+e.message,'bad')}
}

/* ── 记忆 · 时间线 ── */
async function renderTimeline(){
  $('pane').innerHTML=`<section class="panel"><div class="panel-head"><h2>时间线</h2><span class="badge"><i class="dot"></i>按天分组</span></div><div id="tl" class="muted">读取中…</div></section>`;
  try{
    const d=await j('/api/list?limit=300&offset=0');
    const byDay={};for(const r of d.items){(byDay[day(r.ts)]=byDay[day(r.ts)]||[]).push(r)}
    const days=Object.keys(byDay).sort().reverse();
    $('tl').innerHTML=days.map(k=>`<div style="margin-bottom:14px"><div style="font-weight:600;color:var(--accent);margin-bottom:4px">${k}
        <span class="faint" style="font-weight:400">· ${byDay[k].length} 条</span></div>
        <div class="rows">${byDay[k].map(r=>`<div class="row"><div class="row-main"><strong>${esc(r.title||'(无标题)')}</strong>
          <span>${dt(r.ts).slice(11)} ${(r.tags||[]).map(t=>'#'+esc(t)).join(' ')}</span></div>
          <div class="row-side"><button class="btn sm" data-open="${esc(r.id)}">看正文</button></div></div>`).join('')}</div></div>`).join('')
      || '<div class="empty"><strong>无记录</strong></div>';
    $('tl').querySelectorAll('[data-open]').forEach(b=>b.onclick=()=>{go('memory','search');setTimeout(()=>{SR.sel=b.dataset.open;openDetail(b.dataset.open)},0)});
  }catch(e){$('tl').innerHTML=`<div class="se-badbox">${esc(e.message)}</div>`}
}

/* ── 记忆 · 标签 · 项目 ── */
async function renderTags(){
  $('pane').innerHTML=`<section class="panel"><div class="panel-head"><h2>标签</h2></div><div id="tagBox" class="muted">读取中…</div></section>
    <section class="panel"><div class="panel-head"><h2>项目</h2></div><div id="projBox" class="muted">读取中…</div></section>`;
  try{
    const [t,p]=await Promise.all([j('/api/tags'),j('/api/projects')]);
    $('tagBox').innerHTML=t.tags.map(x=>`<span class="chip" data-q="${esc(x.tag)}">#${esc(x.tag)} <span class="n">${x.count}</span></span>`).join('')||'<span class="muted">无标签</span>';
    $('projBox').innerHTML=p.projects.map(x=>`<span class="chip" data-q="${esc(x.project)}">${esc(x.project)} <span class="n">${x.count}</span></span>`).join('')||'<span class="muted">无项目</span>';
    document.querySelectorAll('[data-q]').forEach(c=>c.onclick=()=>{go('memory','search');
      setTimeout(()=>{const q=$('q');if(q){q.value=c.dataset.q;doSearch()}},0)});
  }catch(e){toast(e.message,'bad')}
}

/* ── 运行 · 状态 ── */
async function renderStatus(){
  $('pane').innerHTML=`<div class="grid cols-4" id="statGrid"><div class="stat"><div class="label">读取中</div><div class="value">—</div></div></div>
    <section class="panel"><div class="panel-head"><h2>库</h2><div class="inline">
      <button class="btn sm" id="stExport">导出 JSON</button><button class="btn sm" id="stSync">同步</button></div></div>
      <div id="stKv" class="muted">读取中…</div></section>
    <section class="panel"><div class="panel-head"><h2>文件</h2><span class="hint">data_dir 下实际占用</span></div><div id="stFiles"></div></section>`;
  try{
    const s=ST||await j('/api/status');ST=s;
    $('statGrid').innerHTML=[
      ['记忆条数',s.records,'ok'],['向量侧车',s.vectors,''],['检索日志',s.queries,'accent'],['坏行',s.bad_lines,s.bad_lines?'bad':'']
    ].map(([l,v,c])=>`<div class="stat ${c}"><div class="label">${l}</div><div class="value">${v}</div></div>`).join('');
    $('stKv').innerHTML=`<dl class="kv"><dt>数据目录</dt><dd class="mono">${esc(s.data_dir)}</dd>
      <dt>分层</dt><dd>${s.project_dir?'项目级（'+esc(s.project_dir)+'）':'用户级'}</dd>
      <dt>检索档位</dt><dd>${s.embedding.configured?'语义（语义失败自动回词面）':'词面 BM25（未配嵌入 API）'}</dd>
      <dt>同步白名单</dt><dd class="mono">memory*.jsonl · meta.json · README.md（含检索日志）</dd></dl>`;
    $('stFiles').innerHTML=`<table><thead><tr><th>文件</th><th>字节</th></tr></thead><tbody>${
      s.files.map(f=>`<tr><td class="mono">${esc(f.file)}</td><td class="mono">${f.bytes}</td></tr>`).join('')}</tbody></table>`;
    $('stExport').onclick=()=>window.open('/api/export','_blank');
    $('stSync').onclick=()=>go('runtime','sync');
  }catch(e){toast(e.message,'bad')}
}

/* ── 运行 · 注入 ── */
async function renderInject(){
  $('pane').innerHTML=`<section class="panel"><div class="panel-head"><div><h2>rules 注入</h2>
      <div class="hint">正文取 ~/.qoder/rules/AGENTS.md 全文（读不到即拒注）</div></div>
      <button class="btn sm" id="ijRefresh">刷新</button></div><div id="ijBox" class="muted">探测中…</div></section>`;
  const load=async()=>{
    try{
      const d=await j('/api/inject');
      $('ijBox').innerHTML=d.map(x=>{
        const st=x.state==='fresh'?'<span class="badge ok"><i class="dot"></i>已注入</span>'
          :x.state==='conflict'?'<span class="badge bad"><i class="dot"></i>他系已占</span>'
          :x.state==='stale'?'<span class="badge warn"><i class="dot"></i>待更新</span>':'<span class="badge">未注入</span>';
        const act=(x.state==='fresh'||x.state==='stale')
          ?`<button class="btn sm" data-ij="${esc(x.target)}" data-rm="1">移除</button><button class="btn sm" data-ij="${esc(x.target)}">更新</button>`
          :`<button class="btn sm" data-ij="${esc(x.target)}">注入</button>`;
        return `<div class="row"><div class="row-main"><strong>${esc(x.target)}</strong><span class="mono faint">${esc(x.path)}</span></div>
          <div class="row-side">${st}${act}</div></div>`}).join('');
      $('ijBox').querySelectorAll('[data-ij]').forEach(b=>b.onclick=async()=>{
        const rm=b.dataset.rm==='1';
        const ok=await confirmSheet({title:(rm?'移除 ':'向 ')+b.dataset.ij+(rm?' 的 memlite 规则块？':' 注入 memlite 规则？'),
          lead:rm?'块外内容安全，重跑注入可再写回。':'正文即注入源全文；已有 respire/1memory 规则会拒绝双注入。',danger:rm,label:rm?'移除':'注入'});
        if(!ok)return;
        try{const d2=await j('/api/inject',{method:'POST',headers:{'Content-Type':'application/json'},
          body:JSON.stringify({target:b.dataset.ij,remove:rm})});
          if(d2.error){toast('失败：'+d2.error,'bad');return}
          toast((rm?'已移除 ':'已处理 ')+b.dataset.ij+(d2.hint?'（'+d2.hint+'）':''),'ok');load();
        }catch(e){toast(e.message,'bad')}
      });
    }catch(e){$('ijBox').innerHTML=`<div class="se-badbox">${esc(e.message)}</div>`}
  };
  $('ijRefresh').onclick=load;load();
}

/* ── 运行 · 同步 ── */
async function renderSync(){
  $('pane').innerHTML=`<section class="panel"><div class="panel-head"><div><h2>git 同步</h2>
      <div class="hint">只 add 白名单（memory*.jsonl · meta.json · README.md，含检索日志）→ commit → pull --rebase → push → 双通道核验</div></div>
      <button class="btn accent" id="syRun">同步</button></div><div id="syBox" class="muted">读取中…</div></section>
    <section class="panel"><div class="panel-head"><h2>说明</h2></div>
      <div class="muted">数据仓是 <b>private 假设</b>：明文库的安全靠仓库可见性，仓转 public 即安全失效。
      凭据类已在 .gitignore，永不入库。</div></section>`;
  const load=async()=>{
    try{
      const s=await j('/api/sync-status');
      $('syBox').innerHTML=`<dl class="kv"><dt>仓库</dt><dd class="mono">${esc(s.repo||'—')}</dd>
        <dt>远端</dt><dd>${s.has_remote?'origin 已配':'未配 origin（先 git remote add）'}</dd>
        <dt>工作区</dt><dd>${s.dirty?'<span class="badge warn"><i class="dot"></i>白名单有未提交变更</span>':'<span class="badge ok"><i class="dot"></i>干净</span>'}</dd>
        <dt>本地 HEAD</dt><dd class="mono">${s.local_sha?esc(s.local_sha.slice(0,10)):'—'}</dd>
        ${s.note?`<dt>提示</dt><dd>${esc(s.note)}</dd>`:''}</dl>`;
    }catch(e){$('syBox').innerHTML=`<div class="se-badbox">${esc(e.message)}</div>`}
  };
  $('syRun').onclick=async()=>{
    busy($('syRun'),true,'同步中…');
    try{const d=await j('/api/sync',{method:'POST'});
      toast((d.verified?'同步成功':'核验异常')+'　'+(d.committed?'已提交':'无新变更')+'　'+d.local_sha.slice(0,8),d.verified?'ok':'bad');
    }catch(e){toast('同步失败：'+e.message,'bad')}
    finally{busy($('syRun'),false);load();loadStatus()}
  };
  load();
}

/* ── 设置 · 检索日志（训练/复盘材料） ── */
async function renderQuerylog(){
  $('pane').innerHTML=`<div class="grid cols-4" id="qlStats"><div class="stat"><div class="label">读取中</div><div class="value">—</div></div></div>
    <section class="panel"><div class="panel-head"><div><h2>最近查询</h2><div class="hint">点查询词复跑，点命中标题看正文</div></div>
      <button class="btn sm" id="qlRefresh">刷新</button></div><div id="qlRecent" class="muted">读取中…</div></section>
    <section class="panel"><div class="panel-head"><div><h2>高命中</h2><div class="hint">被召回次数 top（recall_count，按全量日志聚合）</div></div></div>
      <div id="qlTop"></div></section>
    <section class="panel"><div class="panel-head"><h2>导出训练集</h2></div>
      <div class="panel-desc">每行一次查询：{ts, query, backend, limit, positive:[ids], hits:[{id, score, title}]}。零命中与回退一并保留（负样本信号）。</div>
      <div class="inline"><code class="mono" style="background:var(--soft);border:1px solid var(--line);border-radius:8px;padding:8px 12px">memlite querylog --export train.jsonl</code>
      <span class="faint">加 --since &lt;unix秒&gt; 截窗；--recent N 看流水</span></div></section>`;
  const load=async()=>{
    try{
      const d=await j('/api/querylog?limit=30&top=10');
      const s=d.stats;
      const sem=s.backend.semantic,bm=s.backend.bm25,tot=sem+bm||1;
      $('qlStats').innerHTML=[
        ['总查询',s.queries_total,'accent'],['平均命中',s.hits_avg+' 条','ok'],['零命中',s.zero_hit,s.zero_hit?'warn':''],
        ['召回过的记忆',s.distinct_recalled,'']
      ].map(([l,v,c])=>`<div class="stat ${c}"><div class="label">${l}</div><div class="value">${v}</div>
        <div class="foot">${s.queries_window?'窗口内 '+s.queries_window+' 次':''}</div></div>`).join('');
      $('qlRecent').innerHTML=d.recent.length?`<table><thead><tr><th>时间</th><th>查询</th><th>档位</th><th>来源</th><th>命中</th><th>耗时</th></tr></thead><tbody>${
        d.recent.map(r=>`<tr><td class="mono">${dt(r.ts)}</td>
          <td><button class="btn sm ghost" data-rq="${esc(r.query)}" style="padding:2px 6px">${esc(r.query)}</button>
            ${r.fallback?'<span class="badge warn" style="margin-left:6px"><i class="dot"></i>回退</span>':''}</td>
          <td><span class="badge ${r.backend==='semantic'?'accent':''}">${r.backend==='semantic'?'语义':'词面'}</span></td>
          <td class="muted">${esc(r.source)}</td><td class="mono">${r.hits.length}</td><td class="mono">${r.ms}ms</td></tr>`).join('')
      }</tbody></table>`:'<div class="empty"><strong>还没有检索日志</strong><div class="muted">跑一次 recall 或网页检索即开始累积</div></div>';
      $('qlRecent').querySelectorAll('[data-rq]').forEach(b=>b.onclick=()=>{go('memory','search');
        setTimeout(()=>{const q=$('q');if(q){q.value=b.dataset.rq;doSearch()}},0)});
      $('qlTop').innerHTML=(s.top&&s.top.length)?`<table><thead><tr><th>次数</th><th>标题</th><th>id</th></tr></thead><tbody>${
        s.top.map(t=>`<tr><td class="mono">${t.count}</td>
          <td><button class="btn sm ghost" data-tid="${esc(t.id)}" style="padding:2px 6px">${esc(t.title||'(无标题)')}</button></td>
          <td class="mono">${esc(t.id.slice(0,8))}</td></tr>`).join('')}</tbody></table>`
        :'<div class="empty"><strong>暂无</strong></div>';
      $('qlTop').querySelectorAll('[data-tid]').forEach(b=>b.onclick=()=>{go('memory','search');
        setTimeout(()=>{SR.sel=b.dataset.tid;openDetail(b.dataset.tid)},0)});
    }catch(e){toast('检索日志加载失败：'+e.message,'bad')}
  };
  $('qlRefresh').onclick=load;load();
}

/* ── 设置 · 维护 ── */
async function renderMaintain(){
  $('pane').innerHTML=`<section class="panel"><div class="panel-head"><div><h2>导出</h2>
      <div class="hint">全库明文 JSON（含 tombstone 前的存活记录）</div></div>
      <button class="btn" id="mtExport">导出 JSON</button></div></section>
    <section class="panel"><div class="panel-head"><div><h2>压实</h2>
      <div class="hint">只留存活记录重写回单文件；多机并发期间勿跑</div></div>
      <button class="btn" id="mtCompact">压实库</button></div></section>
    <section class="panel"><div class="panel-head"><div><h2>回填向量</h2>
      <div class="hint">给缺向量的旧记忆补侧车（按条调嵌入 API，增量只算没有的）</div></div>
      <button class="btn" id="mtEmbed">回填</button></div></section>
    <section class="panel"><div class="panel-head"><h2>配置</h2><button class="btn sm" id="mtCfg">查看</button></div>
      <div id="cfgBox" class="muted">api_key 只读脱敏视图</div></section>`;
  $('mtExport').onclick=()=>window.open('/api/export','_blank');
  $('mtCompact').onclick=async()=>{
    const ok=await confirmSheet({title:'压实库？',lead:'只留存活记录重写单文件，历史版本与墓碑会被清理；先 export 备份。多机并发期间勿跑。',danger:true,label:'压实'});
    if(!ok)return;busy($('mtCompact'),true);
    try{const d=await j('/api/compact',{method:'POST'});toast('压实完成：'+d.lines_before+' 行 → '+d.records+' 条存活','ok');loadStatus()}
    catch(e){toast('压实失败：'+e.message,'bad')}finally{busy($('mtCompact'),false)}
  };
  $('mtEmbed').onclick=async()=>{busy($('mtEmbed'),true,'回填中…');
    try{const d=await j('/api/reembed',{method:'POST'});toast('回填 '+d.embedded+' 条，失败 '+d.failed+'；侧车 '+d.vectors+'/'+d.total,'ok');loadStatus()}
    catch(e){toast('回填失败：'+e.message,'bad')}finally{busy($('mtEmbed'),false)}
  };
  $('mtCfg').onclick=async()=>{try{const d=await j('/api/config');const e=d.embedding;
    $('cfgBox').innerHTML=`<dl class="kv"><dt>api_base</dt><dd class="mono">${esc(e.api_base||'（未配）')}</dd>
      <dt>model</dt><dd class="mono">${esc(e.model||'—')}</dd><dt>api_key</dt><dd class="mono">${esc(e.api_key||'—')}</dd>
      <dt>data_dir</dt><dd class="mono">${esc(e.data_dir||'—')}</dd>
      <dt>项目级</dt><dd class="mono">${esc(d.project_dir||'无')}</dd></dl>`}
  catch(err){toast(err.message,'bad')}};
}

/* ── 设置 · 外观 ── */
function renderAppearance(){
  const cur=document.documentElement.dataset.theme;
  $('pane').innerHTML=`<section class="panel"><div class="panel-head"><div><h2>主题</h2>
      <div class="hint">存 localStorage（memlite.theme），下次打开保持</div></div></div>
    <div class="se-seg" id="thSeg">
      <button class="btn sm ${cur==='light'?'on':''}" data-th="light">浅色</button>
      <button class="btn sm ${cur==='dark'?'on':''}" data-th="dark">深色</button>
    </div></section>
    <section class="panel"><div class="panel-head"><h2>检索缺省</h2></div>
      <div class="muted">结果上限在「记忆 · 检索」页选择；档位「自动」＝语义优先、失败或空回词面 BM25，「词面」＝强制 BM25。</div></section>`;
  $('thSeg').querySelectorAll('[data-th]').forEach(b=>b.onclick=()=>{setTheme(b.dataset.th);renderAppearance()});
}

/* ── 启动 ── */
(async function(){await loadStatus();renderShell();window.__errs=[];window.onerror=e=>window.__errs.push(String(e))})();
</script>
</body></html>
"##;
