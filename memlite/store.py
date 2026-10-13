"""存储层：append-only JSONL 追加日志 + 内存索引。

设计（嵌入式友好）：
- 数据文件 memory.jsonl 一行一条操作（put/del），追加写 O(1)，无重写整库。
- 加载时顺序回放建内存索引，坏行跳过并计数（截断/损坏不致全库不可读）。
- put 用 fsync 落盘；del 写墓碑，检索层不再见。
- 无加密、无依赖，纯标准库。
"""

from __future__ import annotations

import json
import os
import time
import uuid

DATA_FILE = "memory.jsonl"


class Store:
    def __init__(self, data_dir: str):
        self.data_dir = data_dir
        self.path = os.path.join(data_dir, DATA_FILE)
        self.records: dict[str, dict] = {}
        self.deleted: set[str] = set()
        self.bad_lines = 0
        self._load()

    # ---------- 加载 ----------

    def _load(self) -> None:
        if not os.path.exists(self.path):
            return
        with open(self.path, encoding="utf-8") as f:
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

    # ---------- 写 ----------

    def put(
        self,
        content: str,
        title: str = "",
        tags: list[str] | None = None,
        project: str = "",
        emb: list[float] | None = None,
    ) -> str:
        """追加一条记忆，返回 id。emb 为可选嵌入向量（配了嵌入 API 才有）。"""
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
        if emb:
            rec["emb"] = [float(x) for x in emb]
        self._append(rec)
        self._apply(rec)
        return rid

    def update_emb(self, rid: str, emb: list[float]) -> None:
        """回填/更新嵌入向量：追加同 id 的 put 行（末写胜出，保持追加日志不变式）。"""
        rec = dict(self.records[rid])
        rec["emb"] = [float(x) for x in emb]
        self._append(rec)
        self._apply(rec)

    def delete(self, rid: str) -> None:
        """删除一条记忆（追加墓碑，随同步传播到他端）。"""
        if rid not in self.records:
            raise KeyError(f"记忆不存在: {rid}")
        op = {"op": "del", "id": rid, "ts": time.time()}
        self._append(op)
        self._apply(op)

    def compact(self) -> int:
        """就地压实：只保留存活记录，原子替换。

        单机使用；多机并发期间勿跑（追加日志对 rebase 友好，压实会破坏该性质）。
        返回压实前行数。
        """
        lines_before = self.line_count()
        tmp = self.path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            for rec in self.records.values():
                f.write(json.dumps(rec, ensure_ascii=False) + "\n")
            f.flush()
            os.fsync(f.fileno())
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
        if not os.path.exists(self.path):
            return 0
        with open(self.path, encoding="utf-8") as f:
            return sum(1 for _ in f)
