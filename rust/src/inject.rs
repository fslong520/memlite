//! 注入层：把 memlite 使用规则写进各 AI 载体。
//!
//! 三档策略（按载体能力，国内外主流产品通吃）：
//! 1. **rules 档** —— 产品有专用 rules 机制（Cursor/Windsurf/Qoder/Zed/Cline/Roo 等），
//!    产出独立 rules 文件 + 拷贝引导（各家 rules 格式要求各异——frontmatter、目录位置，
//!    替他方写文件易踩其解析规则，引导用户按表拷贝最稳）；
//! 2. **memory 档** —— 无 rules 但有长期记忆文件（Claude CLAUDE.md、Gemini GEMINI.md、
//!    Copilot instructions、CodeBuddy/WorkBuddy memory），块嵌追加；
//! 3. **agents 档** —— 都没有的走 AGENTS.md 事实标准（Codex/opencode/DeepSeek/Qwen/
//!    豆包/Grok/pi/dsh/kylinbot…），块嵌追加，通用兜底 ~/AGENTS.md。
//!
//! `memlite inject --emit` 生成系统级 rules 全文（memlite-rules.md）并打印逐产品拷贝表。

use std::fs;
use std::path::{Path, PathBuf};

pub const BEGIN: &str = "<!-- memlite:begin -->";
pub const END: &str = "<!-- memlite:end -->";

/// 冲突标记：命中任一即认为目标已挂另一套记忆系统
const CONFLICT: [&str; 5] = ["respire", "1memory", "rsrs recall", "rsrs remember", "忆时"];

pub struct Target {
    pub name: &'static str,
    pub tier: &'static str, // "rules" | "memory" | "agents"
    pub path: &'static str,
    /// auto=true 才参与 --target/--all 块嵌；rules 档一律 false（格式各异，走 emit 引导）
    pub auto: bool,
    pub note: &'static str,
}

