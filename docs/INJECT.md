# memlite 注入指南——把记忆工作流写进 24 个 AI 产品

`memlite inject` 把「先查记忆再作答、收尾落库」的工作流提示词写进各 AI 载体的
rules/记忆文件。按载体能力分三档，国内外主流产品通吃。

## 三档策略

| 档 | 适用载体 | 方式 | 怎么用 |
|---|---|---|---|
| ① rules 档（8 家） | 有专用 rules 机制（格式/frontmatter 各异） | **不自动写**，`--emit` 生成独立 rules 文件＋逐产品拷贝引导，按表 `cp` | `memlite inject --emit [FILE]` |
| ② memory 档（5 家） | 有长期记忆文件（CLAUDE.md 等） | 自动块嵌追加（幂等、带冲突检测） | `memlite inject --target claude` |
| ③ agents 档（11 家） | AGENTS.md 事实标准 | 自动块嵌追加 | `memlite inject --target codex` 或 `--all` |

## 注入提示词（正文）

两份正文，各有其用：

**1. 块嵌正文（`--target` / `--all`）= 注入源全文。**
单一事实源是 `~/.qoder/rules/AGENTS.md`——`inject` 逐字读取它，包进块标记写入目标：

```markdown
<!-- memlite:begin -->
（注入源文件全文，逐字嵌入）
<!-- memlite:end -->
```

- 改规则只改注入源，重跑 `--target`/`--all` 即全量刷新（逐字一致）。
- 注入源读不到/为空 → **报错拒注**（不静默回退旧文本）。
- 目标文件已有 respire/1memory/忆时 规则 → 拒绝双注入，`--force` 可强注。
- 重复注入幂等；`--remove <名>` 移除块。

**2. emit 正文（`--emit`）= 通用独立 rules 文件。**
自带的通用版本（无本机路径，适合 rules 档整文件拷贝与离线批量部署），
`memlite inject --emit` 生成 `memlite-rules.md` 并打印三档逐产品拷贝命令表
（rules 档 `cp`，memory/agents 档 `cat >>`），**不改任何他方文件**。

### 最小注入源模板

新用户（没有 Qoder 规则文件）先落这份，再执行 inject：

```markdown
# memlite 记忆工作流

总纲：凡开口先查，凡动手必查，凡所得可录，凡碰壁翻忆。

## 铁锚 · 三行

1. **答前必查**：`memlite recall "项目名+关键词"`——未翻忆不开答（项目级库优先，无命中自动回退用户级）。
2. **收尾必存**：新事实/决策/教训 `memlite remember "内容" --title "事实句"`——未落库不算完。
3. **遇障先翻忆**：报错、异常先 `memlite recall "<报错原文片段>"` 再动手——未查不动手、不试错。

## 规则

- `--title` 必须是事实句（主语＋动作＋关键实体或结果），禁只写主题分类名。
- 改旧条用 `memlite update <id>`（未传字段保留）；合并先 `show <id>` 再 `forget` + `remember`；禁直接改数据文件。
- 同步走 `memlite sync`（只 add 白名单数据文件，push 后双通道核验 sha）；数据仓须为 private 仓库。
- 敏感信息（口令/证件号/手机号/银行卡/token/私钥）禁入记忆正文与查询词，只写其存放位置。
```

保存为 `~/.qoder/rules/AGENTS.md` 后：

```bash
memlite inject --probe    # 看各目标现状（absent/fresh/stale/conflict）
memlite inject --all      # memory/agents 档批量块嵌
memlite inject --emit     # rules 档生成独立文件＋拷贝引导表
```

## 目标矩阵（24 产品）

### ① rules 档——`--emit` 拷贝引导，不自动块嵌

| 名称 | 路径 | 备注 |
|---|---|---|
| qoder | `~/.qoder/rules/memlite.md` | 注入源即 Qoder rules，勿再拷——双 always_on 互相冲淡 |
| cursor | `.cursor/rules/memlite.mdc` | 项目仓内；头部加 frontmatter `alwaysApply: true` |
| windsurf | `.windsurf/rules/memlite.md` | 项目仓内；文件头加 frontmatter |
| claude-rules | `~/.claude/rules/memlite.md` | 与 CLAUDE.md 块嵌二选一，勿双注 |
| zed | `~/.config/zed/rules.md` | Zed 全局 rules |
| cline | `.clinerules` | 项目仓根单文件 |
| roo | `.roorules` | 项目仓根单文件 |
| trae | `.trae/rules/memlite.md` | 项目仓内 |

