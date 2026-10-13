"""一次性迁移：memory*.jsonl（正文+向量混行）→ 正文分块 + emb.f32 侧车。

兼容现场：单文件、已轮转多分块、两者并存、中断残留（.new/.bak 与既有侧车）。
- 两遍式：先扫每 id 末次向量（末写胜出），再写分块+侧车。
- 原子性：全写 .new → 行数守恒校验 → 旧文件改名 .bak → .new 顶上 → 删 .bak。
用法：python3 scripts/migrate_split.py <data_dir>
"""

from __future__ import annotations

import json
import os
import re
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from memlite.store import DATA_FILE, CHUNK_BYTES  # noqa: E402

_CHUNK_RE = re.compile(r"^memory-(\d{4})\.jsonl$")


def _source_files(data_dir: str) -> list[str]:
    """旧→新排序全部正文文件：归档按序号在前，活跃 memory.jsonl 在后。"""
    archives = []
    for name in os.listdir(data_dir):
        m = _CHUNK_RE.match(name)
        if m:
            archives.append((int(m.group(1)), os.path.join(data_dir, name)))
    out = [p for _, p in sorted(archives)]
    active = os.path.join(data_dir, DATA_FILE)
    if os.path.exists(active):
        out.append(active)
    return out


def _cleanup_tmp(data_dir: str) -> None:
    for name in os.listdir(data_dir):
        if name.endswith(".new"):
            os.remove(os.path.join(data_dir, name))


def main(data_dir: str) -> int:
    srcs = _source_files(data_dir)
    if not srcs:
        print("无正文文件，无事可做")
        return 0
    _cleanup_tmp(data_dir)

    # 既有侧车（上半途迁移/新写入的向量不丢：行内无 emb 时回退）
    old_sidecar: dict[str, list[float]] = {}
    ids_old = os.path.join(data_dir, "emb-ids.txt")
    f32_old = os.path.join(data_dir, "emb.f32")
    if os.path.exists(ids_old) and os.path.exists(f32_old):
        with open(ids_old, encoding="utf-8") as fi, open(f32_old, "rb") as ff:
            id_list = [x.strip() for x in fi if x.strip()]
            raw = ff.read()
        if id_list and len(raw) % (4 * len(id_list)) == 0:
            dim = len(raw) // 4 // len(id_list)
            for i, rid in enumerate(id_list):
                old_sidecar[rid] = list(struct.unpack_from(f"<{dim}f", raw, i * dim * 4))
        print(f"预载既有侧车 {len(old_sidecar)} 条")

    # 第一遍：计数 + 每 id 末次向量
    n_in = n_bad = 0
    last_emb: dict[str, list[float]] = {}
    for src in srcs:
        with open(src, encoding="utf-8") as f:
            for line in f:
                n_in += 1
                try:
                    rec = json.loads(line)
                except json.JSONDecodeError:
                    n_bad += 1
                    continue
                v = rec.pop("emb", None) or old_sidecar.get(rec.get("id", ""))
                if v and rec.get("id"):
                    last_emb[rec["id"]] = v
    n_text = n_in - n_bad

    # 第二遍：正文去 emb 按 16MB 轮转写 .new 分块
    new_chunks: list[str] = []
    cur = None
    cur_bytes = 0
    chunk_no = 0

    def open_chunk():
        nonlocal cur, cur_bytes, chunk_no
        chunk_no += 1
        name = DATA_FILE if chunk_no == 1 else f"memory-{chunk_no:04d}.jsonl"
        path = os.path.join(data_dir, name + ".new")
        cur = open(path, "w", encoding="utf-8")
        cur_bytes = 0
        new_chunks.append(path)

    open_chunk()
    for src in srcs:
        with open(src, encoding="utf-8") as f:
            for line in f:
                try:
                    rec = json.loads(line)
                except json.JSONDecodeError:
                    continue
                rec.pop("emb", None)
                out_line = json.dumps(rec, ensure_ascii=False) + "\n"
                out_bytes = len(out_line.encode("utf-8"))
                if cur_bytes + out_bytes >= CHUNK_BYTES:
                    cur.close()
                    open_chunk()
                cur.write(out_line)
                cur_bytes += out_bytes
    cur.close()

    # 侧车一次写成
    f32n = os.path.join(data_dir, "emb.f32.new")
    idn = os.path.join(data_dir, "emb-ids.txt.new")
    with open(f32n, "wb") as f32, open(idn, "w", encoding="utf-8") as ids:
        for rid, v in last_emb.items():
            f32.write(struct.pack(f"<{len(v)}f", *v))
            ids.write(rid + "\n")

    # 校验
    replayed = sum(sum(1 for _ in open(p, encoding="utf-8")) for p in new_chunks)
    with open(idn, encoding="utf-8") as f:
        sidecar_n = sum(1 for _ in f)
    if replayed != n_text or sidecar_n != len(last_emb):
        print(f"校验失败：回放 {replayed}/{n_text}，侧车 {sidecar_n}/{len(last_emb)}，放弃替换",
              file=sys.stderr)
        _cleanup_tmp(data_dir)
        return 1

    print(f"读入 {n_in} 行（{len(srcs)} 文件）：正文 {n_text}、坏行 {n_bad}、"
          f"末次向量 {len(last_emb)}、新分块 {chunk_no}")

    # 落地：旧文件改名 .bak → .new 顶上 → 删 .bak
    for src in srcs:
        os.rename(src, src + ".bak")
    for p in new_chunks:
        os.replace(p, p[:-4])
    os.replace(f32n, os.path.join(data_dir, "emb.f32"))
    os.replace(idn, os.path.join(data_dir, "emb-ids.txt"))
    for src in srcs:
        os.remove(src + ".bak")
    print("迁移完成")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1] if len(sys.argv) > 1 else "."))
