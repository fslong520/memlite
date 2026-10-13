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
        ("POST", "/api/remember") => api_remember(data_dir, &body, &mut stream),
        ("POST", "/api/forget") => api_forget(data_dir, &body, &mut stream),
        ("POST", "/api/sync") => api_sync(data_dir, &mut stream),
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
<title>memlite 记忆管理</title>
<style>
:root{--bg:#111418;--card:#1a1f26;--fg:#d8dee6;--dim:#7a8494;--gold:#e8b64c;--line:#2a313b;--err:#e06c6c}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:14px/1.6 system-ui,"Noto Sans CJK SC",sans-serif}
.wrap{max-width:960px;margin:0 auto;padding:20px}
h1{font-size:18px;color:var(--gold);margin:0 0 4px}
#meta{color:var(--dim);font-size:12px;margin-bottom:16px}
.bar{display:flex;gap:8px;margin-bottom:14px}
input,textarea{background:var(--card);border:1px solid var(--line);color:var(--fg);border-radius:6px;padding:8px 10px;font:inherit}
input:focus,textarea:focus{outline:none;border-color:var(--gold)}
#q{flex:1}
button{background:var(--card);border:1px solid var(--line);color:var(--fg);border-radius:6px;padding:8px 14px;cursor:pointer;font:inherit}
button:hover{border-color:var(--gold);color:var(--gold)}
button.gold{background:var(--gold);color:#111;border-color:var(--gold);font-weight:600}
button.small{padding:3px 10px;font-size:12px}
.item{background:var(--card);border:1px solid var(--line);border-radius:8px;padding:12px 14px;margin-bottom:10px;cursor:pointer}
.item:hover{border-color:var(--gold)}
.item .t{font-weight:600;margin-bottom:2px}
.item .m{color:var(--dim);font-size:12px}
.item .c{color:#aeb7c4;font-size:13px;margin-top:6px;white-space:pre-wrap;max-height:88px;overflow:hidden}
#detail{display:none;background:var(--card);border:1px solid var(--gold);border-radius:8px;padding:16px;margin-bottom:14px}
#detail pre{white-space:pre-wrap;font:inherit;margin:8px 0}
#addbox{display:none;background:var(--card);border:1px solid var(--line);border-radius:8px;padding:14px;margin-bottom:14px}
#addbox textarea{width:100%;min-height:90px;margin-bottom:8px;resize:vertical}
#status{font-size:13px;min-height:20px;margin:8px 0}
.ok{color:var(--gold)}.err{color:var(--err)}.dim{color:var(--dim)}
.muted{color:var(--dim);font-size:12px}
</style></head><body><div class="wrap">
<h1>memlite 记忆管理</h1>
<div id="meta">加载中…</div>
<div id="status"></div>
<div class="bar">
  <input id="q" placeholder="搜索记忆（语义检索，项目级空自动回退用户级）">
  <button class="gold" onclick="doSearch()">搜索</button>
  <button onclick="toggleAdd()">＋新增</button>
  <button onclick="doSync()">同步到 Git</button>
</div>
<div id="addbox">
  <input id="nt" placeholder="标题（留空自动截正文首句）" style="width:100%;margin-bottom:8px">
  <textarea id="nc" placeholder="记忆正文（口令/证件号等敏感信息会拒入）"></textarea>
  <div class="bar" style="margin:0"><input id="ntags" placeholder="标签，逗号分隔"><button class="gold" onclick="doAdd()">保存</button></div>
</div>
<div id="detail"></div>
<div id="list"></div>
</div>
<script>
const $=id=>document.getElementById(id);
function say(t,cls){$('status').innerHTML='<span class="'+(cls||'dim')+'">'+t+'</span>'}
async function j(url,opts){const r=await fetch(url,opts);const d=await r.json();if(!r.ok)throw Error(d.error||r.status);return d}
async function load(){try{const s=await j('/api/status');
  $('meta').textContent='数据目录 '+s.data_dir+'　·　'+s.records+' 条记忆　·　向量 '+s.vectors+(s.project_dir?'　·　项目级':'　·　用户级')+(s.embedding.configured?'　·　语义检索':'　·　词面检索');
  if(s.bad_lines) say('警告：'+s.bad_lines+' 行损坏','err');
  await doSearch('');}catch(e){say('加载失败：'+e.message,'err')}}
async function doSearch(preset){const q=preset!==undefined?preset:$('q').value.trim();
  if(!q){const d=await j('/api/search?q='+encodeURIComponent(' ')+'&limit=0').catch(()=>null);
    // 空查询列最近：用 limit 大、q 用空不行——直接拿 status 之外用 search 空兜底，改为拉 show 不适用；简化：空查询提示
    $('list').innerHTML='<div class="muted">输入关键词搜索；或点「同步」旁新增。</div>';return}
  try{const d=await j('/api/search?q='+encodeURIComponent(q)+'&limit=30');
    say((d.backend==='semantic'?'语义':'词面')+'检索 '+d.hits.length+' 条'+(d.fallback?'（项目级无命中已回退用户级）':''));
    $('list').innerHTML=d.hits.map(h=>'<div class="item" onclick="show(\''+h.id+'\')"><div class="t">'+esc(h.title)+'</div><div class="m">'+new Date(h.ts*1000).toLocaleString()+'　分数 '+h.score+(h.tags&&h.tags.length?'　#'+h.tags.join(' #'):'')+'</div><div class="c">'+esc((h.content||'').slice(0,160))+'</div></div>').join('')||'<div class="muted">无命中</div>';
  }catch(e){say('搜索失败：'+e.message,'err')}}
async function show(id){try{const r=await j('/api/show?id='+encodeURIComponent(id));
  $('detail').style.display='block';
  $('detail').innerHTML='<div class="t" style="font-weight:600">'+esc(r.title)+'</div><div class="muted">'+new Date(r.ts*1000).toLocaleString()+'　id: '+r.id+(r.project?'　project: '+esc(r.project):'')+'</div><pre>'+esc(r.content)+'</pre><button class="small" onclick="del(\''+r.id+'\')">删除这条</button> <button class="small" onclick="$(\'detail\').style.display=\'none\'">关闭</button>';
  $('detail').scrollIntoView({behavior:'smooth',block:'center'});
}catch(e){say(e.message,'err')}}
async function del(id){if(!confirm('确认删除？'))return;try{await j('/api/forget',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({id})});
  say('已删除','ok');$('detail').style.display='none';doSearch();}catch(e){say('删除失败：'+e.message,'err')}}
function toggleAdd(){const b=$('addbox');b.style.display=b.style.display==='none'?'block':'none'}
async function doAdd(){const content=$('nc').value.trim();if(!content){say('正文不能为空','err');return}
  try{const d=await j('/api/remember',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({content,title:$('nt').value.trim(),tags:$('ntags').value.split(',').map(s=>s.trim()).filter(Boolean)})});
    say('已存 '+d.id+'「'+d.title+'」','ok');$('nc').value='';$('nt').value='';$('ntags').value='';$('addbox').style.display='none';doSearch();
  }catch(e){say(e.message,'err')}}
async function doSync(){say('同步中…');try{const d=await j('/api/sync',{method:'POST'});
  say((d.verified?'同步成功':'核验异常')+'　commit '+(d.committed?'已提交（本地无变更则跳过）':'无新变更')+'　'+d.local_sha.slice(0,8),'ok');
}catch(e){say('同步失败：'+e.message,'err')}}
function esc(s){return (s||'').replace(/[&<>"]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]))}
$('q').addEventListener('keydown',e=>{if(e.key==='Enter')doSearch()});
load();
</script></body></html>
"#;
