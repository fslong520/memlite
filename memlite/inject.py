"""注入层：把 memlite 使用规则写进各 AI 软件的 rules/记忆文件。

调研所得目标表（2026-10-13，ok/linux 真机核对）：
- qoder    → ~/.qoder/rules/ 单独规则文件（rules 是 Qoder 唯一全文载体，定论见 respire 注入 Qoder 记录）
- claude   → ~/.claude/CLAUDE.md
- codex    → ~/.codex/AGENTS.md
- codebuddy→ ~/.codebuddy/CODEBUDDY.md
- workbuddy→ ~/.workbuddy/MEMORY.md（每轮注入 <user_memory>，只写核心规则+全量路径，不嵌全文）
- pi       → ~/.pi/AGENTS.md
- kylinbot → ~/.kylinbot/workspace/AGENTS.md
- generic  → ~/AGENTS.md（兜底，多数 harness 认 AGENTS.md）

规矩：
- 块嵌式 <!-- memlite:begin --> … <!-- memlite:end -->，重复跑幂等。
- 冲突检测：目标文件已有 respire/1memory/rsrs 块则拒绝注入（双记忆系统同文件
  注入有「双份铁律致遗忘」前科），--force 可强注。
- 注入前先读对方解析方式（教训 f348072e）：此处全部走标准 markdown 文件，
  不碰各家私有 jsonc 配置。
"""

from __future__ import annotations

import os

BEGIN = "<!-- memlite:begin -->"
END = "<!-- memlite:end -->"

# 冲突标记：命中任一即认为文件里已有另一套记忆系统
CONFLICT_MARKERS = ("respire", "1memory", "rsrs recall", "rsrs remember", "忆时")

TARGETS: dict[str, str] = {
    # qoder 不在表内：其 rules 全文由 ~/.qoder/rules/xiaoyu-default.md 手工管理，
    # 再注入 memlite.md 会成第二份 always_on 规则，双份铁律致遗忘（前科 f2886a99）。
    "claude": "~/.claude/CLAUDE.md",
    "codex": "~/.codex/AGENTS.md",
    "codebuddy": "~/.codebuddy/CODEBUDDY.md",
    "workbuddy": "~/.workbuddy/MEMORY.md",
    "pi": "~/.pi/AGENTS.md",
    "kylinbot": "~/.kylinbot/workspace/AGENTS.md",
    "generic": "~/AGENTS.md",
}


def rules_text(data_dir: str, lite: bool = False) -> str:
    """注入正文。lite=True 时只写核心规则+全量路径（workbuddy 4000 字符上限适配）。"""
    abs_data = os.path.abspath(os.path.expanduser(data_dir))
    head = (
        f"{BEGIN}\n"
        "# memlite 记忆系统\n\n"
        f"本机记忆库：`{abs_data}`（明文 JSONL，GitHub 私有仓同步）。\n\n"
        "## 铁律\n\n"
        "1. 答前先查：`memlite recall \"项目名+关键词\"`——未查不开答。\n"
        "2. 收尾必存：新事实/决策/教训 `memlite remember \"内容\" --title \"…\"`。\n"
        "3. --title 必须写事实句（主语＋动作＋关键实体或结果，如「nginx 反代 504 调 timeout 解决」），\n"
        "   禁只写主题分类名（如「运维记录」）——检索靠标题与正文词面，分类名等于没写。\n"
        "4. 删改用 `memlite show <id>` 确认后 `memlite forget <id>`，禁直接改数据文件。\n"
        "5. 同步：`memlite sync`（只 add 白名单数据文件，push 后核验 sha）。\n"
        "6. 数据仓是私有仓假设；禁止把 token/密钥写进记忆正文。\n"
    )
    if lite:
        return head + f"全量规则见 `{abs_data}/../memlite/rules.md`。\n{END}\n"
    return head + END + "\n"


def _expand(p: str) -> str:
    return os.path.abspath(os.path.expanduser(p))


def check_state(path: str, force: bool = False) -> str:
    """fresh=已最新 / absent=未装或未注入 / conflict=他系已占 / stale=旧版块。"""
    if not os.path.isfile(path):
        return "absent"
    try:
        text = open(path, encoding="utf-8", errors="replace").read()
    except OSError:
        return "absent"
    has_block = BEGIN in text and END in text
    has_conflict = any(m in text for m in CONFLICT_MARKERS) and not _own_block_only(text)
    if has_conflict and not has_block:
        return "conflict"
    if has_block:
        return "fresh" if text.count(BEGIN) == 1 else "stale"
    return "absent"


def _own_block_only(text: str) -> bool:
    """整文件只含 memlite 自己的块（如 qoder 的 memlite.md），不算冲突。"""
    return BEGIN in text and text.strip().startswith(BEGIN)


def inject(target: str, data_dir: str, force: bool = False, lite: bool = False) -> dict:
    """向单个目标注入，返回 {target, path, state, changed}。"""
    if target not in TARGETS:
        raise KeyError(f"未知目标: {target}（可选 {', '.join(TARGETS)}）")
    path = _expand(TARGETS[target])
    state = check_state(path, force)
    if state == "conflict" and not force:
        return {"target": target, "path": path, "state": state, "changed": False,
                "hint": "目标文件已有 respire/1memory 规则，拒绝双注入；确需则 --force"}
    os.makedirs(os.path.dirname(path), exist_ok=True)
    block = rules_text(data_dir, lite=lite)
    if os.path.isfile(path):
        text = open(path, encoding="utf-8").read()
        if BEGIN in text and END in text:
            pre = text.split(BEGIN)[0]
            post = text.split(END, 1)[1].lstrip("\n")
            new = pre + block + (("\n" + post) if post else "")
        else:
            sep = "" if text.endswith("\n\n") else ("\n" if text.endswith("\n") else "\n\n")
            new = text + sep + block
    else:
        new = block
    changed = new != (open(path, encoding="utf-8").read() if os.path.isfile(path) else "")
    if changed:
        tmp = path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            f.write(new)
        os.replace(tmp, path)
    return {"target": target, "path": path, "state": state, "changed": changed}


def inject_all(data_dir: str, force: bool = False) -> list[dict]:
    """探测全部目标：absent 写入，fresh 跳过，conflict 跳过（除非 force）。"""
    results = []
    for t in TARGETS:
        path = _expand(TARGETS[t])
        state = check_state(path, force)
        if state == "absent" or (state == "conflict" and force) or state == "stale":
            results.append(inject(t, data_dir, force=force))
        else:
            results.append({"target": t, "path": path, "state": state, "changed": False})
    return results


def probe() -> list[dict]:
    """只探测不写入（UI/CLI 预览用）。"""
    return [
        {"target": t, "path": _expand(p), "state": check_state(_expand(p))}
        for t, p in TARGETS.items()
    ]


def remove(target: str) -> dict:
    """移除目标文件中的 memlite 块。"""
    path = _expand(TARGETS[target])
    if not os.path.isfile(path):
        return {"target": target, "path": path, "changed": False}
    text = open(path, encoding="utf-8").read()
    if BEGIN not in text:
        return {"target": target, "path": path, "changed": False}
    pre = text.split(BEGIN)[0].rstrip("\n")
    post = text.split(END, 1)[1].lstrip("\n") if END in text else ""
    new = "\n".join(x for x in (pre, post) if x)
    if new and not new.endswith("\n"):
        new += "\n"
    with open(path, "w", encoding="utf-8") as f:
        f.write(new)
    return {"target": target, "path": path, "changed": True}