pub const TARGETS: &[Target] = &[
    // ---- rules 档：有专用 rules 机制（emit 引导，不自动块嵌）----
    Target { name: "qoder", tier: "rules", path: "~/.qoder/rules/memlite.md", auto: false,
        note: "若已有全局 rules 全文（如 xiaoyu-default.md），勿再拷——双 always_on 互相冲淡" },
    Target { name: "cursor", tier: "rules", path: ".cursor/rules/memlite.mdc", auto: false,
        note: "项目仓内；.mdc 头部加 frontmatter：--- / alwaysApply: true / ---" },
    Target { name: "windsurf", tier: "rules", path: ".windsurf/rules/memlite.md", auto: false,
        note: "项目仓内；文件头加 frontmatter：--- / alwaysApply: true / ---" },
    Target { name: "claude-rules", tier: "rules", path: "~/.claude/rules/memlite.md", auto: false,
        note: "Claude Code rules 目录；已有 CLAUDE.md 块嵌时二选一勿双注" },
    Target { name: "zed", tier: "rules", path: "~/.config/zed/rules.md", auto: false,
        note: "Zed 全局 rules" },
    Target { name: "cline", tier: "rules", path: ".clinerules", auto: false, note: "项目仓根单文件" },
    Target { name: "roo", tier: "rules", path: ".roorules", auto: false, note: "项目仓根单文件" },
    Target { name: "trae", tier: "rules", path: ".trae/rules/memlite.md", auto: false,
        note: "项目仓内" },
    // ---- memory 档：长期记忆文件（块嵌追加）----
    Target { name: "claude", tier: "memory", path: "~/.claude/CLAUDE.md", auto: true,
        note: "CLAUDE.md 是 Claude Code 的长期指令文件" },
    Target { name: "gemini", tier: "memory", path: "~/.gemini/GEMINI.md", auto: true,
        note: "Gemini CLI 全局 memory" },
    Target { name: "copilot", tier: "memory", path: ".github/copilot-instructions.md", auto: true,
        note: "项目仓内；GitHub Copilot 自定义指令" },
    Target { name: "codebuddy", tier: "memory", path: "~/.codebuddy/CODEBUDDY.md", auto: true, note: "" },
    Target { name: "workbuddy", tier: "memory", path: "~/.workbuddy/MEMORY.md", auto: true,
        note: "每轮注入有 4000 字符上限，只写核心规则+全量路径（用 lite 模式）" },
    // ---- agents 档：AGENTS.md 事实标准（块嵌追加）----
    Target { name: "codex", tier: "agents", path: "~/.codex/AGENTS.md", auto: true, note: "OpenAI Codex CLI" },
    Target { name: "opencode", tier: "agents", path: "~/.config/opencode/AGENTS.md", auto: true, note: "opencode 全局" },
    Target { name: "dsh", tier: "agents", path: "~/.dsh/AGENTS.md", auto: true, note: "" },
    Target { name: "pi", tier: "agents", path: "~/.pi/AGENTS.md", auto: true, note: "" },
    Target { name: "grok", tier: "agents", path: "~/.grok/AGENTS.md", auto: true, note: "Grok CLI" },
    Target { name: "deepseek", tier: "agents", path: "~/.deepseek/AGENTS.md", auto: true, note: "DeepSeek CLI" },
    Target { name: "qwen", tier: "agents", path: "~/.qwen/AGENTS.md", auto: true, note: "通义千问 CLI" },
    Target { name: "doubao", tier: "agents", path: "~/.doubao/AGENTS.md", auto: true, note: "豆包 CLI" },
    Target { name: "zigcode", tier: "agents", path: "~/.zcode/AGENTS.md", auto: true, note: "" },
    Target { name: "kylinbot", tier: "agents", path: "~/.kylinbot/workspace/AGENTS.md", auto: true, note: "" },
    Target { name: "generic", tier: "agents", path: "~/AGENTS.md", auto: true,
        note: "兜底：多数 harness 都读工作目录或家目录的 AGENTS.md" },
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

/// 块嵌正文（写进他方文件的 <BEGIN>…<END> 块）
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

/// 独立系统级 rules 文件全文（emit 用；整文件语义，无块标记、不含本机路径）
pub fn standalone_text() -> String {
    "# memlite 记忆工作流\n\n\
本机记忆库由 memlite 管理（明文 JSONL，GitHub 私有仓同步）。CLI：`memlite <命令>`。\n\n\
总纲：**凡开口先查，凡动手必查，凡所得可录，凡碰壁翻忆。**\n\n\
## 铁锚 · 三行\n\n\
1. **答前必查**：`memlite recall \"项目名+关键词\"`——未翻忆不开答。项目级库优先（工作区含 config.json 的 `.memlite/`），没搜到 CLI 自动回退用户级。\n\
2. **收尾必存**：新事实/决策/教训 `memlite remember \"内容\" --title \"事实句\"`——未落库不算完。\n\
3. **遇障先翻忆**：报错、异常先 `memlite recall \"报错原文片段\"` 再动手——未查不动手、不试错。\n\n\
## 流程\n\n\
1. **开口先查**：`memlite recall \"<项目名+核心词>\"`（缺省 top10；`--json` 结构化）。两档皆空换 2–3 组词再查。\n\
2. **所得必存**：先 recall 判重 → `memlite show <id>` 读候选 → 改＞并＞存：旧条更新走 `memlite update <id> --content/--title`（未传字段保留原值）；同题散条合成综合版后 forget 旧条；两轮换词皆空才新存。一事一条。\n\
3. **标题铁律**：`--title` 必须是事实句（主语＋动作＋关键实体或结果），禁只写主题分类名。\n\
4. **敏感闸**：口令/证件号/手机号/银行卡/token/私钥禁入正文与查询词，只写存放位置（如「密码在 ~/.env」）——CLI 写前自动扫描，命中即拒。\n\
5. **同步**：`memlite sync` 只 add 白名单数据文件，push 后双通道核验 sha；数据仓须为 private 仓库。\n\
6. **管理台**：`memlite web --port 7879` 起本地回环管理页（语义检索/增删改/时间线/标签/版本历史/注入）。\n"
        .to_string()
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
    let Some(t) = TARGETS.iter().find(|t| t.name == target && t.auto) else {
        let known = TARGETS.iter().any(|t| t.name == target);
        return if known {
            serde_json::json!({"target": target, "error": "该目标走 emit 拷贝引导（rules 格式各异，不自动块嵌）；memlite inject --emit 查看"})
        } else {
            serde_json::json!({"target": target, "error": "未知目标"})
        };
    };
    let path = expand(t.path);
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
    serde_json::json!({"target": target, "tier": t.tier, "path": path, "state": state, "changed": changed})
}

pub fn probe() -> Vec<serde_json::Value> {
    TARGETS
        .iter()
        .map(|t| {
            let path = expand(t.path);
            let state = check_state(&path);
            serde_json::json!({"target": t.name, "tier": t.tier, "path": path, "state": state,
                                "auto": t.auto, "note": t.note})
        })
        .collect()
}

pub fn remove(target: &str) -> serde_json::Value {
    let Some(t) = TARGETS.iter().find(|t| t.name == target && t.auto) else {
        return serde_json::json!({"target": target, "error": "未知目标或该目标不支持自动块嵌"});
    };
    let path = expand(t.path);
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

/// emit 引导表（人读）：按档位分组的逐产品拷贝命令
pub fn emit_guidance(out_file: &str) -> String {
    let mut s = format!(
        "已生成 {out_file}（独立系统级 rules 全文）——按你的产品拷贝到对应载体：\n\n"
    );
    for (tier, label) in [
        ("rules", "① rules 档（产品有专用 rules 机制，整文件拷贝到其 rules 路径）"),
        ("memory", "② memory 档（长期记忆文件，块嵌追加——文件已有内容不覆盖）"),
        ("agents", "③ agents 档（AGENTS.md 事实标准，块嵌追加）"),
    ] {
        s.push_str(&format!("{label}：\n"));
        for t in TARGETS.iter().filter(|t| t.tier == tier) {
            let action = if tier == "rules" {
                format!("cp {out_file} {}", t.path)
            } else {
                format!("cat {out_file} >> {}", t.path)
            };
            s.push_str(&format!("  {:<14} {}\n", t.name, action));
            if !t.note.is_empty() {
                s.push_str(&format!("                 ↳ {}\n", t.note));
            }
        }
        s.push('\n');
    }
    s.push_str("提示：memory/agents 档的块嵌可直接 `memlite inject --target <名>` 自动写（幂等、带冲突检测），emit 拷贝适合 rules 档或批量离线部署。\n");
    s
}

/// emit JSON 形态（web/API 复用）
pub fn emit_json(out_file: &str) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = TARGETS
        .iter()
        .map(|t| {
            serde_json::json!({
                "target": t.name, "tier": t.tier, "path": t.path, "auto": t.auto,
                "action": if t.tier == "rules" {
                    format!("cp {out_file} {}", t.path)
                } else {
                    format!("cat {out_file} >> {}", t.path)
                },
                "note": t.note
            })
        })
        .collect();
    serde_json::json!({"file": out_file, "targets": rows})
}
