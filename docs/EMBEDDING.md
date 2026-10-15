# 把 memlite 嵌进你的产品

memlite 是**可嵌入的记忆引擎**：一个单文件二进制（~27MB，Rust 编译，零运行时依赖，
静态链入 ONNX Runtime——端侧语义档一体），你的产品把它带在身边，用三种契约之一接入，
用户就拥有完整的本地记忆能力。

```
你的产品
 ├── bin/memlite            ← 随包分发（27MB 单文件，零运行时依赖）
 │     ├── CLI 契约          ← 任何语言 shell out 即可（推荐，最稳）
 │     ├── HTTP 契约         ← `memlite web` 起本地回环服务
 │     └── 数据契约          ← memory.jsonl 开放格式，直接读写也行
 └── .memlite/              ← 项目级记忆文件夹（随你的产品仓一起 git 同步）
```

## 三条接入路径

### 1. CLI 契约（推荐）

跨语言、跨平台、无常驻进程。所有读写命令支持 `--json`，stdout 只出一个 JSON，
错误走 stderr + 非零退出码——直接当结构化 API 用。

```bash
memlite init                                  # 初始化（数据目录 + 用户配置）
memlite remember "内容" --title "事实句" --json
memlite recall "查询" --limit 10 --json        # 语义/词面自动选档，含 fallback 字段
memlite show <id> --json
memlite list --limit 50 --json
memlite update <id> --title "新标题" --content "改后正文" --json   # 只传要改的字段，其余保留
memlite forget <id> --json
memlite sync --json                           # git 同步，sha 双通道核验
memlite status --json                         # 条数/向量/文件/sync 状态
```

退出码约定：`0` 成功；`1` 条目不存在；`2` 未配置（如缺嵌入 API）；`3` 敏感信息拒入。

**记住一个解析序**（你的产品不必自己实现）：
`--data-dir` > 环境变量 `MEMLITE_DATA` > 工作区 `.memlite/`（含 config.json，向上找不越 git 根）
> 用户级 `~/.memlite/config.json` 的 `data_dir` > 默认。
即：**项目级优先，用户级兜底**——recall 在项目级空时自动回退用户级（JSON 回执标 `fallback:"user"`）。

### 2. HTTP 契约（要 UI 就用它）

```bash
memlite web --port 7879    # 只绑 127.0.0.1，明文库不对外
```

