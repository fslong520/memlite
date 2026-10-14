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
use crate::sensitive;
use crate::store::Store;
use crate::sync as syncmod;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

pub fn serve(data_dir: std::path::PathBuf, port: u16) -> Result<(), String> {
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr).map_err(|e| format!("绑定 {addr} 失败: {e}"))?;
    println!("memlite web: http://{addr}/  （数据目录 {}）", data_dir.display());
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let dir = data_dir.clone();
                std::thread::spawn(move || {
                    let _ = handle(s, &dir);
                });
            }
            Err(e) => eprintln!("accept 失败: {e}"),
        }
    }
    Ok(())
}

fn handle(mut stream: TcpStream, data_dir: &std::path::Path) -> Result<(), String> {
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
        ("GET", "/api/status") => api_status(data_dir, &mut stream),
        ("GET", "/api/search") => {
            let q = url_param(&path, "q").unwrap_or_default();
            let limit: usize = url_param(&path, "limit")
                .and_then(|v| v.parse().ok())
                .unwrap_or(20);
            api_search(data_dir, &q, limit, &mut stream)
        }
        ("GET", "/api/show") => {
            let id = url_param(&path, "id").unwrap_or_default();
            api_show(data_dir, &id, &mut stream)
        }
        ("GET", "/api/list") => {
            let limit: usize = url_param(&path, "limit").and_then(|v| v.parse().ok()).unwrap_or(30);
            let offset: usize = url_param(&path, "offset").and_then(|v| v.parse().ok()).unwrap_or(0);
            api_list(data_dir, limit, offset, &mut stream)
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
    let reason = if code == 200 { "OK" } else if code == 403 { "Forbidden" } else if code == 404 { "Not Found" } else { "Bad Request" };
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

fn api_status(data_dir: &std::path::Path, stream: &mut TcpStream) -> Result<(), String> {
    let s = Store::open(data_dir);
    let vidx = crate::vectors::VectorIndex::open(data_dir);
    let ecfg = crate::embed::embedding_config();
    let files: Vec<serde_json::Value> = s
        .file_stats()
        .into_iter()
        .map(|(f, b)| serde_json::json!({"file": f, "bytes": b}))
        .collect();
    let v = serde_json::json!({
        "data_dir": data_dir.to_string_lossy(),
        "project_dir": config::find_project_dir().map(|p| p.to_string_lossy().into_owned()),
        "records": s.count(),
        "vectors": vidx.count(),
        "bad_lines": s.bad_lines,
        "files": files,
        "embedding": {"configured": ecfg.is_some()},
    });
    resp(stream, 200, &serde_json::to_string(&v).unwrap())
}

/// 检索：与 CLI recall 同逻辑（语义优先、项目级空自动回退用户级）
fn api_search(data_dir: &std::path::Path, q: &str, limit: usize, stream: &mut TcpStream) -> Result<(), String> {
    if q.trim().is_empty() {
        return resp(stream, 400, r#"{"error":"q 为空"}"#);
    }
    let (backend, hits) = crate::recall_search(data_dir, q, limit, "", false);
    let user_dir = config::user_data_dir();
    let (backend, hits, fallbacked) = if hits.is_empty() && data_dir != user_dir {
        let (b2, h2) = crate::recall_search(&user_dir, q, limit, "", false);
        if !h2.is_empty() { (b2, h2, true) } else { (backend, hits, false) }
    } else {
        (backend, hits, false)
    };
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
    let v = serde_json::json!({"backend": backend, "fallback": if fallbacked {"user"} else {""}, "hits": arr});
    resp(stream, 200, &serde_json::to_string(&v).unwrap())
}

fn api_show(data_dir: &std::path::Path, id: &str, stream: &mut TcpStream) -> Result<(), String> {
    let s = Store::open(data_dir);
    match s.resolve(id) {
        Some(r) => resp(stream, 200, &serde_json::to_string(r).unwrap()),
        None => resp(stream, 404, r#"{"error":"记忆不存在"}"#),
    }
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

/// 单页 UI：检索 + 列表 + 详情 + 新增 + 删除 + 同步
const PAGE: &str = r#"<!DOCTYPE html>
<html lang="zh"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>memlite 记忆</title>
<style>
:root{--bg:#f2f3f5;--panel:#fff;--text:#27292d;--muted:#82858d;--line:#eaecef;--accent:#6856c9;--soft:#f7f8fa;--hover:#eeeef2;--black:#242527;--green:#4c927a;--red:#c05b5b;--shadow:0 5px 22px #21242d05,0 1px 4px #21242d03;
--ui-font:"LXGW Marker Gothic","霞鹜漫黑","SF Pro Text","PingFang SC","Microsoft YaHei",sans-serif;
--read-font:"LXGW Marker Gothic","霞鹜漫黑","Source Han Serif SC","Noto Serif CJK SC","Songti SC",serif}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font:14px/1.7 var(--ui-font);-webkit-font-smoothing:antialiased}
button,input,textarea{font:inherit;color:inherit}button{cursor:pointer;border:0;background:none;transition:background .16s,color .16s}button:active{filter:brightness(.97)}button:disabled{opacity:.4;cursor:default}
input,textarea{outline:none}::selection{background:#ded7fb}::-webkit-scrollbar{width:5px}::-webkit-scrollbar-thumb{background:#c4c5cc60;border-radius:8px}
.app{max-width:880px;margin:0 auto;padding:28px 20px 60px}
header{display:flex;align-items:baseline;gap:12px;margin-bottom:4px}
h1{font-size:20px;font-weight:600;margin:0}
h1 em{font-style:normal;color:var(--accent)}
#meta{color:var(--muted);font-size:13px;margin-bottom:18px}
.panel{background:var(--panel);box-shadow:var(--shadow);border:1px solid var(--line);border-radius:20px}
.searchbar{display:flex;gap:10px;padding:10px;margin-bottom:12px}
.searchbar input{flex:1;border:1px solid var(--line);border-radius:10px;padding:10px 14px;background:var(--soft)}
.searchbar input:focus{border-color:var(--accent);background:var(--panel)}
.b{display:inline-flex;align-items:center;gap:6px;border-radius:10px;padding:10px 17px;background:var(--panel);border:1px solid var(--line);font-weight:500;white-space:nowrap}
.b:hover{background:var(--hover)}
.b.primary{color:#fff;background:var(--black);border-color:var(--black)}
.b.primary:hover{background:#3d3d43}
.b.small{padding:5px 12px;font-size:13px;border-radius:8px}
.tools{display:flex;gap:8px;padding:0 10px 10px;flex-wrap:wrap}
.tools .b{color:var(--muted);font-weight:400}
.box{margin-bottom:12px;padding:16px 18px}
.box textarea{width:100%;min-height:100px;margin:8px 0;border:1px solid var(--line);border-radius:10px;padding:10px 14px;background:var(--soft);resize:vertical;font-family:var(--read-font);line-height:1.9}
.box input{width:100%;border:1px solid var(--line);border-radius:10px;padding:10px 14px;background:var(--soft);margin-bottom:4px}
.box textarea:focus,.box input:focus{border-color:var(--accent);background:var(--panel)}
#status{font-size:13px;min-height:22px;margin:10px 2px;color:var(--muted)}
#status.ok{color:var(--accent)}#status.err{color:var(--red)}
.item{display:block;width:100%;text-align:left;padding:16px 20px;border-bottom:1px solid var(--line)}
.item:last-child{border-bottom:0}.item:hover{background:var(--soft)}
.item .t{font-weight:600;font-size:15px;margin-bottom:3px}
.item .m{color:var(--muted);font-size:12.5px;display:flex;gap:10px;flex-wrap:wrap}
.item .c{color:#5a5e66;font-size:13.5px;margin-top:6px;white-space:pre-wrap;max-height:84px;overflow:hidden;font-family:var(--read-font);line-height:1.9}
.tag{display:inline-block;background:var(--soft);border:1px solid var(--line);border-radius:6px;padding:0 7px;font-size:12px;color:var(--muted)}
.score{color:var(--accent);font-weight:500}
#detail{display:none;margin-bottom:12px;padding:20px 24px;border:1px solid var(--accent)}
#detail .dt{font-size:16px;font-weight:600}
#detail pre{white-space:pre-wrap;font:inherit;font-family:var(--read-font);line-height:2.05;margin:10px 0;font-size:14.5px}
#detail .ops{display:flex;gap:8px;margin-top:8px}
.dot{display:inline-block;width:6px;height:6px;border-radius:50%;background:#58a98b;box-shadow:0 0 0 3px #58a98b10;margin-right:4px;vertical-align:1px}
nav{display:flex;gap:2px;margin-bottom:14px;border-bottom:1px solid var(--line)}
nav button{padding:9px 18px;font-size:14px;font-weight:500;color:var(--muted);border-bottom:2px solid transparent;margin-bottom:-1px}
nav button:hover{color:var(--text);background:var(--hover)}
nav button.on{color:var(--accent);border-bottom-color:var(--accent)}
.chip{display:inline-flex;align-items:center;gap:5px;background:var(--panel);border:1px solid var(--line);border-radius:8px;padding:6px 12px;margin:0 6px 8px 0;cursor:pointer;font-size:13px}
.chip:hover{border-color:var(--accent);color:var(--accent)}
.chip .n{color:var(--muted);font-size:12px}
.tagcloud{padding:16px 20px}
.ver{border-left:3px solid var(--line);padding:6px 0 6px 14px;margin:10px 0}
.ver.cur{border-left-color:var(--accent)}
.ver .vh{font-size:12.5px;color:var(--muted)}
.ver .vc{font-family:var(--read-font);line-height:1.9;margin-top:4px;white-space:pre-wrap}
#more-wrap{text-align:center;margin:16px 0}
.muted{color:var(--muted)}
</style></head><body><div class="app">
<header><h1><em>mem</em>lite 记忆</h1></header>
<nav>
  <button id="tab-mem" class="on" onclick="tab('mem')">记忆</button>
  <button id="tab-time" onclick="tab('time')">时间线</button>
  <button id="tab-tags" onclick="tab('tags')">标签 · 项目</button>
  <button id="tab-inject" onclick="tab('inject')">注入</button>
</nav>
<div id="meta">加载中…</div>
<div id="status"></div>
<div class="panel searchbar">
  <input id="q" placeholder="搜索记忆（留空＝列最近）">
  <button class="b primary" onclick="doSearch()">搜索</button>
  <button class="b" onclick="toggleAdd()">＋ 新增</button>
  <button class="b" onclick="doSync()">同步</button>
</div>
<div class="tools">
  <button class="b small" onclick="doExport()">导出 JSON</button>
  <button class="b small" onclick="doCompact()">压实库</button>
  <button class="b small" onclick="doReembed()">回填向量</button>
  <button class="b small" onclick="showConfig()">配置</button>
</div>
<div id="addbox" class="panel box" style="display:none">
  <input id="nt" placeholder="标题（留空自动截正文首句）">
  <textarea id="nc" placeholder="记忆正文（口令/证件号等敏感信息会拒入）"></textarea>
  <div style="display:flex;gap:10px"><input id="ntags" placeholder="标签，逗号分隔" style="flex:1;margin:0"><button class="b primary" onclick="doAdd()">保存</button></div>
</div>
<div id="editbox" class="panel box" style="display:none;border-color:var(--accent)">
  <input id="et" placeholder="标题">
  <textarea id="ec" placeholder="正文" style="min-height:130px"></textarea>
  <div style="display:flex;gap:10px"><input id="etags" placeholder="标签，逗号分隔" style="flex:1;margin:0"><button class="b primary" onclick="doEdit()">保存修改</button><button class="b" onclick="$('editbox').style.display='none'">取消</button></div>
</div>
<div id="view-mem">
  <div id="detail"></div>
  <div id="list" class="panel"></div>
  <div id="more-wrap"><button id="more" class="b" style="display:none" onclick="loadMore()">加载更多</button></div>
</div>
<div id="view-time" style="display:none"><div id="timeline"></div></div>
<div id="view-tags" style="display:none"><div id="tagview" class="panel tagcloud">加载中…</div></div>
<div id="view-inject" style="display:none"><div id="injectview" class="panel tagcloud">加载中…</div></div>
</div>
<script>
const $=id=>document.getElementById(id);
function say(t,cls){$('status').textContent=t;$('status').className=cls||''}
async function j(url,opts){const r=await fetch(url,opts);const d=await r.json();if(!r.ok)throw Error(d.error||r.status);return d}
let MODE='list',OFFSET=0,LIMIT=30,EDIT_ID=null;
function dt(ts){const d=new Date(ts*1000);const p=n=>String(n).padStart(2,'0');return d.getFullYear()+'-'+p(d.getMonth()+1)+'-'+p(d.getDate())+' '+p(d.getHours())+':'+p(d.getMinutes())}
async function load(){try{const s=await j('/api/status');
  $('meta').innerHTML='<span class="dot"></span>'+(s.project_dir?'项目级':'用户级')+'　'+s.records+' 条记忆　向量 '+s.vectors+'　'+(s.embedding.configured?'语义检索':'词面检索')+'　<span class="muted">'+s.data_dir+'</span>';
  if(s.bad_lines) say('警告：'+s.bad_lines+' 行损坏','err');
  await doSearch('');}catch(e){say('加载失败：'+e.message,'err')}}
function itemHtml(r,score){return '<button class="item" onclick="show(\''+r.id+'\')"><div class="t">'+esc(r.title||'(无标题)')+'</div><div class="m"><span>'+dt(r.ts)+'</span>'+(score!==undefined?'<span class="score">'+score.toFixed(3)+'</span>':'')+((r.tags||[]).map(t=>'<span class="tag">'+esc(t)+'</span>').join(''))+(r.project?'<span class="muted">'+esc(r.project)+'</span>':'')+'</div><div class="c">'+esc((r.content||'').slice(0,170))+'</div></button>'}
async function doSearch(preset){const q=preset!==undefined?preset:$('q').value.trim();
  if(!q){MODE='list';OFFSET=0;
    try{const d=await j('/api/list?limit='+LIMIT+'&offset=0');
      say('最近 '+d.items.length+' 条 / 共 '+d.total);
      $('list').innerHTML=d.items.map(r=>itemHtml(r)).join('')||'<div class="item muted">库是空的</div>';
      $('more').style.display=d.total>OFFSET+LIMIT?'inline-flex':'none';
    }catch(e){say('列取失败：'+e.message,'err')}return}
  MODE='search';OFFSET=0;
  try{const d=await j('/api/search?q='+encodeURIComponent(q)+'&limit=30');
    say((d.backend==='semantic'?'语义':'词面')+'检索 '+d.hits.length+' 条'+(d.fallback?'（项目级无命中已回退用户级）':''),'ok');
    $('list').innerHTML=d.hits.map(h=>itemHtml(h,h.score)).join('')||'<div class="item muted">无命中</div>';
    $('more').style.display='none';
  }catch(e){say('搜索失败：'+e.message,'err')}}
async function loadMore(){OFFSET+=LIMIT;
  try{const d=await j('/api/list?limit='+LIMIT+'&offset='+OFFSET);
    $('list').insertAdjacentHTML('beforeend',d.items.map(r=>itemHtml(r)).join(''));
    say('已加载 '+(OFFSET+d.items.length)+' / '+d.total);
    if(OFFSET+d.items.length>=d.total)$('more').style.display='none';
  }catch(e){say(e.message,'err')}}
async function show(id){try{const r=await j('/api/show?id='+encodeURIComponent(id));
  $('detail').style.display='block';
  $('detail').className='panel';
  $('detail').innerHTML='<div class="dt">'+esc(r.title||'(无标题)')+'</div><div class="muted" style="font-size:12.5px;margin-top:2px">'+dt(r.ts)+'　id: '+r.id+(r.project?'　project: '+esc(r.project):'')+'</div><pre>'+esc(r.content)+'</pre><div class="ops"><button class="b small" onclick="showVersions(\''+r.id+'\')">版本历史</button><button class="b small" onclick="edit(\''+r.id+'\')">编辑</button><button class="b small" onclick="del(\''+r.id+'\')">删除</button><button class="b small" onclick="$(\'detail\').style.display=\'none\'">关闭</button></div>';
  $('detail').scrollIntoView({behavior:'smooth',block:'nearest'});
}catch(e){say(e.message,'err')}}
async function edit(id){try{const r=await j('/api/show?id='+encodeURIComponent(id));
  EDIT_ID=r.id;$('et').value=r.title;$('ec').value=r.content;$('etags').value=(r.tags||[]).join(',');
  $('editbox').style.display='block';$('editbox').scrollIntoView({behavior:'smooth',block:'center'});
}catch(e){say(e.message,'err')}}
async function doEdit(){if(!EDIT_ID)return;try{
  const d=await j('/api/update',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({id:EDIT_ID,title:$('et').value,content:$('ec').value,tags:$('etags').value.split(',').map(s=>s.trim()).filter(Boolean)})});
  say('已更新 '+d.id,'ok');$('editbox').style.display='none';$('detail').style.display='none';doSearch();
}catch(e){say('更新失败：'+e.message,'err')}}
async function del(id){if(!confirm('确认删除？'))return;try{await j('/api/forget',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({id})});
  say('已删除','ok');$('detail').style.display='none';doSearch();}catch(e){say('删除失败：'+e.message,'err')}}
function toggleAdd(){const b=$('addbox');b.style.display=b.style.display==='none'?'block':'none'}
async function doAdd(){const content=$('nc').value.trim();if(!content){say('正文不能为空','err');return}
  try{const d=await j('/api/remember',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({content,title:$('nt').value.trim(),tags:$('ntags').value.split(',').map(s=>s.trim()).filter(Boolean)})});
    say('已存 '+d.id+'「'+d.title+'」','ok');$('nc').value='';$('nt').value='';$('ntags').value='';$('addbox').style.display='none';doSearch();
  }catch(e){say(e.message,'err')}}
async function doSync(){say('同步中…');try{const d=await j('/api/sync',{method:'POST'});
  say((d.verified?'同步成功':'核验异常')+'　'+(d.committed?'已提交':'无新变更')+'　'+d.local_sha.slice(0,8),'ok');
}catch(e){say('同步失败：'+e.message,'err')}}
function doExport(){window.open('/api/export','_blank')}
async function doCompact(){if(!confirm('压实库：只留存活记录重写单文件？多机并发期间勿跑。'))return;say('压实中…');
  try{const d=await j('/api/compact',{method:'POST'});say('压实完成：'+d.lines_before+' 行 → '+d.records+' 条存活','ok');doSearch();
  }catch(e){say('压实失败：'+e.message,'err')}}
async function doReembed(){say('回填向量中…（按条调 API，稍候）');
  try{const d=await j('/api/reembed',{method:'POST'});say('回填 '+d.embedded+' 条，失败 '+d.failed+'，侧车 '+d.vectors+' / 共 '+d.total,'ok');load();
  }catch(e){say('回填失败：'+e.message,'err')}}
async function showConfig(){try{const d=await j('/api/config');
  const e=d.embedding;say('api_base: '+(e.api_base||'（未配）')+'　model: '+(e.model||'—')+'　api_key: '+(e.api_key||'—')+'　项目级: '+(d.project_dir||'无'));
}catch(e){say('读配置失败：'+e.message,'err')}}
function esc(s){return (s||'').replace(/[&<>"]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]))}
$('q').addEventListener('keydown',e=>{if(e.key==='Enter')doSearch()});
let CURTAB='mem';
function tab(t){CURTAB=t;
  for(const x of ['mem','time','tags','inject']){
    $('tab-'+x).className=x===t?'on':'';
    $('view-'+x).style.display=x===t?'':'none'}
  if(t==='time')loadTimeline();
  if(t==='tags')loadTags();
  if(t==='inject')loadInject();
  if(t==='mem')say('')}
async function loadTimeline(){try{const d=await j('/api/list?limit=300&offset=0');
  const byDay={};for(const r of d.items){const k=new Date(r.ts*1000).toISOString().slice(0,10);(byDay[k]=byDay[k]||[]).push(r)}
  const days=Object.keys(byDay).sort().reverse();
  $('timeline').innerHTML=days.map(day=>{
    const rows=byDay[day].map(r=>'<button class="item" style="border-bottom:1px solid var(--line)" onclick="tab(\'mem\');show(\''+r.id+'\')"><div class="t">'+esc(r.title||'(无标题)')+'</div><div class="m"><span>'+dt(r.ts).slice(11)+'</span>'+((r.tags||[]).map(t=>'<span class="tag">'+esc(t)+'</span>').join(''))+'</div></button>').join('');
    return '<div class="panel" style="margin-bottom:12px"><div style="padding:12px 20px 0;font-weight:600;color:var(--accent)">'+day+' <span class="muted" style="font-weight:400">· '+byDay[day].length+' 条</span></div>'+rows+'</div>';
  }).join('')||'<div class="panel muted" style="padding:20px">无记录</div>';
  say('时间线：近 '+d.items.length+' 条 / 共 '+d.total+'（更早的用记忆页搜索）');
}catch(e){say('时间线加载失败：'+e.message,'err')}}
async function loadTags(){try{const [t,p]=await Promise.all([j('/api/tags'),j('/api/projects')]);
  $('tagview').innerHTML='<div style="font-weight:600;margin-bottom:8px">标签</div>'+
    (t.tags.map(x=>'<span class="chip" onclick="tab(\'mem\');$(\'q\').value=\''+esc(x.tag)+'\';doSearch()">#'+esc(x.tag)+' <span class="n">'+x.count+'</span></span>').join('')||'<span class="muted">无标签</span>')+
    '<div style="font-weight:600;margin:14px 0 8px">项目</div>'+
    (p.projects.map(x=>'<span class="chip" onclick="tab(\'mem\');$(\'q\').value=\''+esc(x.project)+'\';doSearch()">'+esc(x.project)+' <span class="n">'+x.count+'</span></span>').join(''));
}catch(e){say('标签加载失败：'+e.message,'err')}}
async function loadInject(){try{const d=await j('/api/inject');
  $('injectview').innerHTML='<div style="font-weight:600;margin-bottom:10px">软件注入点探测</div><div class="muted" style="margin-bottom:10px">正文取 ~/.qoder/rules/AGENTS.md 全文（读不到即拒注），写入各 AI 软件的 rules/AGENTS.md 载体；已有 respire/1memory 规则的会拒绝双注入。</div>'+
    d.map(x=>{const st=x.state==='fresh'?'<span style="color:var(--green)">已注入</span>':x.state==='conflict'?'<span style="color:var(--red)">他系已占</span>':x.state==='stale'?'<span style="color:var(--accent)">待更新</span>':'未注入';
    const ops=x.state==='fresh'||x.state==='stale'?'<button class="b small" onclick="injectDo(\''+x.target+'\',true)">移除</button> <button class="b small" onclick="injectDo(\''+x.target+'\',false)">更新</button>':'<button class="b small" onclick="injectDo(\''+x.target+'\',false)">注入</button>';
    return '<div style="display:flex;justify-content:space-between;align-items:center;padding:10px 0;border-bottom:1px solid var(--line)"><div><div style="font-weight:500">'+x.target+'</div><div class="muted" style="fontSize:12px">'+esc(x.path)+'</div></div><div style="display:flex;gap:8px;align-items:center">'+st+ops+'</div></div>'}).join('');
}catch(e){say('注入探测失败：'+e.message,'err')}}
async function injectDo(target,remove){if(!remove&&!confirm('向 '+target+' 注入 memlite 规则？'))return;if(remove&&!confirm('移除 '+target+' 的 memlite 规则块？'))return;
  try{const d=await j('/api/inject',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({target,remove})});
    if(d.error){say('失败：'+d.error,'err');return}
    say((remove?'已移除 ':'已处理 ')+target+(d.hint?'（'+d.hint+'）':''),'ok');loadInject();
  }catch(e){say(e.message,'err')}}
async function showVersions(id){try{const d=await j('/api/versions?id='+encodeURIComponent(id));
  $('detail').style.display='block';$('detail').className='panel';
  $('detail').innerHTML='<div style="font-weight:600">版本历史 · '+d.versions.length+' 版</div><div class="muted" style="font-size:12.5px">append-only 日志天然保历史；最新版在最后</div>'+
    d.versions.map((v,i)=>'<div class="ver'+(i===d.versions.length-1?' cur':'')+'"><div class="vh">'+dt(v.ts)+(i===d.versions.length-1?'　（当前）':'')+'</div><div class="vc">'+esc((v.content||'').slice(0,300))+'</div></div>').join('')+
    '<button class="b small" onclick="show(\''+id+'\')">返回详情</button>';
  $('detail').scrollIntoView({behavior:'smooth',block:'center'});
}catch(e){say(e.message,'err')}}
load();</script></body></html>
"#;
