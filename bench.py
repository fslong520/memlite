"""memlite 检索命中率评测：20 条记忆 × 20 查询，报 hit@1 / hit@3。

记忆与查询取自真实使用形态（中英混合、部分查询不含正文原词），
如实反映检索在小库上的能力边界。改检索参数后重跑对比。

  python3 bench.py             # 词面 BM25（零依赖基线）
  python3 bench.py --semantic  # 语义档（须配嵌入 API，真实打 API 算向量）
  python3 bench.py --semantic --fake  # 语义档 mock（调试管道用，非真实命中率）
"""

from memlite.search import search, semantic_search
from memlite.store import Store

import shutil
import sys
import tempfile

# (记忆 title, content, 查询) —— 查询按用户自然提问写，不为刷分贴原词
CORPUS = [
    ("Python 3.12 发布新解释器", "Python 3.12 正式发布，新解释器性能提升 10%，f-string 语法限制放宽", "python 最新版出了什么"),
    ("nginx 反代超时排查", "nginx 反向代理 upstream 504，最终是 proxy_read_timeout 默认 60s 太短，调到 300s 解决", "网关 504 超时怎么处理"),
    ("SQLite WAL 模式开启", "SQLite 多进程读写冲突，开启 WAL journal_mode 后并发读不再互相阻塞", "数据库多进程读写冲突"),
    ("React 18 createRoot 迁移", "React 18 迁移要把 ReactDOM.render 换成 createRoot，否则并发特性不生效", "react 升级后渲染方式要改吗"),
    ("Docker 磁盘占满清理", "docker system df 查看占用，prune -a 清理无容器引用的镜像，回收 20G", "容器磁盘满了怎么清理"),
    ("Git 合并保留两侧修改", "git merge 冲突时用 git checkout --ours/--theirs 分别取本侧与对侧文件", "版本冲突两边代码都要怎么办"),
    ("Redis 内存持续增长", "Redis 大 key 未设过期时间导致内存涨到 8G，用 scan 找出后逐个设置 ttl", "缓存内存一直涨不释放"),
    ("SSH 免密登录配置", "ssh-keygen 生成密钥后 ssh-copy-id 追加到服务端 authorized_keys 即可免密", "每次登录都要输密码"),
    ("PostgreSQL 连接池耗尽", "pgbouncer 配置 transaction 模式连接池，应用侧 max_connections 从 200 降到 50", "数据库连接数被打满"),
    ("Vim 批量替换文本", "vim 里 %s/old/new/g 全文替换，先用 %s/old/new/gn 预览匹配数", "编辑器怎么全文查找替换"),
    ("CSS Grid 自适应布局", "grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)) 实现免媒体查询的响应式", "不写媒体查询做自适应"),
    ("TypeScript 类型断言", "TS 里 as const 把字面量收窄为精确类型，避免联合类型过宽", "typescript 字面量类型收窄"),
    ("K8s Pod 一直 Pending", "Pod Pending 是资源不足，describe 看 events 发现 CPU requests 超过节点可分配", "容器编排任务卡住不动"),
    ("Linux 查看端口占用", "ss -tlnp 列出监听端口及进程，比 netstat 快且不用 root 看全", "哪个程序占了端口"),
    ("Rust 编译速度优化", "cargo build 慢用 sccache 加速二次编译，workspace 拆分减少重复编译", "rust 项目编译太慢"),
    ("Go goroutine 泄漏排查", "pprof 的 goroutine profile 找到泄漏协程栈，根因是未关闭的 channel 消费者", "golang 内存缓慢上涨"),
    ("Nginx HTTPS 证书更新", "certbot renew 自动续期，nginx reload 后用 openssl s_client 验证新证书有效期", "证书到期怎么换"),
    ("MySQL 慢查询定位", "slow_query_log 开启后用 mysqldumpslow 排序，发现缺索引的联合查询", "数据库查询很慢怎么查"),
    ("Terraform 状态文件锁", "terraform state 锁定用 DynamoDB 避免两人同时 apply 互相覆盖", "基础设施并发操作冲突"),
    ("前端构建产物过大", "vite 打包分析 vite-bundle-visualizer，发现某库被全量引入，改按需导入后减 400K", "打包体积太大怎么分析"),
]


def _fake_embed(text: str, dim: int = 64) -> list[float]:
    """确定性 mock 向量：token 哈希进桶。只用于调试管道，不代表真实语义质量。"""
    from memlite.search import tokenize

    v = [0.0] * dim
    for tok in tokenize(text):
        v[hash(tok) % dim] += 1.0
    return v


def main():
    use_semantic = "--semantic" in sys.argv
    fake = "--fake" in sys.argv
    d = tempfile.mkdtemp(prefix="memlite-bench-")
    try:
        client = None
        if use_semantic and not fake:
            from memlite import embed as embedm

            client = embedm.get_client()
            if client is None:
                print("未配置嵌入 API；调试管道可用 --semantic --fake")
                sys.exit(2)
        embed = _fake_embed if fake else (lambda t: client.embed(t))

        s = Store(d)
        from memlite.vectors import VectorIndex
        vidx = VectorIndex(d)
        ids = []
        for title, content, _ in CORPUS:
            rid = s.put(content, title=title, tags=[])
            if use_semantic:
                vidx.put(rid, embed(f"{title}\n{content}"))
            ids.append(rid)
        hits1 = hits3 = 0
        misses = []
        for i, (title, content, query) in enumerate(CORPUS):
            if use_semantic:
                got = semantic_search(s.all(), embed(query), limit=3, vec_getter=vidx.get)
            else:
                got = search(s.all(), query, limit=3)
            top_ids = [r["id"] for _, r in got]
            if top_ids and top_ids[0] == ids[i]:
                hits1 += 1
            if ids[i] in top_ids:
                hits3 += 1
            else:
                misses.append(query)
        n = len(CORPUS)
        mode = "semantic" + ("(fake)" if fake else "") if use_semantic else "bm25"
        print(f"模式: {mode} | 库容: {n} 条 | 查询: {n} 个")
        print(f"hit@1: {hits1}/{n} = {hits1/n:.0%}")
        print(f"hit@3: {hits3}/{n} = {hits3/n:.0%}")
        if misses:
            print("未进 top3 的查询：")
            for q in misses:
                print(f"  - {q}")
    finally:
        shutil.rmtree(d, ignore_errors=True)


if __name__ == "__main__":
    main()
