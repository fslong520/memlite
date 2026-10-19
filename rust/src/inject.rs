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
//!
//! 块嵌正文 = ~/.qoder/rules/AGENTS.md 全文（单一事实源，读不到即报错拒注）。
//! qoder 自身勿再块嵌——该文件本身是注入源，再嵌即双 always_on 互相冲淡。

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

/// 注入正文的单一事实源：Qoder 规则文件
pub const SOURCE: &str = "~/.qoder/rules/AGENTS.md";

fn expand(p: &str) -> PathBuf {
    let raw = if let Some(rest) = p.strip_prefix("~/") {
        let home = crate::config::home_dir();
        format!("{home}/{rest}")
    } else {
        p.to_string()
    };
    PathBuf::from(raw)
}

pub fn source_path() -> PathBuf {
    expand(SOURCE)
}

pub fn rules_text(lite: bool) -> Result<String, String> {
    rules_text_from(&source_path(), lite)
}

/// 正文取注入源全文包进 BEGIN/END；lite（4000 字符上限目标）只嵌指针。
/// 源缺失/不可读/为空一律 Err——正文以该文件为准，不静默回退旧文本。
fn rules_text_from(source: &Path, lite: bool) -> Result<String, String> {
    let text = fs::read_to_string(source).map_err(|e| {
        format!("读不到注入源 {}：{e}——先修该文件再注入", source.display())
    })?;
    if text.trim().is_empty() {
        return Err(format!("注入源 {} 是空文件，拒绝注入", source.display()));
    }
    if lite {
        return Ok(format!(
            "{BEGIN}\n# memlite 记忆系统\n\n全量规则见 `{}`（Qoder 每会话常驻的注入源）。\n{END}\n",
            source.display()
        ));
    }
    let body = text.trim_end_matches('\n');
    Ok(format!("{BEGIN}\n{body}\n{END}\n"))
}

