"""存储层：append-only JSONL + 16MB 分块轮转 + 内存索引。

分块设计（长期增长的正解）：
- 活跃文件 memory.jsonl 恒为追加目标；写后超 CHUNK_BYTES 即轮转——
  重命名为归档 memory-<4位序号>.jsonl，下一条起写新 memory.jsonl。
- 加载按序读归档（0001→N）再读活跃文件，末写胜出语义不变，
  墓碑在新文件里照常压掉旧归档里的 put。
- 每个文件 ≤16MB，永远到不了 GitHub 单文件 100MB 上限；追加日志
  对 rebase 友好（多机各写不同文件，冲突面更小）。
- compact() 只留存活记录重写回单一活跃文件并清归档。
向量不在此层——见 vectors.py 侧车（派生数据，gitignore）。
"""

from __future__ import annotations

import json
import os
import re
import time
import uuid

DATA_FILE = "memory.jsonl"
CHUNK_BYTES = 16 * 1024 * 1024  # 单文件上限，超过即轮转
_ARCHIVE_RE = re.compile(r"^memory-(\d{4})\.jsonl$")


class Store:
    def __init__(self, data_dir: str, chunk_bytes: int = CHUNK_BYTES):
        self.data_dir = data_dir
        self.path = os.path.join(data_dir, DATA_FILE)
        self.chunk_bytes = chunk_bytes
        self.records: dict[str, dict] = {}
        self.deleted: set[str] = set()
        self.bad_lines = 0
        self._load()

    # ---------- 加载 ----------

    def _archive_paths(self) -> list[str]:
        """归档文件按序号升序（旧→新）。"""
        found = []
        if not os.path.isdir(self.data_dir):
            return found
        for name in os.listdir(self.data_dir):
            m = _ARCHIVE_RE.match(name)
            if m:
                found.append((int(m.group(1)), os.path.join(self.data_dir, name)))
        return [p for _, p in sorted(found)]

    def _files(self) -> list[str]:
        """全部数据文件：归档在前（旧），活跃文件在后（新）。"""
        paths = self._archive_paths()
        if os.path.exists(self.path):
            paths.append(self.path)
        return paths

    def _load(self) -> None:
        for path in self._files():
            with open(path, encoding="utf-8") as f:
                for line in f:
                    line = line.strip()
                    if not line:
                        continue
                    try:
                        op = json.loads(line)
                    except json.JSONDecodeError:
                        self.bad_lines += 1
                        continue
                    self._apply(op)

    def _apply(self, op: dict) -> None:
        kind = op.get("op")
        rid = op.get("id")
        if not rid:
            self.bad_lines += 1
            return
        if kind == "put":
            self.records[rid] = op
            self.deleted.discard(rid)
        elif kind == "del":
            self.deleted.add(rid)
            self.records.pop(rid, None)
        else:
            self.bad_lines += 1

    def _append(self, obj: dict) -> None:
        os.makedirs(self.data_dir, exist_ok=True)
        line = json.dumps(obj, ensure_ascii=False)
        with open(self.path, "a", encoding="utf-8") as f:
            f.write(line + "\n")
            f.flush()
            os.fsync(f.fileno())
        self._maybe_rotate()

    def _maybe_rotate(self) -> None:
        """活跃文件超限即轮转：改名归档（取最大序号+1），新追加自然落新文件。"""
        if not os.path.exists(self.path):
            return
        if os.path.getsize(self.path) < self.chunk_bytes:
            return
        archives = self._archive_paths()
        seq = int(_ARCHIVE_RE.match(os.path.basename(archives[-1])).group(1)) + 1 if archives else 1
        os.replace(self.path, os.path.join(self.data_dir, f"memory-{seq:04d}.jsonl"))

    # ---------- 写 ----------

    def put(
        self,
        content: str,
        title: str = "",
        tags: list[str] | None = None,
        project: str = "",
    ) -> str:
        """追加一条记忆，返回 id。向量另存 VectorIndex 侧车（派生数据不进正文仓）。"""
        if not content or not content.strip():
            raise ValueError("content 为空")
        rid = uuid.uuid4().hex[:12]
        rec = {
            "op": "put",
            "id": rid,
            "ts": time.time(),
            "title": title.strip(),
            "content": content.strip(),
            "tags": [t.strip() for t in (tags or []) if t.strip()],
            "project": project.strip(),
        }
        self._append(rec)
        self._apply(rec)
        return rid

    def delete(self, rid: str) -> None:
        """删除一条记忆（追加墓碑，随同步传播到他端）。"""
        if rid not in self.records:
            raise KeyError(f"记忆不存在: {rid}")
        op = {"op": "del", "id": rid, "ts": time.time()}
        self._append(op)
        self._apply(op)

    def compact(self) -> int:
        """压实：只留存活记录重写回单一活跃文件，删归档。

        单机使用；多机并发期间勿跑（追加日志对 rebase 友好，压实破坏该性质）。
        返回压实前行数。
        """
        lines_before = self.line_count()
        tmp = self.path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            for rec in self.records.values():
                f.write(json.dumps(rec, ensure_ascii=False) + "\n")
            f.flush()
            os.fsync(f.fileno())
        for p in self._archive_paths():
            os.remove(p)
        os.replace(tmp, self.path)
        return lines_before

    # ---------- 读 ----------

    def get(self, rid: str) -> dict | None:
        return self.records.get(rid)

    def all(self) -> list[dict]:
        return list(self.records.values())

    def count(self) -> int:
        return len(self.records)

    def line_count(self) -> int:
        total = 0
        for path in self._files():
            with open(path, encoding="utf-8") as f:
                total += sum(1 for _ in f)
        return total

    def files(self) -> list[dict]:
        """分块清单（status 展示用）。"""
        out = []
        for path in self._files():
            out.append({"file": os.path.basename(path),
                        "bytes": os.path.getsize(path)})
        return out