### ② memory 档——`--target <名>` 自动块嵌

| 名称 | 路径 | 备注 |
|---|---|---|
| claude | `~/.claude/CLAUDE.md` | Claude Code 长期指令 |
| gemini | `~/.gemini/GEMINI.md` | Gemini CLI 全局 memory |
| copilot | `.github/copilot-instructions.md` | 项目仓内 |
| codebuddy | `~/.codebuddy/CODEBUDDY.md` | |
| workbuddy | `~/.workbuddy/MEMORY.md` | 每轮 4000 字符上限，用 lite 模式（嵌指针不嵌全文） |

### ③ agents 档——`--target <名>` 自动块嵌

| 名称 | 路径 | 备注 |
|---|---|---|
| codex | `~/.codex/AGENTS.md` | OpenAI Codex CLI |
| opencode | `~/.config/opencode/AGENTS.md` | |
| dsh | `~/.dsh/AGENTS.md` | |
| pi | `~/.pi/AGENTS.md` | |
| grok | `~/.grok/AGENTS.md` | Grok CLI |
| deepseek | `~/.deepseek/AGENTS.md` | DeepSeek CLI |
| qwen | `~/.qwen/AGENTS.md` | 通义千问 CLI |
| doubao | `~/.doubao/AGENTS.md` | 豆包 CLI |
| zigcode | `~/.zcode/AGENTS.md` | |
| kylinbot | `~/.kylinbot/workspace/AGENTS.md` | |
| generic | `~/AGENTS.md` | 兜底：多数 harness 认工作目录或家目录 AGENTS.md |

## 手动注入与自定义

注入文件都是**普通文本**，三条手动路径，随你粘贴、随你改：

1. **出块文本手动粘**：`memlite inject --print` 把块嵌正文（`<!-- memlite:begin -->` 外壳＋
   注入源全文）打到 stdout，不写任何文件——粘进任意文件都行（不限目标矩阵：团队 wiki、
   他方工具的自定义指令文件皆可）。
2. **出独立文件手动放**：`memlite inject --emit [FILE]` 生成通用 rules 文件（无块标记），
   按打印出的拷贝表 `cp` / `cat >>` 各目标。
3. **纯手抄**：就用上面的最小模板，自己贴进目标文件。

**修改规则（重要）**：

- 块**外**的原有内容永远安全——注入是追加/替换块模型，动不到你文件的其他部分。
- 块**内**以注入源为准：重跑 `--target`/`--all` 会按 `~/.qoder/rules/AGENTS.md`
  **逐字刷新块内**，手改会被覆盖。想让自定义留住：
  - 首选**改注入源**——一处改，重跑注入全量刷新；
  - 或注入后**手动维护**，不再对这些目标跑 inject（要收回改用 `--remove <名>` 再手动）。
- `--probe` 只看块标记数量（一次 BEGIN/END），检不出块内手改——对账用
  `memlite inject --print | diff - <目标文件>` 或 git。
- `--force` 只解「目标已有他家记忆系统」的冲突拒注，不改变上述刷新语义。

## 命令一览

```bash
memlite inject --probe              # 探测各目标现状（不写任何文件）
memlite inject --print              # 只打印块文本到 stdout（手动粘贴/自定义用，不写文件）
memlite inject --target <名>        # 单目标块嵌（幂等）
memlite inject --all                # memory+agents 档全部自动目标
memlite inject --emit [FILE]        # 生成独立 rules 文件＋逐产品拷贝表
memlite inject --remove <名>        # 移除该目标内的 memlite 块
memlite inject --target <名> --force   # 目标已有他家记忆系统时强注
```

真机验证：`memlite inject --probe` 回执标 `fresh`，或
`grep -c 'memlite:begin' <目标路径>` = 1。
