# memlite — 可嵌入的本地记忆引擎

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

**单文件二进制（~27MB，Rust，零运行时依赖——静态链入 ONNX Runtime，端侧语义档一体）**：把它放进你的产品，用 CLI / HTTP / 数据文件三契约任一接入，用户即拥有完整的本地记忆——语义+词面双档检索、git 同步、项目级/用户级两档、敏感闸、Web 管理台、三档注入（rules/memory/AGENTS.md 覆盖 24 个国内外主流 AI 产品，`memlite inject --emit` 一键出拷贝引导表）。

> **嵌入指南：[docs/EMBEDDING.md](docs/EMBEDDING.md)** —— 三条接入路径、完整 API 表、退出码契约、分发清单。
> **注入指南：[docs/INJECT.md](docs/INJECT.md)** —— 三档 24 产品矩阵、注入提示词（正文与模板）、命令与防呆。

快速体验：

```bash
cargo build --release --manifest-path rust/Cargo.toml   # 或拿现成二进制
memlite init                      # 建库
memlite remember "第一条记忆" --title "事实句标题"
memlite recall "查询"             # 语义/词面自动选档
memlite web --port 7879           # 管理台 http://127.0.0.1:7879/
```

明文 JSON 存储 + GitHub 私有仓同步 + rules 注入。为低配设备设计：零重型依赖、常驻进程仅可选的 `embed-server` 与 `web`（均只绑回环）、无应用层加密。

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

单页管理界面：语义搜索（两档自动回退）、详情/删除、新增（带敏感闸）、一键 git 同步，另有时间线、标签·项目、注入、检索日志页。零框架手写 HTTP，**只监听 127.0.0.1**（明文库不对外），写接口做同源 Origin 校验防 CSRF。

## 端侧嵌入服务（零外网语义档）

```bash
memlite embed-server --port 18321    # bge-m3 ONNX 量化版，OpenAI 兼容 /v1/embeddings
```

