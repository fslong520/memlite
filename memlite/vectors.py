"""向量侧车：emb.f32（packed float32）+ emb-ids.txt（id 顺序对齐）。

设计动机：向量是派生数据（reembed 可重建），不该混进 git 跟踪的
memory.jsonl——拆出后正文仓只同步正文分块，向量文件 gitignore。
- emb.f32     二进制，每条 dim×4 字节，顺序与 emb-ids.txt 对齐
- emb-ids.txt 每行一个 id
dim 自适应：写首条时定 dim；读时按 文件字节数/id 数 推导，无需配置。
读用 mmap（惰性、页共享）；追加写 fsync 落盘。
"""

from __future__ import annotations

import mmap
import os
import struct
import threading

F32 = "emb.f32"
IDS = "emb-ids.txt"


class VectorIndex:
    """id → 向量 的侧车索引。进程内缓存 ids，向量按需 mmap 切片。"""

    def __init__(self, data_dir: str):
        self.data_dir = data_dir
        self.dim: int | None = None  # 首次 put 或从文件推导后确定
        self.f32_path = os.path.join(data_dir, F32)
        self.ids_path = os.path.join(data_dir, IDS)
        self.ids: list[str] = []
        self._offset: dict[str, int] = {}
        self._lock = threading.Lock()
        self._mm: mmap.mmap | None = None
        self._load()

    def _load(self) -> None:
        if os.path.exists(self.ids_path):
            with open(self.ids_path, encoding="utf-8") as f:
                for i, line in enumerate(f):
                    rid = line.strip()
                    if rid:
                        self.ids.append(rid)
                        self._offset[rid] = i
        # dim 从文件推导：f32 字节数 / id 数 / 4
        if self.dim is None and self.ids and os.path.exists(self.f32_path):
            n = os.path.getsize(self.f32_path) / 4
            if n % len(self.ids) == 0:
                self.dim = int(n // len(self.ids))

    def _mmap(self) -> mmap.mmap | None:
        if self._mm is not None:
            return self._mm
        if not os.path.exists(self.f32_path) or os.path.getsize(self.f32_path) == 0:
            return None
        f = open(self.f32_path, "rb")
        self._mm = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
        f.close()  # mmap 持有映射，关文件对象无妨
        return self._mm

    def get(self, rid: str) -> list[float] | None:
        idx = self._offset.get(rid)
        if idx is None or self.dim is None:
            return None
        mm = self._mmap()
        if mm is None:
            return None
        start = idx * self.dim * 4
        end = start + self.dim * 4
        if end > len(mm):
            return None
        return list(struct.unpack(f"<{self.dim}f", mm[start:end]))

    def put(self, rid: str, emb: list[float]) -> None:
        """追加/覆盖一条向量。覆盖（同 id）时原地改写原偏移。"""
        with self._lock:
            if self.dim is None:
                self.dim = len(emb)
            if len(emb) != self.dim:
                raise ValueError(f"向量维度不一致: {len(emb)} != {self.dim}")
            vec = struct.pack(f"<{self.dim}f", *emb)
            # 关旧 mmap 才能写
            if self._mm is not None:
                self._mm.close()
                self._mm = None
            if rid in self._offset:
                with open(self.f32_path, "r+b") as f:
                    f.seek(self._offset[rid] * self.dim * 4)
                    f.write(vec)
                    f.flush()
                    os.fsync(f.fileno())
                return
            with open(self.f32_path, "ab") as f:
                f.write(vec)
                f.flush()
                os.fsync(f.fileno())
            with open(self.ids_path, "a", encoding="utf-8") as f:
                f.write(rid + "\n")
                f.flush()
                os.fsync(f.fileno())
            self._offset[rid] = len(self.ids)
            self.ids.append(rid)

    def has(self, rid: str) -> bool:
        return rid in self._offset

    def count(self) -> int:
        return len(self.ids)
