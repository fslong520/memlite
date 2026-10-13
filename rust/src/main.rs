//! memlite CLI——简化嵌入式记忆系统（Rust 版）。
//! 命令面与 Python 版对齐：init/remember/recall/show/list/forget/sync/inject/reembed/status/compact。

mod config;
mod embed;
mod inject;
mod search;
mod sensitive;
mod store;
mod sync;
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
    /// 就地改一条（同 id 追加新版本，末写胜出；id/标签/项目保留除非显式给）
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
    /// rules 注入
    Inject {
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        probe: bool,
        #[arg(long)]
        remove: Option<String>,
        #[arg(long)]
        force: bool,
    },
    /// 给无向量的记忆回填嵌入（须配 API）
    Reembed,
    /// 状态
    Status,
    /// 压实（只留存活重写回单文件）
    Compact,
    /// 内置 Web 管理界面（本地回环）
    Web {
        #[arg(long, default_value_t = 7879)]
        port: u16,
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

fn run(cli: Cli, dir: &PathBuf) -> i32 {
    let json = cli.json;
    match cli.cmd {
        Cmd::Init => {
            std::fs::create_dir_all(dir).expect("建目录失败");
            let cfg = std::env::var("MEMLITE_CONFIG")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
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
            if !dir.join(".git").is_dir() {
                let _ = std::process::Command::new("git")
                    .args(["init", "-b", "main"])
                    .arg(dir)
                    .output();
            }
            let s = Store::open(dir);
            emit(
                json,
                serde_json::json!({"data_dir": dir, "records": s.count()}),
                &format!("数据目录: {}\n记忆条数: {}", dir.display(), s.count()),
            );
            0
        }

        Cmd::Remember { content, title, tags, project } => {
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
            let rid = match s.put(&content, &title, &tag_list, &project) {
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
                serde_json::json!({"id": rid, "title": title, "embedded": embedded, "records": s.count()}),
                &format!("已存 {rid}「{title}」{note}（共 {} 条）", s.count()),
            );
            0
        }

        Cmd::Recall { query, limit, project, bm25 } => {
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
            if json {
                let arr: Vec<serde_json::Value> = hits
                    .iter()
                    .map(|(s, r)| {
                        serde_json::json!({
                            "score": (s * 10000.0).round() / 10000.0,
                            "id": r.id, "ts": r.ts, "title": r.title,
                            "content": r.content, "tags": r.tags, "project": r.project
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
                for (sc, r) in &hits {
                    let head = if r.title.is_empty() {
                        r.content.chars().take(40).collect::<String>()
                    } else {
                        r.title.clone()
                    };
                    let head = head.replace('\n', " ");
                    println!("[{sc:.3}] {}  {head}", r.id);
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
            if json {
                // 原样输出该条 JSON
                let v = serde_json::to_value(r).unwrap();
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            } else {
                println!(
                    "id: {}\ncreated: {}\ntitle: {}\ntags: {}\nproject: {}\n---\n{}",
                    r.id, r.ts, r.title, r.tags.join(", "), r.project, r.content
                );
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

        Cmd::Inject { target, all, probe: probe_only, remove, force } => {
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
            let dir_s = dir.to_string_lossy().into_owned();
            let results: Vec<serde_json::Value> = if all {
                inject::TARGETS.iter().map(|(t, _)| inject::inject(t, &dir_s, force, false)).collect()
            } else if let Some(t) = target {
                vec![inject::inject(&t, &dir_s, force, false)]
            } else {
                eprintln!("须指定 --target <名> 或 --all 或 --probe 或 --remove <名>");
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