自带管理单页（respire 同款设计语言）：语义搜索、列最近分页、时间线、标签云、
增删改查、版本历史、git 同步、导出、注入管理。API：

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/status` | 库状态（data_dir/条数/分层/嵌入配置） |
| GET | `/api/search?q=&limit=` | 检索（`backend` 标语义/词面，`fallback` 标两档回退） |
| GET | `/api/list?limit=&offset=` | 列最近（分页） |
| GET | `/api/show?id=` | 单条全文（附 `recall_count`：该条被召回次数） |
| GET | `/api/querylog?limit=&top=` | 检索日志：`stats`（次数/档位/零命中/高命中 top）＋`recent` 流水 |
| GET | `/api/versions?id=` | 同 id 全部历史版本（append-only 日志天然保历史） |
| GET | `/api/tags` · `/api/projects` | 标签/项目频次 |
| GET | `/api/inject` | 各 AI 软件注入点探测 |
| POST | `/api/inject` | `{target, remove?, force?}` 注入/移除规则 |
| GET | `/api/export` | 全库 JSON 下载 |
| GET | `/api/config` | 合并配置（api_key 脱敏） |
| POST | `/api/remember` | `{content, title?, tags?}`，走敏感闸 |
| POST | `/api/update` | `{id, title?, content?, tags?}` |
| POST | `/api/forget` | `{id}` |
| POST | `/api/sync` | git 同步 |
| POST | `/api/compact` | 压实（只留存活重写单文件） |
| POST | `/api/reembed` | 增量回填向量（须嵌入 API 在配） |

安全：写接口校验 Origin 只认回环（防浏览器任意站点 CSRF 打本地端口）；
敏感信息（明文口令/身份证/手机号/银行卡/JWT/PEM/token 前缀）写入即 400 拒绝。

### 3. 数据契约（最轻）

`memory.jsonl` 是开放的 append-only JSONL，一行一条操作：

```json
{"op":"put","id":"a1b2c3d4e5f6","ts":1791954151.87,"title":"事实句","content":"正文","tags":[],"project":""}
{"op":"del","id":"a1b2c3d4e5f6","ts":1791960000.0}
```

- 末写胜出：同 id 的后行覆盖前行；`del` 是墓碑。
- 单文件超 16MB 自动轮转为 `memory-0001.jsonl`，加载按序回放，语义不变。
- 向量是派生数据：`emb.f32`（packed float32 LE）+ `emb-ids.txt`（顺序对齐），
  **不入 git**，clone 后 `memlite reembed` 重建。

你可以不调 CLI 直接读写这个格式——只要遵守上述不变式。

## 项目级记忆（随你的产品仓走）

在产品仓里放一个含 `config.json` 的 `.memlite/` 文件夹（空 `{}` 即可识别）：

```
your-product/
├── .memlite/
│   ├── config.json        ← 存在即被识别为项目级记忆库
│   ├── memory.jsonl       ← 记忆正文（git 同步）
│   ├── .gitignore         ← 排除 emb.f32 / emb-ids.txt（派生数据）
│   └── emb.f32            ← 向量侧车（不入库）
└── src/...
```

- 读写自动落项目级；项目级没搜到自动回退用户级。
- `memlite sync` 提交时带**路径限定**（不会把你工作区里已暂存的无关改动卷进提交），
  工作区有半成品时 `--autostash` 自动暂存还原。
- 记忆随产品仓的 git 历史演进——用户的记忆版本和代码版本在一起。

## 语义检索（可选）

不配也完整可用（词面 BM25 兜底）。要语义档：任何 OpenAI 兼容
`/embeddings` 端点（硅基流动、DeepSeek、OpenAI、自建 vLLM/Ollama）：

```bash
memlite config set embedding.api_base https://api.example.com/v1
memlite config set embedding.api_key  sk-...
memlite config set embedding.model     BAAI/bge-m3
```

API key 只存用户级 `~/.memlite/config.json`（永不入数据仓），`config show` 脱敏回显。

### 端侧服务（零外网语义档）

用仓库自带的嵌入服务子命令（源码 `rust/src/embed_server.rs`）：

```bash
cargo build --release                                # 单二进制，ort 静态链入
./target/release/memlite embed-server --port 18321   # OpenAI 兼容 /v1/embeddings
```

**模型**（bge-m3 ONNX，取自 Hugging Face `Xenova/bge-m3`，本地见 `MODEL-SOURCE.json`）——
自动下载不可靠，手动放置：

```
~/.rsrs/models/bge-m3/
├── tokenizer.json
└── onnx/model_quantized.onnx
```

目录三级回退：`MEMLITE_MODEL_DIR` → `~/.rsrs/models/bge-m3` → `/usr/lib/respire/models/bge-m3`。
CLS 池化＋L2 归一化，与旧 Python 版同契约。

**systemd 用户单元（开机自启，`~/.config/systemd/user/memlite-embed.service`）**：

```ini
[Unit]
Description=memlite embed-server (bge-m3 ONNX, port 18321)

[Service]
ExecStart=/usr/local/bin/memlite embed-server --host 127.0.0.1 --port 18321
Environment=MEMLITE_EMBED_THREADS=2
Nice=10
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

```bash
systemctl --user daemon-reload && systemctl --user enable --now memlite-embed
```

**接入与验证一条龙**：

```bash
memlite config set embedding.api_base http://127.0.0.1:18321/v1
memlite config set embedding.model bge-m3-local
memlite config set embedding.api_key local-noauth     # 本机服务不校验，占位即可
curl -s http://127.0.0.1:18321/health                # {"status":"ok",...}
memlite reembed                                       # 老记忆回填向量
memlite recall "查询" --json | head                   # backend 应为 "semantic"
```

推理线程默认 min(cpu/2, 4)，`MEMLITE_EMBED_THREADS` 覆盖。

注意：同一文本在不同推理运行时（如 PyPI onnxruntime wheel 与 ort 发行包）产出的向量
不逐位重合（自余弦≈0.99）——向量是派生数据，切换运行时后用 `memlite reembed` 全量重建，
勿混存两种运行时的向量。

## 分发清单

1. `cargo build --release`（源码在 `rust/`），产物单文件 `target/release/memlite`；
2. 放进你产品的 `bin/`，或依赖 PATH 里的 `memlite`（`GOLDSCALE_*` 类环境变量不必须）；
3. 首次运行 `memlite init`（或什么都不做——写入时自动建目录）；
4. 建议给用户 `memlite web` 的入口——记忆可视化是这类产品的刚需。

版本契约：`memlite --version`；命令面遵循「新命令只增不删、`--json` 字段只增不改」。
