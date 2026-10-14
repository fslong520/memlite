//! 注入层：把 memlite 使用规则写进各 AI 软件的 rules/记忆文件。
//! qoder 不在表内（其 rules 全文手工管理，再注一份成双 always_on 会打架）。

use std::fs;
use std::path::{Path, PathBuf};

pub const BEGIN: &str = "<!-- memlite:begin -->";
pub const END: &str = "<!-- memlite:end -->";

/// 冲突标记：命中任一即认为目标已挂另一套记忆系统
const CONFLICT: [&str; 5] = ["respire", "1memory", "rsrs recall", "rsrs remember", "忆时"];

pub const TARGETS: [(&str, &str); 7] = [
    ("claude", "~/.claude/CLAUDE.md"),
    ("codex", "~/.codex/AGENTS.md"),
    ("codebuddy", "~/.codebuddy/CODEBUDDY.md"),
    ("workbuddy", "~/.workbuddy/MEMORY.md"),
    ("pi", "~/.pi/AGENTS.md"),
    ("kylinbot", "~/.kylinbot/workspace/AGENTS.md"),
    ("generic", "~/AGENTS.md"),
];

fn expand(p: &str) -> PathBuf {
    let raw = if let Some(rest) = p.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        format!("{home}/{rest}")
    } else {
        p.to_string()
    };
    PathBuf::from(raw)
}

pub fn rules_text(data_dir: &str, lite: bool) -> String {
    let abs = fs::canonicalize(data_dir)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| data_dir.to_string());
    let mut s = format!(
        "{BEGIN}\n# memlite 记忆系统\n\n本机记忆库：`{abs}`（明文 JSONL，GitHub 私有仓同步）。\n\n## 铁律\n\n1. 答前先查：`memlite recall \"项目名+关键词\"`——未查不开答。\n2. 收尾必存：新事实/决策/教训 `memlite remember \"内容\" --title \"…\"`。\n3. --title 必须写事实句（主语＋动作＋关键实体或结果），禁只写主题分类名——检索靠词面与正文，分类名等于没写。\n4. 改旧条用 `memlite update <id>`——传要改的 `--title`/`--content`/`--tags`（标题同样可改，未传字段保留原值）；整条纠错或合并先 `show <id>` 确认，再 `forget` + `remember`。禁直接改数据文件。\n5. 同步：`memlite sync`（只 add 白名单数据文件，push 后核验 sha）。\n6. 数据仓是私有仓假设；禁止把 token/密钥写进记忆正文。\n"
    );
    if lite {
        s.push_str(&format!("全量规则见 `{abs}/../rules.md`。\n"));
    }
    s.push_str(END);
    s.push('\n');
    s
}

pub fn check_state(path: &Path) -> &'static str {
    let Ok(text) = fs::read_to_string(path) else {
        return "absent";
    };
    let has_block = text.contains(BEGIN) && text.contains(END);
    let own_only = text.trim_start().starts_with(BEGIN);
    let conflict = CONFLICT.iter().any(|m| text.contains(m)) && !own_only;
    if conflict && !has_block {
        "conflict"
    } else if has_block {
        if text.matches(BEGIN).count() == 1 {
            "fresh"
        } else {
            "stale"
        }
    } else {
        "absent"
    }
}

pub fn inject(target: &str, data_dir: &str, force: bool, lite: bool) -> serde_json::Value {
    let Some((_, path)) = TARGETS.iter().find(|(t, _)| *t == target) else {
        return serde_json::json!({"target": target, "error": "未知目标"});
    };
    let path = expand(path);
    let state = check_state(&path);
    if state == "conflict" && !force {
        return serde_json::json!({
            "target": target, "path": path, "state": state, "changed": false,
            "hint": "目标已有 respire/1memory 规则，拒绝双注入；确需则 --force"
        });
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let block = rules_text(data_dir, lite);
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let new_text = if existing.contains(BEGIN) && existing.contains(END) {
        let pre = existing.split(BEGIN).next().unwrap_or("");
        let post = existing.split_once(END).map(|(_, r)| r.trim_start_matches('\n')).unwrap_or("");
        if post.is_empty() {
            format!("{pre}{block}")
        } else {
            format!("{pre}{block}\n{post}")
        }
    } else if existing.is_empty() {
        block
    } else {
        let sep = if existing.ends_with('\n') {
            if existing.ends_with("\n\n") { "" } else { "\n" }
        } else {
            "\n\n"
        };
        format!("{existing}{sep}{block}")
    };
    let changed = new_text != existing;
    if changed {
        let tmp = path.with_extension("md.tmp");
        fs::write(&tmp, &new_text).expect("写临时文件失败");
        fs::rename(&tmp, &path).expect("替换失败");
    }
    serde_json::json!({"target": target, "path": path, "state": state, "changed": changed})
}

pub fn probe() -> Vec<serde_json::Value> {
    TARGETS
        .iter()
        .map(|(t, p)| {
            let path = expand(p);
            let state = check_state(&path);
            serde_json::json!({"target": t, "path": path, "state": state})
        })
        .collect()
}

pub fn remove(target: &str) -> serde_json::Value {
    let Some((_, path)) = TARGETS.iter().find(|(t, _)| *t == target) else {
        return serde_json::json!({"target": target, "error": "未知目标"});
    };
    let path = expand(path);
    let Ok(text) = fs::read_to_string(&path) else {
        return serde_json::json!({"target": target, "path": path, "changed": false});
    };
    if !text.contains(BEGIN) {
        return serde_json::json!({"target": target, "path": path, "changed": false});
    }
    let pre = text.split(BEGIN).next().unwrap_or("").trim_end_matches('\n');
    let post = text.split_once(END).map(|(_, r)| r.trim_start_matches('\n')).unwrap_or("");
    let mut parts: Vec<&str> = vec![];
    if !pre.is_empty() {
        parts.push(pre);
    }
    if !post.is_empty() {
        parts.push(post);
    }
    let mut new_text = parts.join("\n");
    if !new_text.is_empty() && !new_text.ends_with('\n') {
        new_text.push('\n');
    }
    fs::write(&path, new_text).expect("写回失败");
    serde_json::json!({"target": target, "path": path, "changed": true})
}
