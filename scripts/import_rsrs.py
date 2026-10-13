"""把 rsrs 全量导出（明文 JSON 数组）灌进 memlite 数据仓。

映射：id 原样保留（便于回查对照）、created_at→ts（epoch 秒）、
title/content/tags/project 直取，importance/kind/parent_id 原样带过去
（memlite 不解释这些字段，但检索展示与日后对照用得上）。
幂等：按 id 查重，已存在跳过——重跑不重复灌。
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from datetime import datetime, timezone


def _epoch(ts: str) -> float:
    """ISO 时间戳→epoch 秒；解析失败回退当前时刻。"""
    try:
        return datetime.fromisoformat(ts.replace("Z", "+00:00")).timestamp()
    except (ValueError, AttributeError):
        return time.time()


def main(argv=None) -> int:
    p = argparse.ArgumentParser()
    p.add_argument("export_json", help="rsrs export 产出的明文 JSON")
    p.add_argument("--data-dir", required=True, help="memlite 数据仓目录")
    p.add_argument("--dry-run", action="store_true")
    args = p.parse_args(argv)

    entries = json.load(open(args.export_json, encoding="utf-8"))
    if not isinstance(entries, list):
        print("导出文件不是 JSON 数组", file=sys.stderr)
        return 1

    # 已灌 id 查重（幂等）
    path = os.path.join(args.data_dir, "memory.jsonl")
    existing: set[str] = set()
    if os.path.exists(path):
        with open(path, encoding="utf-8") as f:
            for line in f:
                try:
                    existing.add(json.loads(line)["id"])
                except (json.JSONDecodeError, KeyError):
                    continue

    skipped = written = 0
    if not args.dry_run:
        os.makedirs(args.data_dir, exist_ok=True)
    with open(path, "a" if not args.dry_run else os.devnull, encoding="utf-8") as out:
        for e in entries:
            rid = e.get("id", "")
            if not rid or rid in existing:
                skipped += 1
                continue
            rec = {
                "op": "put",
                "id": rid,
                "ts": _epoch(e.get("created_at", "")),
                "title": (e.get("title") or "").strip(),
                "content": (e.get("content") or "").strip(),
                "tags": e.get("tags") or [],
                "project": e.get("project") or "",
                # rsrs 特有元数据原样携带
                "importance": e.get("importance", ""),
                "kind": e.get("kind", ""),
                "parent_id": e.get("parent_id", ""),
                "src": "rsrs",
            }
            if not rec["content"]:
                skipped += 1
                continue
            out.write(json.dumps(rec, ensure_ascii=False) + "\n")
            existing.add(rid)
            written += 1

    print(json.dumps({
        "total": len(entries), "written": written,
        "skipped": skipped, "data_dir": args.data_dir,
        "dry_run": args.dry_run,
    }, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
