# memlite — 简化嵌入式记忆系统

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

## 命令

```bash
python3 -m memlite init                          # 初始化数据仓与配置
python3 -m memlite remember "内容" --title "事实句标题"
python3 -m memlite recall "查询"                 # 自动选档；--bm25 强制词面
python3 -m memlite reembed                       # 给旧记忆回填向量（配了 API）
python3 -m memlite show <id> / list / forget <id>
python3 -m memlite sync                          # git 同步（白名单 add + sha 双通道核验）
python3 -m memlite status
python3 -m memlite inject --probe                # 看各软件 rules 注入点现状
python3 -m memlite inject --all                  # 注入（冲突自动跳过，--force 强注）
```

标题规则（影响检索命中率，实测见下）：`--title` 必须写事实句——主语＋动作＋关键实体或结果；禁只写主题分类名。缺省则截正文首句 ≤40 字。

## 检索命中率（bench.py）

词面档 20 条真实形态记忆 × 20 改写型查询：**hit@1 85%、hit@3 85%**。漏检全部是查询与正文零词面重合的纯改写——这正是语义档的适用场景；配 API 后重跑 `python3 bench.py --semantic` 对比。

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
| Qoder | `~/.qoder/rules/memlite.md` | 独立规则文件（rules 是 Qoder 唯一全文载体） |
| Claude Code | `~/.claude/CLAUDE.md` | 块嵌 `<!-- memlite:begin/end -->` |
| Codex | `~/.codex/AGENTS.md` | 块嵌 |
| CodeBuddy | `~/.codebuddy/CODEBUDDY.md` | 块嵌 |
| WorkBuddy | `~/.workbuddy/MEMORY.md` | 块嵌（精简版，4000 字符上限内） |
| pi | `~/.pi/AGENTS.md` | 块嵌 |
| kylinbot | `~/.kylinbot/workspace/AGENTS.md` | 块嵌 |
| 兜底 | `~/AGENTS.md` | 块嵌（多数 harness 认 AGENTS.md） |

目标文件已有 respire/1memory 规则时拒绝注入（防双记忆系统同文件打架），`--force` 可强注。重复注入幂等。
