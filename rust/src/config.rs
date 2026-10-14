//! 配置分层：项目级（工作区记忆文件夹内 config.json）> 用户级（~/.memlite/config.json）。
//!
//! - data_dir：CLI flag > MEMLITE_DATA > 工作区记忆文件夹 > 用户级 data_dir > 默认
//! - embedding：env > 项目级 > 用户级（API 三件套写入默认落用户级——`config set` 不带 --project）
//! - 工作区记忆文件夹 = 从 cwd 向上找：`<dir>/.memlite/config.json` 或 `<dir>/memlite-data/config.json`，
//!   文件夹本身即 data_dir（项目级记忆与工作区同生共死，随工作区 git 一起同步）。

use serde_json::Value;
use std::path::{Path, PathBuf};

/// 家目录：HOME（Linux/macOS）→ USERPROFILE（Windows）→ "."
pub fn home_dir() -> String {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into())
}

pub fn user_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("MEMLITE_CONFIG") {
        return PathBuf::from(p);
    }
    let home = home_dir();
    PathBuf::from(home).join(".memlite/config.json")
}

pub fn load(path: &Path) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

/// 从 cwd 向上找工作区记忆文件夹（含 config.json 的 .memlite/ 或 memlite-data/）。
/// 找到的第一个即项目级 data_dir——不跨出工作区继续上爬（git 根或文件系统根为界）。
/// 用户级配置目录（~/.memlite）与项目级同名，必须排除——否则家目录跑任何命令都会
/// 把用户配置误认成项目库（data_dir 错指 ~/.memlite，教训 2026-10-14 真机抓到）。
pub fn find_project_dir() -> Option<PathBuf> {
    let user_dir = user_config_path().parent().map(|p| p.to_path_buf());
    let mut cur = std::env::current_dir().ok()?;
    loop {
        for name in [".memlite", "memlite-data"] {
            let cand = cur.join(name);
            if cand.join("config.json").is_file() {
                if user_dir.as_deref() == Some(cand.as_path()) {
                    continue; // 用户级配置，不是项目库
                }
                return Some(cand);
            }
        }
        // git 根不越过：工作区即仓库根
        if cur.join(".git").exists() || !cur.pop() {
            return None;
        }
    }
}

pub fn project_config() -> (Option<PathBuf>, Value) {
    match find_project_dir() {
        Some(d) => {
            let cfg = load(&d.join("config.json"));
            (Some(d), cfg)
        }
        None => (None, Value::Null),
    }
}

/// 用户级 data_dir（不受项目级影响——两档回退的兜底目标）
pub fn user_data_dir() -> PathBuf {
    let user = load(&user_config_path());
    if let Some(d) = user["data_dir"].as_str() {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    let home = home_dir();
    PathBuf::from(home).join("memlite-data")
}

/// data_dir 解析（四层）
pub fn resolve_data_dir(cli: Option<&Path>) -> PathBuf {
    if let Some(d) = cli {
        return d.to_path_buf();
    }
    if let Ok(v) = std::env::var("MEMLITE_DATA") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Some(proj) = find_project_dir() {
        return proj;
    }
    user_data_dir()
}

/// embedding 合并：用户级为底、项目级覆盖、env 最后覆盖（操作者意图最优先）
pub fn embedding_merged() -> Value {
    let user = load(&user_config_path());
    let (_, proj) = project_config();
    let mut out = Value::Object(Default::default());
    for src in [user["embedding"].clone(), proj["embedding"].clone()] {
        if let Some(obj) = src.as_object() {
            if let Some(o) = out.as_object_mut() {
                for (k, v) in obj {
                    o.insert(k.clone(), v.clone());
                }
            }
        }
    }
    if let Ok(v) = std::env::var("MEMLITE_API_BASE") {
        out["api_base"] = Value::String(v);
    }
    if let Ok(v) = std::env::var("MEMLITE_API_KEY") {
        out["api_key"] = Value::String(v);
    }
    if let Ok(v) = std::env::var("MEMLITE_EMBED_MODEL") {
        out["model"] = Value::String(v);
    }
    out
}

/// `memlite config set <key> <value>`——key 形如 data_dir / embedding.api_base；
/// 缺省写用户级（全局），`--project` 写项目级（须已识别到工作区记忆文件夹）。
pub fn config_set(key: &str, value: &str, project: bool) -> Result<PathBuf, String> {
    let (path, mut cfg) = if project {
        let (dir, cfg) = project_config();
        let dir = dir.ok_or_else(|| "未识别到工作区记忆文件夹（.memlite/ 或 memlite-data/ 含 config.json）".to_string())?;
        (dir.join("config.json"), cfg)
    } else {
        (user_config_path(), load(&user_config_path()))
    };
    if !cfg.is_object() {
        cfg = Value::Object(Default::default());
    }
    let keys: Vec<&str> = key.split('.').collect();
    match keys.as_slice() {
        [single] => {
            cfg[*single] = value_into(single, value);
        }
        [sec, field] => {
            if !cfg[sec].is_object() {
                cfg[sec] = Value::Object(Default::default());
            }
            cfg[sec][*field] = Value::String(value.to_string());
        }
        _ => return Err(format!("key 只支持一层或两层（如 embedding.api_base）: {key}")),
    }
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap_or_default())
        .map_err(|e| e.to_string())?;
    Ok(path)
}

/// data_dir 这类路径值原样存，其余按字符串存（不猜类型，简单可预期）
fn value_into(key: &str, value: &str) -> Value {
    if key == "data_dir" {
        Value::String(shellexpand_home(value))
    } else {
        Value::String(value.to_string())
    }
}

fn shellexpand_home(v: &str) -> String {
    if let Some(rest) = v.strip_prefix("~/") {
        let home = home_dir();
        format!("{home}/{rest}")
    } else {
        v.to_string()
    }
}
