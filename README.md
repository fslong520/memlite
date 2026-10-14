# memlite — 可嵌入的本地记忆引擎

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

**单文件二进制（~2.2MB，Rust，零运行时依赖）**：把它放进你的产品，用 CLI / HTTP / 数据文件三契约任一接入，用户即拥有完整的本地记忆——语义+词面双档检索、git 同步、项目级/用户级两档、敏感闸、Web 管理台、三档注入（rules/memory/AGENTS.md 覆盖 24 个国内外主流 AI 产品，`memlite inject --emit` 一键出拷贝引导表）。

> **嵌入指南：[docs/EMBEDDING.md](docs/EMBEDDING.md)** —— 三条接入路径、完整 API 表、退出码契约、分发清单。

快速体验：

```bash
cargo build --release --manifest-path rust/Cargo.toml   # 或拿现成二进制
memlite init                      # 建库
memlite remember "第一条记忆" --title "事实句标题"
memlite recall "查询"             # 语义/词面自动选档
memlite web --port 7879           # 管理台 http://127.0.0.1:7879/
```

明文 JSON 存储 + GitHub 私有仓同步 + rules 注入。为低配嵌入式设备设计：零重型依赖、无守护进程、无应用层加密。

安全边界 = 数据仓可见性：记忆明文存本地 `memory.jsonl`，push 到用户自建 **private** 仓库。仓库转 public 即安全失效。

## 双档检索

| 档位 | 条件 | 机制 |
|---|---|---|
| 语义 | 配了嵌入 API | 查询与记忆过 API 算向量，余弦排序 |
| 词面 | 没配 API（或 API 失败/无命中） | BM25-lite，中英混合分词（CJK bigram+单字，英文整词），零依赖自动兜底 |

配置（`~/.memlite/config.json`，key 只存本机、永不入数据仓）：

```json
{
  "data_dir": "/path/to/memlite-data",
  "embedding": {
    "api_base": "https://api.siliconflow.cn/v1",
    "api_key": "sk-...",
    "model": "BAAI/bge-m3"
  }
}
```

环境变量 `MEMLITE_API_BASE` / `MEMLITE_API_KEY` / `MEMLITE_EMBED_MODEL` 覆盖之。endpoint 兼容 OpenAI `/embeddings` 契约（硅基流动、DeepSeek、OpenAI、自建 vLLM/Ollama 皆可）。

## 内置 Web 客户端

```bash
memlite web --port 7879    # 打开 http://127.0.0.1:7879/
```

单页管理界面：语义搜索（两档自动回退）、详情/删除、新增（带敏感闸）、一键 git 同步。零框架手写 HTTP，**只监听 127.0.0.1**（明文库不对外），写接口做同源 Origin 校验防 CSRF。

## 项目级记忆（工作区优先）

工作区里有含 `config.json` 的 `.memlite/` 或 `memlite-data/` 文件夹（从 cwd 向上找，不越过 git 根）→ 该项目的记忆读写全部落它，不落用户级库；找不到才回用户级。项目级 data_dir 随工作区 git 一起 `memlite sync`（提交进外层仓）。

## 分层配置

用户级 `data_dir` 可改：`memlite config set data_dir <新路径>`（写 `~/.memlite/config.json`，立即生效）。

`~/.memlite/config.json`（用户级/全局）与项目级文件夹内 `config.json` 合并，项目级覆盖用户级，env 最后覆盖。向量 API 默认写用户级：

```bash
memlite config set embedding.api_base http://127.0.0.1:18321/v1
memlite config set embedding.model bge-m3-local
memlite config set embedding.api_key <key>
memlite config show        # 合并视图（api_key 脱敏）
```

## 敏感闸（默认拒入）

`remember`/`update` 写前扫描：明文口令（含「密码是xxx」口语式）、身份证、手机号、银行卡（Luhn）、JWT、PEM 私钥、常见 token 前缀——命中即拒（exit 3），改存位置指针（如「密码在 ~/.env」）。暂无强制入库口子，宁严勿松。

## 命令

```bash
memlite init                          # 初始化数据仓与配置
memlite remember "内容" --title "事实句标题"
memlite recall "查询"                 # 自动选档；--bm25 强制词面
memlite reembed                       # 给旧记忆回填向量（配了 API）
memlite show <id> / list / forget <id>
memlite update <id> --title "新标题" --content "改后正文"   # 就地改，未传字段保留
memlite sync                          # git 同步（白名单 add + sha 双通道核验）
memlite status
memlite inject --probe                # 看各软件 rules 注入点现状
memlite inject --all                  # 注入（正文取 Qoder rules 全文；冲突自动跳过，--force 强注）
memlite bench [--semantic]            # 检索命中率评测（临时库，报 hit@1/hit@3）
```

标题规则（影响检索命中率，实测见下）：`--title` 必须写事实句——主语＋动作＋关键实体或结果；禁只写主题分类名。缺省则截正文首句 ≤40 字。

## 检索命中率（memlite bench）

词面档 20 条真实形态记忆 × 20 改写型查询：**hit@1 85%、hit@3 85%**；语义档 **hit@1 85%、hit@3 90%**。漏检全部是查询与正文零词面重合的纯改写——这正是语义档的适用场景；重跑 `memlite bench` / `memlite bench --semantic` 对比。

## 数据仓布局

```
memlite-data/
├── memory.jsonl      # 追加日志，一行一条（put/del）
├── meta.json         # 预留
└── .gitignore        # 凭据类一律排除
```

`.gitignore` 已排除 `.env`、`*.token`、`session*`、`secret*`、`*.key`、`.git-credentials`。同步只 add 白名单文件，从不 `git add -A`。

## rules 注入点

| 软件 | 路径 | 方式 |
|---|---|---|
| Qoder | `~/.qoder/rules/AGENTS.md` | **注入源**：inject 读其全文嵌入下方各目标；自身不注块（rules 每会话常驻，再注成双 always_on） |
| Claude Code | `~/.claude/CLAUDE.md` | 块嵌 `<!-- memlite:begin/end -->` |
| Codex | `~/.codex/AGENTS.md` | 块嵌 |
| CodeBuddy | `~/.codebuddy/CODEBUDDY.md` | 块嵌 |
| WorkBuddy | `~/.workbuddy/MEMORY.md` | 块嵌（lite 指针版，4000 字符上限内） |
| pi | `~/.pi/AGENTS.md` | 块嵌 |
| kylinbot | `~/.kylinbot/workspace/AGENTS.md` | 块嵌 |
| 兜底 | `~/AGENTS.md` | 块嵌（多数 harness 认 AGENTS.md） |

注入正文 = `~/.qoder/rules/AGENTS.md` 全文（单一事实源，改规则只改它；读不到/为空即报错拒注）。目标文件已有 respire/1memory 规则时拒绝注入（防双记忆系统同文件打架），`--force` 可强注。重复注入幂等。

## 协议

MIT，见 [LICENSE](LICENSE)。数据仓是**你的私有数据**：本仓开源，用户的 `memory.jsonl` 不在此列——同步目标必须是 private 仓库，转 public 即安全假设失效。