单二进制内嵌 ONNX Runtime（ort 静态链入）。模型目录三级回退：`MEMLITE_MODEL_DIR` →
`~/.rsrs/models/bge-m3` → `/usr/lib/respire/models/bge-m3`；推理线程默认 min(cpu/2, 4)，
`MEMLITE_EMBED_THREADS` 覆盖。systemd 用户单元 `ExecStart=…/memlite embed-server` 即开机自启。

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
memlite remember "内容" --parent <父id/纲名>   # 认因挂载：父子即因果（显式挂载不判存）
memlite recall "查询"                 # 自动选档；--bm25 强制词面
memlite reembed                       # 给旧记忆回填向量（配了 API）
memlite show <id> / list / forget <id>
memlite update <id> --title "新标题" --content "改后正文"   # 就地改，未传字段保留（因链随迁）
memlite attach <子> --parent <父>     # 收编挂链（id/前8位/纲名皆可，防环防自挂）
memlite demote <子> --parent <父>     # 因果下沉：挂到父下成为其果（同 attach，下游果链随迁）
memlite promote <子>                  # 因果上浮：升为更上游之因（挂到祖父/根，果链随之）
memlite chain <id> [--depth N]        # 因果链深挖：因链全文（根在前）+ 本条 + 果链下探 N 层
memlite tree [--from <id>] [--depth N] [--outline]   # 因果树：森林/子树；--outline 全深目录
memlite sync                          # git 同步（白名单 add + sha 双通道核验）
memlite status
memlite inject --probe                # 看各软件 rules 注入点现状
memlite inject --print                # 只打印块文本（手动粘进任意文件），不写盘
memlite inject --all                  # 注入（正文取 Qoder rules 全文；冲突自动跳过，--force 强注）
memlite bench [--semantic]            # 检索命中率评测（临时库，报 hit@1/hit@3）
memlite querylog                      # 检索日志汇总：次数、档位、零命中、高命中 top
memlite querylog --recent 20          # 最近 20 条查询流水
memlite querylog --export train.jsonl # 导出训练集（每行一次查询：query/positive/hits）
```

标题规则（影响检索命中率，实测见下）：`--title` 必须写事实句——主语＋动作＋关键实体或结果；禁只写主题分类名。缺省则截正文首句 ≤40 字。

## 因果树与因果链（对齐 respire）

父=因、子=果，`parent_id` 随条入库（老数据迁移保留）。`recall`/`show` 命中即见前因后果：🔺因链标题链 → ▸纲L1 父内容头 → 🔻果（子条摘要＋孙标题）。数据语义同 respire——琐事（`trivial`）出树、父失效（琐事/已删）者提为根，不产悬空。

```bash
memlite chain 8744409e --depth 2    # 一把取回整条链，免逐层 show
memlite tree --depth 3               # 全部根森林（├─ 树形，含 kind 与叶标记）
memlite tree --from 8744409e         # 从某节点看子树
memlite tree --outline               # 全深 id+标题目录（AI 梳理先读此）
```

改挂防环：`attach`/`demote` 拒自挂、拒挂自己后代（新父因链含本条即拦）；`promote` 升根果链随迁。所有命令带 `--json` 结构化输出。Web 侧「记忆 · 因果树」页签可下钻，详情页显示🔺因链/🔻果并可点击跳转。

## 检索命中率（memlite bench）

词面档 20 条真实形态记忆 × 20 改写型查询：**hit@1 85%、hit@3 85%**；语义档 **hit@1 85%、hit@3 90%**。漏检全部是查询与正文零词面重合的纯改写——这正是语义档的适用场景；重跑 `memlite bench` / `memlite bench --semantic` 对比。

## 检索日志（query_log，训练/复盘取材）

每次 `recall`（CLI）与 Web 检索落一行进 `memory.querylog.jsonl`，含零命中与回退：

```json
{"ts":1792078629.79,"query":"检索日志怎么导出","backend":"semantic","limit":10,"fallback":"","source":"cli","ms":17,"hits":[{"id":"6f557054ce71","score":0.657,"title":"…"}]}
```

- `backend` semantic/bm25，`fallback:"user"` 标项目级回退，`source` cli/web，`ms` 端到端耗时（含回退那次）。
- 每条记忆的**被召回次数**（recall_count）不另设文件——全量聚合自日志，Web 详情页显示，`memlite querylog` 出高命中 top。
- 导出训练集：`memlite querylog --export train.jsonl`（每行 `{ts,query,backend,limit,positive:[ids],hits:[{id,score,title}]}`，`--since <unix秒>` 截窗）。
- 文件名过同步白名单 `memory*.jsonl`，随 `memlite sync` 走；`bench` 评测查询不入日志（避免污染训练材料）。

## 数据仓布局

```
memlite-data/
├── memory.jsonl             # 追加日志，一行一条（put/del）
├── memory.querylog.jsonl    # 检索日志，一行一次查询（recall/网页检索）
├── meta.json                # 预留
└── .gitignore               # 凭据类一律排除
```

`.gitignore` 已排除 `.env`、`*.token`、`session*`、`secret*`、`*.key`、`.git-credentials`。同步只 add 白名单文件，从不 `git add -A`。

## 注入（三档 24 产品）

| 档 | 方式 | 代表目标 | 命令 |
|---|---|---|---|
| ① rules 档（8 家） | 不自动写，`--emit` 出独立文件＋拷贝表 | Qoder/Cursor/Windsurf/Zed/Cline/Roo… | `memlite inject --emit` |
| ② memory 档（5 家） | 块嵌 `<!-- memlite:begin/end -->` | CLAUDE.md/GEMINI.md/copilot-instructions… | `memlite inject --target claude` |
| ③ agents 档（11 家） | 块嵌 | Codex/opencode/DeepSeek/Qwen/豆包…/兜底 `~/AGENTS.md` | `memlite inject --all` |

**注入提示词正文** = 注入源 `~/.qoder/rules/AGENTS.md` 全文（单一事实源，改规则只改它，
重跑注入逐字刷新；读不到/为空即报错拒注）。目标已有 respire/1memory 规则时拒绝双注入
（`--force` 强注），重复注入幂等。没有 Qoder 规则文件的用户，先按
[docs/INJECT.md](docs/INJECT.md) 的最小模板落一份再注入；Qoder 自身不注块——
该文件就是注入源，再嵌即双 always_on。

**手动注入与修改**：`memlite inject --print` 出块文本随你粘进任意文件；
仓库附带现成的 [docs/memlite-rules.md](docs/memlite-rules.md)（与 `--emit` 逐字一致，单测守护），
下载手改即可；块外内容永远安全，块内以注入源为准（重跑按源刷新，想留住自定义就改注入源或改后不再重跑）——细则见 [docs/INJECT.md](docs/INJECT.md)。

## 协议

MIT，见 [LICENSE](LICENSE)。数据仓是**你的私有数据**：本仓开源，用户的 `memory.jsonl` 不在此列——同步目标必须是 private 仓库，转 public 即安全假设失效。
