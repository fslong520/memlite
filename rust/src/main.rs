//! memlite CLI——简化嵌入式记忆系统（Rust 版）。
//! 命令面：init/remember/recall/show/list/forget/attach/demote/promote/chain/tree/sync/inject/reembed/status/compact/bench/web/embed-server。

mod bench;
mod config;
mod embed;
mod embed_server;
mod inject;
mod querylog;
mod search;
mod sensitive;
mod store;
mod sync;
mod tree;
mod vectors;
mod web;

use clap::{Parser, Subcommand};
use store::Store;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "memlite", about = "简化嵌入式记忆系统", version)]
struct Cli {
    /// 数据目录（默认取配置）
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    /// JSON 输出
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 初始化数据目录与配置
    Init,
    /// 存记忆
    Remember {
        content: String,
        #[arg(long, default_value = "")]
        title: String,
        #[arg(long, default_value = "")]
        tags: String,
        #[arg(long, default_value = "")]
        project: String,
        /// 认因挂载：本条之因（父）——id/前8位/纲名皆可，显式挂载即意图（不判存）
        #[arg(long, default_value = "")]
        parent: String,
    },
    /// 检索（配了嵌入 API 走语义，否则词面 BM25）
    Recall {
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long, default_value = "")]
        project: String,
        /// 强制词面档
        #[arg(long)]
        bm25: bool,
    },
    /// 看全文
    Show { id: String },
    /// 列最近
    List {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// 删记忆
    Forget { id: String },
    /// 收编挂链：把子条挂到 --parent 之下（显式挂载即意图，不判存；父子即因果）
    Attach {
        id: String,
        #[arg(long)]
        parent: String,
    },
    /// 因果下沉：本条挂到 --parent 下成为其果（因果链延一层，防环）
    Demote {
        id: String,
        #[arg(long)]
        parent: String,
    },
    /// 因果上浮：本条升为更上游之因（挂到祖父/根，果链随之）
    Promote { id: String },
    /// 因果链深挖：一把取回整条链——因链全文（根在前）+ 本条 + 果链下探 --depth 层
    Chain {
        id: String,
        /// 果链下探层数（缺省 3；0 = 只出因链与本条）
        #[arg(long, default_value_t = 3)]
        depth: usize,
    },
    /// 因果树：渲染森林或子树（--from 起点）；--outline 输出全深 id+标题 目录
    Tree {
        #[arg(long, default_value = "")]
        from: String,
        #[arg(long, default_value_t = 3)]
        depth: usize,
        #[arg(long)]
        outline: bool,
    },
    /// 就地改一条（同 id 追加新版本，末写胜出；未传的标题/正文/标签保留原值——显式 --title 即改标题）
    Update {
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        content: Option<String>,
        #[arg(long)]
        tags: Option<String>,
    },
    /// git 同步
    Sync {
        #[arg(long)]
        dry_run: bool,
    },
    /// rules 注入（三档：rules 整文件 / memory 块嵌 / agents 块嵌；正文取 ~/.qoder/rules/AGENTS.md 全文，读不到即拒注）
    Inject {
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        probe: bool,
        /// 打印块嵌正文（BEGIN/END 外壳＋注入源全文）到 stdout，不写任何文件——手动粘到任意文件用
        #[arg(long)]
        print: bool,
        #[arg(long)]
        remove: Option<String>,
        #[arg(long)]
        force: bool,
        /// 生成独立系统级 rules 文件并打印逐产品拷贝引导（可带输出路径，缺省 memlite-rules.md）
        #[arg(long, num_args = 0..=1, default_missing_value = "memlite-rules.md", value_name = "FILE")]
        emit: Option<String>,
    },
    /// 给无向量的记忆回填嵌入（须配 API）
    Reembed,
    /// 状态
    Status,
    /// 压实（只留存活重写回单文件）
    Compact,
    /// 检索命中率评测：20 条真实形态记忆 × 20 改写型查询，报 hit@1/hit@3（临时库）
    Bench {
        /// 语义档：须配嵌入 API，未配置或嵌入失败退出码 2
        #[arg(long)]
        semantic: bool,
    },
    /// 检索日志：查询流水＋命中次数（训练/复盘材料；写于 recall 与 web 检索）
    Querylog {
        /// 打印最近 N 条流水
        #[arg(long, default_value_t = 0)]
        recent: usize,
        /// 导出训练集 JSONL 到文件（每行一次查询：query/backend/positive/hits）
        #[arg(long)]
        export: Option<PathBuf>,
        /// 只导出该时间之后的（unix 秒）
        #[arg(long, default_value_t = 0.0)]
        since: f64,
        /// 汇总里点名的高命中条数
        #[arg(long, default_value_t = 10)]
        top: usize,
    },
    /// 内置 Web 管理界面（本地回环）
    Web {
        #[arg(long, default_value_t = 7879)]
        port: u16,
    },
    /// 本地嵌入服务：bge-m3 ONNX 量化版，OpenAI 兼容 /v1/embeddings（缺省只绑 127.0.0.1）
    EmbedServer {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 18321)]
        port: u16,
        /// 模型目录（缺省三级回退：MEMLITE_MODEL_DIR → ~/.rsrs/models/bge-m3 → /usr/lib/respire/models/bge-m3）
        #[arg(long)]
        model_dir: Option<String>,
    },
    /// 查看/写配置（分层：项目级 > 用户级；set 缺省写用户级，--project 写项目级）
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// 打印合并后的配置（api_key 脱敏）
    Show,
    /// 写一项：key 形如 data_dir / embedding.api_base / embedding.api_key / embedding.model
    Set {
        key: String,
        value: String,
        /// 写项目级（须在含 .memlite/ 或 memlite-data/config.json 的工作区内）
        #[arg(long)]
        project: bool,
    },
}

