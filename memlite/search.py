"""检索层：BM25-lite，中英混合分词，零依赖。

分词：ASCII 词按 \w+ 抽取小写；CJK 连续段取 bigram（单字成一 token）。
打分：BM25（k1=1.2, b=0.75），title 权重 ×2。无向量模型——嵌入式设备
不跑 embedding，靠词面匹配；规模到几千条时全量打分毫秒级。
"""

from __future__ import annotations

import math
import re
from collections import Counter

_TOKEN_RE = re.compile(r"[A-Za-z0-9_]+|[一-鿿]+")
_LATIN_RE = re.compile(r"[A-Za-z0-9_]+")
_CJK_RE = re.compile(r"[一-鿿]+")


def tokenize(text: str) -> list[str]:
    """中英混合分词：英文/数字整词小写；CJK 取 bigram 并保单字（单字查询可命中）。"""
    tokens: list[str] = []
    for m in _TOKEN_RE.finditer(text):
        piece = m.group()
        if _LATIN_RE.fullmatch(piece):
            tokens.append(piece.lower())
        else:
            tokens.extend(piece)  # 单字
            if len(piece) > 1:
                tokens.extend(piece[i : i + 2] for i in range(len(piece) - 1))  # bigram
    return tokens


def _bm25_score(
    qtf: Counter,
    doc_tf: Counter,
    doc_len: int,
    avg_dl: float,
    df: Counter,
    n_docs: int,
) -> float:
    k1, b = 1.2, 0.75
    score = 0.0
    for term, qf in qtf.items():
        tf = doc_tf.get(term, 0)
        if tf == 0:
            continue
        d = df.get(term, 0)
        idf = math.log(1 + (n_docs - d + 0.5) / (d + 0.5))
        score += qf * idf * (tf * (k1 + 1)) / (
            tf + k1 * (1 - b + b * doc_len / max(avg_dl, 1e-9))
        )
    return score


def search(
    records: list[dict],
    query: str,
    limit: int = 10,
    project: str = "",
) -> list[tuple[float, dict]]:
    """按相关性检索，返回 [(score, record), ...] 降序，最多 limit 条。"""
    q = tokenize(query)
    if not q:
        return []
    qtf = Counter(q)

    pool = [r for r in records if not project or r.get("project") == project]
    if not pool:
        return []

    docs: list[tuple[Counter, int]] = []
    df: Counter = Counter()
    for r in pool:
        title = r.get("title", "")
        text = r.get("content", "")
        tags = " ".join(r.get("tags", []))
        # title ×3 出现（权重 2），content/tags ×1
        toks = tokenize(text) + tokenize(tags) + tokenize(title) * 3
        c = Counter(toks)
        docs.append((c, len(toks)))
        df.update(c.keys())

    avg_dl = sum(l for _, l in docs) / len(docs)
    scored: list[tuple[float, dict]] = []
    for (c, dl), r in zip(docs, pool):
        s = _bm25_score(qtf, c, dl, avg_dl, df, len(pool))
        if s > 0:
            scored.append((s, r))
    scored.sort(key=lambda x: (-x[0], -x[1].get("ts", 0)))
    return scored[:limit]


def cosine(a: list[float], b: list[float]) -> float:
    if not a or not b or len(a) != len(b):
        return 0.0
    dot = sum(x * y for x, y in zip(a, b))
    na = math.sqrt(sum(x * x for x in a))
    nb = math.sqrt(sum(y * y for y in b))
    if na == 0 or nb == 0:
        return 0.0
    return dot / (na * nb)


def semantic_search(
    records: list[dict],
    query_emb: list[float],
    limit: int = 10,
    project: str = "",
    vec_getter=None,
) -> list[tuple[float, dict]]:
    """余弦语义检索。vec_getter(rid)->向量：给则用之（向量已拆侧车），
    否则回退读记录内 emb 字段（兼容拆分前旧数据）。"""
    if not query_emb:
        return []
    pool = [r for r in records if not project or r.get("project") == project]
    scored = []
    for r in pool:
        v = vec_getter(r["id"]) if vec_getter else r.get("emb")
        if not v:
            continue
        s = cosine(query_emb, v)
        if s > 0:
            scored.append((s, r))
    scored.sort(key=lambda x: (-x[0], -x[1].get("ts", 0)))
    return scored[:limit]
