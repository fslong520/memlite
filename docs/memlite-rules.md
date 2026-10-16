# memlite 记忆工作流

本机记忆库由 memlite 管理（明文 JSONL，GitHub 私有仓同步）。CLI：`memlite <命令>`。

总纲：**凡开口先查，凡动手必查，凡所得可录，凡碰壁翻忆。**

## 铁锚 · 三行

1. **答前必查**：`memlite recall "项目名+关键词"`——未翻忆不开答。项目级库优先（工作区含 config.json 的 `.memlite/`），没搜到 CLI 自动回退用户级。
2. **收尾必存即推**：新事实/决策/教训 `memlite remember "内容" --title "事实句"` 落库后紧接 `memlite sync`——未推上远端不算完。
3. **遇障先翻忆**：报错、异常先 `memlite recall "报错原文片段"` 再动手——未查不动手、不试错。

## 流程

1. **开口先查**：`memlite recall "<项目名+核心词>"`（缺省 top10；`--json` 结构化）。两档皆空换 2–3 组词再查。
2. **所得必存**：先 recall 判重 → `memlite show <id>` 读候选 → 改＞并＞存：旧条更新走 `memlite update <id> --content/--title`（未传字段保留原值）；同题散条合成综合版后 forget 旧条；两轮换词皆空才新存。一事一条。
3. **标题铁律**：`--title` 必须是事实句（主语＋动作＋关键实体或结果），禁只写主题分类名。
4. **敏感闸**：口令/证件号/手机号/银行卡/token/私钥禁入正文与查询词，只写存放位置（如「密码在 ~/.env」）——CLI 写前自动扫描，命中即拒。
5. **同步（存后即跑）**：`remember`/`forget`/`update` 落库后紧接 `memlite sync`，收尾再跑一次；数据仓须为 private 仓库。sync 内置白名单 add（`memory*.jsonl`／`meta.json`／`README.md`，禁 `git add -A`）→ commit → pull --rebase → push → 双通道核验 local/remote sha，回执须 `verified:true` 方算同步。失败分诊：**瞬时错**（index.lock、并发 git 互踩、push 超时）隔 30s 重试——sync 先 commit 后推，失败不丢数据；**真冲突**（rebase 残留 `UU`＋HEAD 游离，另台设备推过同区内容）走并集三步，任一不过即停手、禁 force 禁 drop——① `git merge-base` 取共同基底，验两侧对基底只增不删不改；② 两侧新增行 id 零交集（同 id 双改须人工裁决）；③ 过①②写并集（上游侧全量＋本地侧不重复行）→ `git add` → `GIT_EDITOR=true git rebase --continue` → 重跑 sync 取 `verified:true`，终验两侧新增 id 全在、无 `<<<<<<<` 标记残留。`memory.querylog.jsonl` 无 id 键，冲突按行并集去重即可。
6. **管理台**：`memlite web --port 7879` 起本地回环管理页（语义检索/增删改/时间线/标签/版本历史/注入）。