fn data_dir(arg: &Option<PathBuf>) -> PathBuf {
    // 分层解析：CLI > MEMLITE_DATA > 工作区记忆文件夹 > 用户级 config > 默认（config.rs）
    config::resolve_data_dir(arg.as_deref())
}

fn emit(json: bool, value: serde_json::Value, human: &str) {
    if json {
        println!("{}", serde_json::to_string_pretty(&value).unwrap());
    } else {
        println!("{human}");
    }
}

/// id 取前 8 位（人读）
fn short(id: &str) -> &str {
    let n = id.len().min(8);
    &id[..n]
}

/// unix 秒 → 本地时间；date 不可用时原样回数字
fn fmt_ts(ts: f64) -> String {
    let out = std::process::Command::new("date")
        .args(["-d", &format!("@{ts}"), "+%Y-%m-%d %H:%M:%S"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if out.is_empty() {
        format!("{ts}")
    } else {
        out
    }
}

/// 单库检索：语义档优先（API 在配且通），失败或空回词面 BM25。
/// 返回 (backend, hits)。两档回退的上层逻辑见 recall 命令（web 同复用）。
pub(crate) fn recall_search(
    dir: &Path,
    query: &str,
    limit: usize,
    project: &str,
    bm25_only: bool,
) -> (&'static str, Vec<(f64, store::Record)>) {
    let s = Store::open(dir);
    let recs = s.all();
    let mut backend: &'static str = "bm25";
    let mut hits: Vec<(f64, store::Record)> = vec![];
    if !bm25_only {
        if let Some(cfg) = embed::embedding_config() {
            if let Ok(qv) = embed::embed(&cfg, query) {
                let mut vidx = vectors::VectorIndex::open(dir);
                let mut getter = |id: &str| vidx.get(id);
                hits = search::semantic(&recs, &qv, limit, project, &mut getter)
                    .into_iter()
                    .map(|(sc, r)| (sc, r.clone()))
                    .collect();
                backend = "semantic";
            }
        }
    }
    if backend != "semantic" || hits.is_empty() {
        backend = "bm25";
        hits = search::bm25(&recs, query, limit, project)
            .into_iter()
            .map(|(sc, r)| (sc, r.clone()))
            .collect();
    }
    (backend, hits)
}

fn main() {
    let cli = Cli::parse();
    let dir = data_dir(&cli.data_dir);
    let code = run(cli, &dir);
    std::process::exit(code);
}

/// attach/demote 共用：解析两端（id/前8位/纲名）→ 防自挂防环 → 改挂。
/// demote=true 用「下沉」文案（同一 reparent 语义，对齐 respire attach/demote 双命令）。
fn attach_like(dir: &PathBuf, json: bool, id: &str, parent: &str, demote: bool) -> i32 {
    let mut s = Store::open(dir);
    let (child_full, parent_full) = match (s.resolve(id), s.resolve(parent)) {
        (Some(c), Some(p)) => (c.id.clone(), p.id.clone()),
        (None, _) => {
            eprintln!("记忆不存在: {id}");
            return 1;
        }
        (_, None) => {
            eprintln!("目标因（父）不存在: {parent}");
            return 1;
        }
    };
    if let Err(e) = s.reparent_checked(&child_full, &parent_full) {
        eprintln!("{e}");
        return 1;
    }
    if demote {
        emit(
            json,
            serde_json::json!({"id": child_full, "parent": parent_full}),
            &format!(
                "⬇️ 因果下沉：{} 挂到 {} 之下成为其果（下游果链随迁）",
                short(&child_full),
                short(&parent_full)
            ),
        );
    } else {
        emit(
            json,
            serde_json::json!({"child": child_full, "parent": parent_full}),
            &format!(
                "🔗 已挂链：{} → 父 {}「{}」",
                short(&child_full),
                short(&parent_full),
                s.get(&parent_full).map(|p| p.title.as_str()).unwrap_or("")
            ),
        );
    }
    0
}

fn run(cli: Cli, dir: &PathBuf) -> i32 {
    let json = cli.json;
    match cli.cmd {
        Cmd::Init => {
            std::fs::create_dir_all(dir).expect("建目录失败");
            // 显式 --data-dir / MEMLITE_DATA 指来的目录只就地初始化，不写回用户配置——
            // 否则临时库会把 ~/.memlite/config.json 的 data_dir 顶掉、真库被静默绕过
            // （2026-10-14 memprobe、2026-10-15 qlog-sync 两次踩同一坑）
            let explicit = cli.data_dir.is_some() || std::env::var("MEMLITE_DATA").is_ok();
            let mut wrote_cfg = false;
            if !explicit {
                let cfg = std::env::var("MEMLITE_CONFIG")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| {
                        let home = config::home_dir();
                        PathBuf::from(home).join(".memlite/config.json")
                    });
                if let Some(p) = cfg.parent() {
                    let _ = std::fs::create_dir_all(p);
                }
                let mut cfg_v: serde_json::Value = std::fs::read_to_string(&cfg)
                    .ok()
                    .and_then(|t| serde_json::from_str(&t).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                cfg_v["data_dir"] = serde_json::json!(dir.to_string_lossy());
                let _ = std::fs::write(&cfg, serde_json::to_string_pretty(&cfg_v).unwrap());
                wrote_cfg = true;
            }
            if !dir.join(".git").is_dir() {
                let _ = std::process::Command::new("git")
                    .args(["init", "-b", "main"])
                    .arg(dir)
                    .output();
            }
            let s = Store::open(dir);
            emit(
                json,
                serde_json::json!({"data_dir": dir, "records": s.count(), "config_written": wrote_cfg}),
                &format!(
                    "数据目录: {}\n记忆条数: {}{}",
                    dir.display(),
                    s.count(),
                    if wrote_cfg { "" } else { "\n（显式 --data-dir：未改用户配置）" }
                ),
            );
            0
        }

        Cmd::Remember { content, title, tags, project, parent } => {
            // 写前敏感闸：明文口令/身份证/手机号/银行卡/token/私钥默认不入库
            let probe = format!("{title}\n{content}");
            let hits = sensitive::scan(&probe);
            if !hits.is_empty() {
                eprintln!(
                    "拒绝入库：命中敏感信息（{}）。改存位置指针（如「密码在 ~/.env」）后重试。",
                    hits.join("、")
                );
                return 3;
            }
            let title = if title.trim().is_empty() {
                search::default_title(&content)
            } else {
                title.trim().to_string()
            };
            let tag_list: Vec<String> = tags.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
            let mut s = Store::open(dir);
            // 认因：解析全长（id/前8位/纲名皆可），显式挂载不判存
            let parent_full = if parent.is_empty() {
                String::new()
            } else {
                match s.resolve(&parent) {
                    Some(p) => p.id.clone(),
                    None => {
                        eprintln!("目标因（父）不存在: {parent}");
                        return 1;
                    }
                }
            };
            let rid = match s.put_child(&content, &title, &tag_list, &project, &parent_full) {
                Ok(id) => id,
                Err(e) => {
                    eprintln!("{e}");
                    return 1;
                }
            };
            // 配了嵌入 API 顺手写侧车；失败不拦存入
            let mut embedded = false;
            let mut note = String::new();
            if let Some(cfg) = embed::embedding_config() {
                match embed::embed(&cfg, &format!("{title}\n{content}")) {
                    Ok(v) => {
                        let mut vidx = vectors::VectorIndex::open(dir);
                        if let Err(e) = vidx.put(&rid, &v) {
                            note = format!("（侧车写入失败：{e}）");
                        } else {
                            embedded = true;
                        }
                    }
                    Err(e) => note = format!("（嵌入失败，仅词面可查：{e}）"),
                }
            }
            emit(
                json,
                serde_json::json!({
                    "id": rid, "title": title, "embedded": embedded, "records": s.count(),
                    "parent": parent_full,
                }),
                &format!(
                    "已存 {rid}「{title}」{note}（共 {} 条）{}",
                    s.count(),
                    if parent_full.is_empty() {
                        String::new()
                    } else {
                        format!("\n🔗 因果链挂载：本条之因（父）为 {}「{}」", short(&parent_full),
                            s.get(&parent_full).map(|p| p.title.as_str()).unwrap_or(""))
                    }
                ),
            );
            0
        }

        Cmd::Recall { query, limit, project, bm25 } => {
            let t0 = std::time::Instant::now();
            // 两档查序：解析库非用户级（项目级/--data-dir）且没搜到 → 自动回退用户级
            let (mut backend, mut hits) = recall_search(&dir, &query, limit, &project, bm25);
            let user_dir = config::user_data_dir();
            let mut fallbacked = false;
            if hits.is_empty() && *dir != user_dir {
                let (b2, h2) = recall_search(&user_dir, &query, limit, &project, bm25);
                if !h2.is_empty() {
                    backend = b2;
                    hits = h2;
                    fallbacked = true;
                }
            }
            // 落检索日志：一次查询一行（含零命中与回退），失败只提示不拦检索
            // 因果视界：命中沿因链上溯、果链下探（回退库的命中在回退库展示——链在彼库）
            let causal_dir = if fallbacked { user_dir.clone() } else { dir.clone() };
            let entry = querylog::Entry {
                ts: store::now_ts(),
                query: query.clone(),
                backend: backend.to_string(),
                limit,
                fallback: if fallbacked { "user" } else { "" }.into(),
                source: "cli".into(),
                ms: t0.elapsed().as_millis() as u64,
                hits: hits
                    .iter()
                    .map(|(sc, r)| querylog::Hit {
                        id: r.id.clone(),
                        score: *sc,
                        title: r.title.clone(),
                    })
                    .collect(),
            };
            if let Err(e) = querylog::append_line(&dir, &entry) {
                eprintln!("（检索日志写入失败：{e}）");
            }
            if json {
                let cstore = Store::open(&causal_dir);
                let arr: Vec<serde_json::Value> = hits
                    .iter()
                    .map(|(s, r)| {
                        let ancestors: Vec<serde_json::Value> = cstore
                            .ancestor_chain(&r.id)
                            .into_iter()
                            .filter(|pid| cstore.records.contains_key(pid))
                            .map(|pid| {
                                serde_json::json!({
                                    "id": pid,
                                    "title": cstore.records[&pid].title,
                                })
                            })
                            .collect();
                        let children: Vec<serde_json::Value> = cstore
                            .children(&r.id)
                            .iter()
                            .map(|c| {
                                serde_json::json!({"id": c.id, "title": c.title})
                            })
                            .collect();
                        serde_json::json!({
                            "score": (s * 10000.0).round() / 10000.0,
                            "id": r.id, "ts": r.ts, "title": r.title,
                            "content": r.content, "tags": r.tags, "project": r.project,
                            "ancestors": ancestors, "children": children
                        })
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "backend": backend,
                        "fallback": if fallbacked {"user"} else {""},
                        "hits": arr
                    })).unwrap()
                );
            } else {
                eprintln!(
                    "（{backend} 检索{}）",
                    if fallbacked { "，项目级无命中已回退用户级" } else { "" }
                );
                if hits.is_empty() {
                    println!("（无命中）");
                }
                let cstore = Store::open(&causal_dir);
                for (sc, r) in &hits {
                    let head = if r.title.is_empty() {
                        r.content.chars().take(40).collect::<String>()
                    } else {
                        r.title.clone()
                    };
                    let head = head.replace('\n', " ");
                    println!("[{sc:.3}] {}  {head}", r.id);
                    // 因果视界：🔺因链+纲L1+🔻果——查因及果，命中即知链影响
                    for line in tree::causal_lines(&cstore, &r.id) {
                        println!("{line}");
                    }
                }
            }
            0
        }

        Cmd::Querylog { recent, export, since, top } => {
            // 导出走全文件（不吃内存尾窗），其余走常驻视图
            if let Some(out) = export {
                match querylog::export(&dir, &out, since) {
                    Ok(n) => {
                        emit(
                            json,
                            serde_json::json!({"exported": n, "path": out, "since": since}),
                            &format!("已导出 {n} 次查询 → {}", out.display()),
                        );
                        return 0;
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        return 1;
                    }
                }
            }
            let log = querylog::Log::load(&dir);
            if recent > 0 {
                let rows = log.recent_json(recent);
                if json {
                    println!("{}", serde_json::to_string_pretty(&rows).unwrap());
                } else if rows.is_empty() {
                    println!("（检索日志为空——还没有 recall 过）");
                } else {
                    for r in &rows {
                        println!(
                            "{}  {}/{}  命中 {}  {}{}  「{}」",
                            fmt_ts(r["ts"].as_f64().unwrap_or(0.0)),
                            r["backend"].as_str().unwrap_or(""),
                            r["source"].as_str().unwrap_or(""),
                            r["hits"].as_array().map(|a| a.len()).unwrap_or(0),
                            if r["ms"].as_u64().unwrap_or(0) > 0 {
                                format!("{}ms  ", r["ms"].as_u64().unwrap_or(0))
                            } else {
                                String::new()
                            },
                            if r["fallback"].as_str().unwrap_or("").is_empty() {
                                String::new()
                            } else {
                                "回退用户级  ".into()
                            },
                            r["query"].as_str().unwrap_or(""),
                        );
                    }
                }
                return 0;
            }
            let st = log.stats(top);
            if json {
                println!("{}", serde_json::to_string_pretty(&st).unwrap());
                return 0;
            }
            let b = &st["backend"];
            let s = &st["source"];
            println!(
                "检索日志：窗口 {} 次查询（全量 {}，坏行 {}）",
                st["queries_window"], st["queries_total"], st["bad_lines"]
            );
            if st["queries_window"].as_u64().unwrap_or(0) > 0 {
                println!(
                    "  档位 semantic {} · bm25 {}　来源 cli {} · web {}　回退用户级 {}",
                    b["semantic"], b["bm25"], s["cli"], s["web"], st["fallback_user"]
                );
                println!(
                    "  命中 平均 {} 条/次 · 零命中 {} 次 · 被召回过的记忆 {} 条",
                    st["hits_avg"], st["zero_hit"], st["distinct_recalled"]
                );
                if let Some(top) = st["top"].as_array() {
                    if !top.is_empty() {
                        println!("  高命中：");
                        for t in top {
                            let title = t["title"].as_str().unwrap_or("");
                            let title = if title.is_empty() { "(无标题)" } else { title };
                            println!("    {:>4}  {}  {}", t["count"], short(t["id"].as_str().unwrap_or("")), title);
                        }
                    }
                }
            }
            0
        }

        Cmd::Show { id } => {
            let s = Store::open(dir);
            let Some(r) = s.resolve(&id) else {
                eprintln!("记忆不存在: {id}");
                return 1;
            };
            let rid = r.id.clone();
            if json {
                // 原样输出该条 JSON + 因果邻居（对齐 respire show --json：entry/ancestors/children）
                let mut v = serde_json::to_value(r).unwrap();
                let title_of = |pid: &str| -> String {
                    s.records
                        .get(pid)
                        .map(|p| if p.title.is_empty() { short(pid).to_string() } else { p.title.clone() })
                        .unwrap_or_else(|| short(pid).to_string())
                };
                let ancestors: Vec<serde_json::Value> = s
                    .ancestor_chain(&rid)
                    .into_iter()
                    .filter(|pid| s.records.contains_key(pid))
                    .map(|pid| serde_json::json!({"id": pid, "title": title_of(&pid)}))
                    .collect();
                let children: Vec<serde_json::Value> = s
                    .children(&rid)
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "id": c.id,
                            "title": c.title,
                            "summary": c.content.chars().take(80).collect::<String>(),
                        })
                    })
                    .collect();
                v["ancestors"] = serde_json::json!(ancestors);
                v["children"] = serde_json::json!(children);
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            } else {
                println!(
                    "id: {}\ncreated: {}\ntitle: {}\ntags: {}\nproject: {}\n---\n{}",
                    r.id, r.ts, r.title, r.tags.join(", "), r.project, r.content
                );
                for line in tree::causal_lines(&s, &rid) {
                    println!("{line}");
                }
            }
            0
        }

        Cmd::List { limit } => {
            let s = Store::open(dir);
            let mut recs = s.all();
            recs.sort_by(|a, b| b.ts.partial_cmp(&a.ts).unwrap());
            recs.truncate(limit);
            if json {
                let arr: Vec<serde_json::Value> = recs.iter().map(|r| serde_json::to_value(r).unwrap()).collect();
                println!("{}", serde_json::to_string_pretty(&arr).unwrap());
            } else {
                for r in recs {
                    let head = if r.title.is_empty() {
                        r.content.chars().take(40).collect::<String>()
                    } else {
                        r.title.clone()
                    };
                    println!("{}  {}", r.id, head.replace('\n', " "));
                }
            }
            0
        }

        Cmd::Forget { id } => {
            let mut s = Store::open(dir);
            let target = s.resolve(&id).map(|r| r.id.clone());
            let Some(rid) = target else {
                eprintln!("记忆不存在: {id}");
                return 1;
            };
            if let Err(e) = s.delete(&rid) {
                eprintln!("{e}");
                return 1;
            }
            emit(json, serde_json::json!({"deleted": rid}), &format!("已删 {rid}"));
            0
        }

        Cmd::Attach { id, parent } => attach_like(dir, json, &id, &parent, false),
        Cmd::Demote { id, parent } => attach_like(dir, json, &id, &parent, true),

        Cmd::Promote { id } => {
            let mut s = Store::open(dir);
            let Some(me) = s.resolve(&id).map(|r| r.clone()) else {
                eprintln!("记忆不存在: {id}");
                return 1;
            };
            if me.parent_id().is_empty() {
                emit(
                    json,
                    serde_json::json!({"id": me.id, "new_parent": ""}),
                    "已是根（无因）——无可上浮",
                );
                return 0;
            }
            let old_parent = me.parent_id().to_string();
            // 新因 = 原因的因（祖父）；祖父不在库则升根
            let grandparent = s
                .records
                .get(&old_parent)
                .map(|p| p.parent_id().to_string())
                .unwrap_or_default();
            if let Err(e) = s.reparent_checked(&me.id, &grandparent) {
                eprintln!("{e}");
                return 1;
            }
            emit(
                json,
                serde_json::json!({"id": me.id, "new_parent": grandparent}),
                &format!(
                    "⬆️ 因果上浮：{} {}（本条之果随迁，因果层级上移）",
                    short(&me.id),
                    if grandparent.is_empty() {
                        "升为根（自为因）".to_owned()
                    } else {
                        format!("挂到因 {} 下", short(&grandparent))
                    }
                ),
            );
            0
        }

        Cmd::Chain { id, depth } => {
            let s = Store::open(dir);
            let Some(r) = s.resolve(&id) else {
                eprintln!("记忆不存在: {id}");
                return 1;
            };
            let rid = r.id.clone();
            let view = match tree::chain_view(&s, &rid, depth) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("{e}");
                    return 1;
                }
            };
            if json {
                let anc: Vec<serde_json::Value> = view
                    .ancestors
                    .iter()
                    .map(|a| serde_json::json!({"id": a.id, "title": a.title, "content": a.content}))
                    .collect();
                let des: Vec<serde_json::Value> = view
                    .descendants
                    .iter()
                    .map(|(lv, d)| serde_json::json!({"level": lv, "id": d.id, "title": d.title, "content": d.content}))
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "entry": {"id": view.entry.id, "title": view.entry.title, "content": view.entry.content},
                        "ancestors": anc,
                        "descendants": des,
                    }))
                    .unwrap()
                );
                return 0;
            }
            println!("🔗 因果链（自 {}）", short(&rid));
            if view.ancestors.is_empty() {
                println!("   （无因——本条为根/纲）");
            } else {
                println!("   🔺因链（{} 层，根在前）：", view.ancestors.len());
                for a in &view.ancestors {
                    let t = if a.title.is_empty() { short(&a.id).to_string() } else { a.title.clone() };
                    println!("\n   ┌─ [{}] {t}", short(&a.id));
                    for line in a.content.lines() {
                        println!("   │ {line}");
                    }
                }
            }
            {
                let e = view.entry;
                let t = if e.title.is_empty() { short(&e.id).to_string() } else { e.title.clone() };
                println!("\n   ◆ 本条 [{}] {t}", short(&e.id));
                for line in e.content.lines() {
                    println!("   │ {line}");
                }
            }
            if depth == 0 {
                return 0;
            }
            if view.descendants.is_empty() {
                println!("\n   （无果——本条为叶）");
            } else {
                println!("\n   🔻果链（{} 条，下探 {depth} 层）：", view.descendants.len());
                for (lv, d) in &view.descendants {
                    let t = if d.title.is_empty() { short(&d.id).to_string() } else { d.title.clone() };
                    println!("\n   {}┌─ L{lv} [{}] {t}", "  ".repeat(lv - 1), short(&d.id));
                    for line in d.content.lines() {
                        println!("   {}│ {line}", "  ".repeat(lv - 1));
                    }
                }
            }
            0
        }

        Cmd::Tree { from, depth, outline: ol } => {
            let s = Store::open(dir);
            let scope = tree::TreeScope::build(&s);
            // --from 支持 id/前8位/纲名（先全库解析再入作用域）
            let from_full = if from.is_empty() {
                None
            } else {
                match s.resolve(&from) {
                    Some(r) => Some(r.id.clone()),
                    None => {
                        eprintln!("记忆不存在: {from}");
                        return 1;
                    }
                }
            };
            if ol {
                match tree::outline(&scope, from_full.as_deref()) {
                    Ok((lines, n)) => {
                        if json {
                            let rows: Vec<serde_json::Value> = lines
                                .iter()
                                .enumerate()
                                .map(|(i, l)| serde_json::json!({"i": i, "line": l}))
                                .collect();
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&serde_json::json!({"nodes": rows, "count": n})).unwrap()
                            );
                        } else {
                            for l in &lines {
                                println!("{l}");
                            }
                            eprintln!("—— 共 {n} 节点");
                        }
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        return 1;
                    }
                }
                return 0;
            }
            if json {
                match tree::tree_json(&scope, from_full.as_deref()) {
                    Ok(nodes) => {
                        fn to_json(n: &tree::TreeNode) -> serde_json::Value {
                            serde_json::json!({
                                "id": n.id, "title": n.title, "kind": n.kind,
                                "level": n.level, "leaf": n.leaf,
                                "children": n.children.iter().map(to_json).collect::<Vec<_>>(),
                            })
                        }
                        let arr: Vec<serde_json::Value> = nodes.iter().map(to_json).collect();
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&serde_json::json!({"nodes": arr})).unwrap()
                        );
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        return 1;
                    }
                }
                return 0;
            }
            match tree::render_tree(&scope, from_full.as_deref(), depth) {
                Ok(lines) => {
                    for l in &lines {
                        println!("{l}");
                    }
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            }
        }

        Cmd::Update { id, title, content, tags } => {
            let probe = format!(
                "{}\n{}",
                title.clone().unwrap_or_default(),
                content.clone().unwrap_or_default()
            );
            let hits = sensitive::scan(&probe);
            if !hits.is_empty() {
                eprintln!("拒绝更新：命中敏感信息（{}）。", hits.join("、"));
                return 3;
            }
            let mut s = Store::open(dir);
            let target = s.resolve(&id).map(|r| r.clone());
            let Some(old) = target else {
                eprintln!("记忆不存在: {id}");
                return 1;
            };
            let new_title = title.unwrap_or_else(|| old.title.clone());
            let new_content = content.unwrap_or_else(|| old.content.clone());
            let new_tags: Vec<String> = match tags {
                Some(csv) => csv.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
                None => old.tags.clone(),
            };
            if new_content.trim().is_empty() {
                eprintln!("content 不能为空");
                return 1;
            }
            // 同 id 追加新 put——追加日志末写胜出，id/父链语义不变
            if let Err(e) = s.put_with_id(&old.id, &new_content, &new_title, &new_tags, &old.project) {
                eprintln!("{e}");
                return 1;
            }
            // 侧车向量跟着更新（有 API 才算；失败不拦）
            let mut embedded = false;
            if let Some(cfg) = embed::embedding_config() {
                if let Ok(v) = embed::embed(&cfg, &format!("{new_title}\n{new_content}")) {
                    let mut vidx = vectors::VectorIndex::open(dir);
                    embedded = vidx.put(&old.id, &v).is_ok();
                }
            }
            emit(
                json,
                serde_json::json!({"id": old.id, "title": new_title, "embedded": embedded}),
                &format!("已更新 {}", old.id),
            );
            0
        }

        Cmd::Sync { dry_run } => {
            if dry_run {
                let v = sync::status(dir);
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
                return 0;
            }
            match sync::sync(dir) {
                Ok(r) => {
                    let v = serde_json::json!({
                        "committed": r.committed, "local_sha": r.local_sha,
                        "remote_sha": r.remote_sha, "verified": r.verified
                    });
                    emit(json, v.clone(), &serde_json::to_string_pretty(&v).unwrap());
                    0
                }
                Err(e) => {
                    eprintln!("同步失败: {e}");
                    1
                }
            }
        }

        Cmd::Inject { target, all, probe: probe_only, print: print_block, remove, force, emit } => {
            if let Some(out_file) = emit {
                // emit 只生成文件+引导，不改任何他方文件（格式各异，用户按表拷贝）
                if let Err(e) = std::fs::write(&out_file, inject::standalone_text()) {
                    eprintln!("写 {out_file} 失败: {e}");
                    return 1;
                }
                print!("{}", inject::emit_guidance(&out_file));
                return 0;
            }
            if print_block {
                // 手动注入：只出文本不落盘，用户可粘进任意文件并自行修改
                return match inject::rules_text(false) {
                    Ok(b) => {
                        print!("{b}");
                        0
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        1
                    }
                };
            }
            if probe_only {
                let v = inject::probe();
                println!("{}", serde_json::to_string_pretty(&serde_json::json!(v)).unwrap());
                return 0;
            }
            if let Some(t) = remove {
                let v = inject::remove(&t);
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
                return 0;
            }
            let results: Vec<serde_json::Value> = if all {
                inject::TARGETS.iter().filter(|t| t.auto).map(|t| inject::inject(t.name, force, false)).collect()
            } else if let Some(t) = target {
                vec![inject::inject(&t, force, false)]
            } else {
                eprintln!("须指定 --emit [文件] 或 --target <名> 或 --all 或 --probe 或 --print 或 --remove <名>");
                return 2;
            };
            println!("{}", serde_json::to_string_pretty(&serde_json::json!(results)).unwrap());
            0
        }

        Cmd::Reembed => {
            let Some(cfg) = embed::embedding_config() else {
                eprintln!("未配置嵌入 API（config 的 embedding 段或 MEMLITE_API_BASE/KEY/MODEL）");
                return 2;
            };
            let s = Store::open(dir);
            let mut vidx = vectors::VectorIndex::open(dir);
            let (mut done, mut failed) = (0u32, 0u32);
            let mut all = s.all();
            all.sort_by(|a, b| a.ts.partial_cmp(&b.ts).unwrap());
            for r in all {
                if vidx.has(&r.id) {
                    continue;
                }
                let text = format!("{}\n{}", r.title, r.content);
                match embed::embed(&cfg, &text) {
                    Ok(v) => {
                        if vidx.put(&r.id, &v).is_ok() {
                            done += 1;
                        }
                    }
                    Err(e) => {
                        eprintln!("跳过 {}: {e}", r.id);
                        failed += 1;
                    }
                }
            }
            emit(
                json,
                serde_json::json!({"embedded": done, "failed": failed, "vectors": vidx.count(), "total": s.count()}),
                &format!("回填 {done} 条，失败 {failed}，侧车向量 {}，库容 {}", vidx.count(), s.count()),
            );
            if failed > 0 { 1 } else { 0 }
        }

        Cmd::Status => {
            let s = Store::open(dir);
            let vidx = vectors::VectorIndex::open(dir);
            let cfg = embed::embedding_config();
            let files: Vec<serde_json::Value> = s
                .file_stats()
                .into_iter()
                .map(|(f, b)| serde_json::json!({"file": f, "bytes": b}))
                .collect();
            let v = serde_json::json!({
                "data_dir": dir,
                "records": s.count(),
                "lines": s.line_count(),
                "bad_lines": s.bad_lines,
                "files": files,
                "vectors": vidx.count(),
                "embedding": {
                    "configured": cfg.is_some(),
                    "api_base": cfg.as_ref().map(|c| c.api_base.clone()).unwrap_or_default(),
                    "model": cfg.as_ref().map(|c| c.model.clone()).unwrap_or_default(),
                },
                "sync": sync::status(dir),
            });
            emit(json, v.clone(), &serde_json::to_string_pretty(&v).unwrap());
            0
        }

        Cmd::Compact => {
            let mut s = Store::open(dir);
            match s.compact() {
                Ok(before) => {
                    emit(
                        json,
                        serde_json::json!({"lines_before": before, "records": s.count()}),
                        &format!("压实完成：{} 行 → {} 条存活", before, s.count()),
                    );
                    0
                }
                Err(e) => {
                    eprintln!("压实失败: {e}");
                    1
                }
            }
        }

        Cmd::Bench { semantic } => bench::run(semantic, json),

        Cmd::Web { port } => {
            eprintln!("web 界面仅监听 127.0.0.1（明文库不对外）；Ctrl-C 退出");
            match web::serve(dir.clone(), port) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            }
        }

        Cmd::EmbedServer { host, port, model_dir } => {
            embed_server::serve(host, port, model_dir);
            0
        }

        Cmd::Config { action } => match action {
            ConfigCmd::Show => {
                let mut merged = config::embedding_merged();
                // data_dir 展示解析结果（不泄路径层级语义给脚本猜）
                merged["data_dir"] =
                    serde_json::json!(dir.to_string_lossy());
                // api_key 脱敏
                if let Some(k) = merged["api_key"].as_str() {
                    if !k.is_empty() {
                        merged["api_key"] = serde_json::json!("***");
                    }
                }
                let proj = config::find_project_dir();
                let v = serde_json::json!({
                    "merged": merged,
                    "project_dir": proj.map(|p| p.to_string_lossy().into_owned()),
                    "user_config": config::user_config_path().to_string_lossy(),
                });
                emit(json, v.clone(), &serde_json::to_string_pretty(&v).unwrap());
                0
            }
            ConfigCmd::Set { key, value, project } => match config::config_set(&key, &value, project) {
                Ok(path) => {
                    emit(
                        json,
                        serde_json::json!({"key": key, "path": path.to_string_lossy(), "scope": if project {"project"} else {"user"}}),
                        &format!("已写入 {}（{}级）", path.display(), if project { "项目" } else { "用户" }),
                    );
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            },
        },
    }
}