/// 独立系统级 rules 文件全文（emit 用；整文件语义，无块标记、不含本机路径）
pub fn standalone_text() -> String {
    "# memlite 记忆工作流\n\n本机记忆库由 memlite 管理（明文 JSONL，GitHub 私有仓同步）。CLI：`memlite <命令>`。\n\n总纲：**凡开口先查，凡动手必查，凡所得可录，凡碰壁翻忆。**\n\n## 铁锚 · 三行\n\n1. **答前必查**：`memlite recall \"项目名+关键词\"`——未翻忆不开答。项目级库优先（工作区含 config.json 的 `.memlite/`），没搜到 CLI 自动回退用户级。\n2. **收尾必存即推**：新事实/决策/教训 `memlite remember \"内容\" --title \"事实句\"` 落库后紧接 `memlite sync`——未推上远端不算完。\n3. **遇障先翻忆**：报错、异常先 `memlite recall \"报错原文片段\"` 再动手——未查不动手、不试错。\n\n## 流程\n\n1. **开口先查**：`memlite recall \"<项目名+核心词>\"`（缺省 top10；`--json` 结构化）。两档皆空换 2–3 组词再查。\n2. **所得必存**：先 recall 判重 → `memlite show <id>` 读候选 → 改＞并＞存：旧条更新走 `memlite update <id> --content/--title`（未传字段保留原值）；同题散条合成综合版后 forget 旧条；两轮换词皆空才新存。一事一条。\n3. **标题铁律**：`--title` 必须是事实句（主语＋动作＋关键实体或结果），禁只写主题分类名。\n4. **敏感闸**：口令/证件号/手机号/银行卡/token/私钥禁入正文与查询词，只写存放位置（如「密码在 ~/.env」）——CLI 写前自动扫描，命中即拒。\n5. **同步（存后即跑）**：`remember`/`forget`/`update` 落库后紧接 `memlite sync`，收尾再跑一次；数据仓须为 private 仓库。sync 内置白名单 add（`memory*.jsonl`／`meta.json`／`README.md`，禁 `git add -A`）→ commit → pull --rebase → push → 双通道核验 local/remote sha，回执须 `verified:true` 方算同步。失败分诊：**瞬时错**（index.lock、并发 git 互踩、push 超时）隔 30s 重试——sync 先 commit 后推，失败不丢数据；**真冲突**（rebase 残留 `UU`＋HEAD 游离，另台设备推过同区内容）走并集三步，任一不过即停手、禁 force 禁 drop——① `git merge-base` 取共同基底，验两侧对基底只增不删不改；② 两侧新增行 id 零交集（同 id 双改须人工裁决）；③ 过①②写并集（上游侧全量＋本地侧不重复行）→ `git add` → `GIT_EDITOR=true git rebase --continue` → 重跑 sync 取 `verified:true`，终验两侧新增 id 全在、无 `<<<<<<<` 标记残留。`memory.querylog.jsonl` 无 id 键，冲突按行并集去重即可。\n6. **管理台**：`memlite web --port 7879` 起本地回环管理页（语义检索/增删改/时间线/标签/版本历史/注入）。\n\n## 因果树梳理（判官是你）\n\nmemlite 不调模型判挂——读本规则的你自任判官，CLI 只给原语：\n\n- **认因入库**：新条上游已定 → `remember --parent <纲名/id>` 显式挂载；拿不准先裸存，留待梳理。\n- **散记归树**：`tree --outline`（只出 id+标题全深目录，梳理先读此）→ 判该挂哪纲 → `attach <子> --parent <父>`。\n- **错挂纠正**：升层 `promote`（挂到祖父/根）、下沉 `demote <子> --parent <父>`。改挂已防自挂防环，违规即拒。\n- **深挖因果**：见因链名要全文 → `chain <id> [--depth N]` 一把取回整条链，别逐层 show。\n- **查树形**：`tree [--from <id>] [--depth N]` 看森林或子树。梳理一轮收尾 sync。\n".to_string()
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

pub fn inject(target: &str, force: bool, lite: bool) -> serde_json::Value {
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
    let block = match rules_text(lite) {
        Ok(b) => b,
        Err(e) => {
            return serde_json::json!({
                "target": target, "path": path, "state": state, "changed": false, "error": e
            })
        }
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("memlite-inject-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn rules_text_wraps_source_fulltext() {
        let d = tmp("wrap");
        let src = d.join("AGENTS.md");
        fs::write(&src, "# 规则正文\n\n两档检索。\n").unwrap();
        let out = rules_text_from(&src, false).unwrap();
        assert_eq!(out.matches(BEGIN).count(), 1);
        assert_eq!(out.matches(END).count(), 1);
        assert!(out.contains("# 规则正文"));
        assert!(out.contains("两档检索"));
        assert!(out.trim_end().ends_with(END));
    }

    #[test]
    fn rules_text_missing_source_errors() {
        let d = tmp("missing");
        let err = rules_text_from(&d.join("nope.md"), false).unwrap_err();
        assert!(err.contains("读不到注入源"), "got: {err}");
    }

    #[test]
    fn rules_text_empty_source_errors() {
        let d = tmp("empty");
        let src = d.join("AGENTS.md");
        fs::write(&src, "   \n").unwrap();
        let err = rules_text_from(&src, false).unwrap_err();
        assert!(err.contains("空文件"), "got: {err}");
    }

    #[test]
    fn rules_text_lite_is_pointer_only() {
        let d = tmp("lite");
        let src = d.join("AGENTS.md");
        fs::write(&src, "# 秘密正文不该出现\n").unwrap();
        let out = rules_text_from(&src, true).unwrap();
        assert!(out.contains(&src.display().to_string()));
        assert!(!out.contains("秘密正文"));
        assert_eq!(out.matches(BEGIN).count(), 1);
    }

    /// 仓库附带的通用 rules 文件必须与 standalone_text() 逐字一致（漂移即红；行尾归一化——Windows 检出 CRLF）。
    #[test]
    fn repo_rules_file_matches_standalone() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/memlite-rules.md");
        let text = fs::read_to_string(&path).expect("docs/memlite-rules.md 应随仓库存在");
        let lf = |s: &str| s.replace("\r\n", "\n");
        assert_eq!(
            lf(&text),
            lf(&standalone_text()),
            "docs/memlite-rules.md 与 inject::standalone_text() 漂移——重跑 `memlite inject --emit docs/memlite-rules.md` 更新并提交"
        );
    }

    #[test]
    fn check_state_cycle() {
        let d = tmp("state");
        let p = d.join("T.md");
        assert_eq!(check_state(&p), "absent");
        fs::write(&p, format!("head\n{BEGIN}\nx\n{END}\n")).unwrap();
        assert_eq!(check_state(&p), "fresh");
        fs::write(&p, format!("{BEGIN}\nx\n{END}\n{BEGIN}\ny\n{END}\n")).unwrap();
        assert_eq!(check_state(&p), "stale");
        fs::write(&p, "respire 规则在此\n").unwrap();
        assert_eq!(check_state(&p), "conflict");
    }

    /// 端到端：假 HOME 下注入→幂等→源缺失拒注。HOME 为进程级，靠单测集中占用。
    #[test]
    fn e2e_inject_fake_home() {
        let old_home = std::env::var("HOME").ok();
        let home = tmp("home");
        fs::create_dir_all(home.join(".qoder/rules")).unwrap();
        fs::write(home.join(".qoder/rules/AGENTS.md"), "# memlite 记忆工作流\n").unwrap();
        std::env::set_var("HOME", &home);

        let first = inject("claude", false, false);
        assert_eq!(first["changed"], true, "first: {first}");
        let target = home.join(".claude/CLAUDE.md");
        let text = fs::read_to_string(&target).unwrap();
        assert!(text.contains("# memlite 记忆工作流"));
        assert_eq!(text.matches(BEGIN).count(), 1);

        let second = inject("claude", false, false);
        assert_eq!(second["changed"], false, "idempotent: {second}");

        fs::remove_file(home.join(".qoder/rules/AGENTS.md")).unwrap();
        let fail = inject("claude", false, false);
        assert_eq!(fail["changed"], false);
        assert!(fail["error"].as_str().unwrap().contains("读不到注入源"), "fail: {fail}");

        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }
}
