//! memlite CLI——简化嵌入式记忆系统（Rust 版）。
//! 命令面与 Python 版对齐：init/remember/recall/show/list/forget/sync/inject/reembed/status/compact。

mod embed;
mod inject;
mod search;
mod store;
mod sync;
mod vectors;

use clap::{Parser, Subcommand};
use store::Store;
use std::path::PathBuf;

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
}

fn data_dir(arg: &Option<PathBuf>) -> PathBuf {
    if let Some(d) = arg {
        return d.clone();
    }
    if let Ok(v) = std::env::var("MEMLITE_DATA") {
        return PathBuf::from(v);
    }
    // config.json 的 data_dir
    let cfg = std::env::var("MEMLITE_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".memlite/config.json")
        });
    if let Ok(text) = std::fs::read_to_string(&cfg) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(d) = v["data_dir"].as_str() {
                return PathBuf::from(d);
            }
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("memlite-data")
}

fn emit(json: bool, value: serde_json::Value, human: &str) {
    if json {
        println!("{}", serde_json::to_string_pretty(&value).unwrap());
    } else {
        println!("{human}");
    }
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
            let s = Store::open(dir);
            let recs = s.all();
            let mut backend = "bm25";
            let mut hits: Vec<(f64, &store::Record)> = vec![];
            if !bm25 {
                if let Some(cfg) = embed::embedding_config() {
                    match embed::embed(&cfg, &query) {
                        Ok(qv) => {
                            let mut vidx = vectors::VectorIndex::open(dir);
                            let mut getter = |id: &str| vidx.get(id);
                            hits = search::semantic(&recs, &qv, limit, &project, &mut getter);
                            backend = "semantic";
                        }
                        Err(_) => hits = vec![], // 静默回退词面
                    }
                }
            }
            if backend != "semantic" || hits.is_empty() {
                backend = "bm25";
                hits = search::bm25(&recs, &query, limit, &project);
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
                    serde_json::to_string_pretty(&serde_json::json!({"backend": backend, "hits": arr})).unwrap()
                );
            } else {
                eprintln!("（{backend} 检索）");
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
    }
}
