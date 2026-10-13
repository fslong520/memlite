"""memlite CLI。

用法：
  memlite init [--data-dir DIR]        初始化数据目录与配置
  memlite remember "内容" [--title T] [--tags a,b] [--project P]
  memlite recall "查询" [--limit N] [--project P] [--json]
  memlite show <id> [--json]
  memlite list [--limit N] [--json]
  memlite forget <id>
  memlite sync [--dry-run]
  memlite inject [--target T | --all | --probe | --remove T] [--force]
  memlite status [--json]

配置：~/.memlite/config.json {"data_dir": "..."}；环境变量 MEMLITE_DATA 覆盖。
"""

from __future__ import annotations

import argparse
import json
import os
import sys

CONFIG_PATH = os.path.expanduser("~/.memlite/config.json")


def _config() -> dict:
    if os.path.isfile(CONFIG_PATH):
        try:
            return json.load(open(CONFIG_PATH, encoding="utf-8"))
        except json.JSONDecodeError:
            pass
    return {}


def _data_dir(args=None) -> str:
    if args is not None and getattr(args, "data_dir", None):
        return os.path.abspath(os.path.expanduser(args.data_dir))
    env = os.environ.get("MEMLITE_DATA")
    if env:
        return os.path.abspath(os.path.expanduser(env))
    cfg = _config().get("data_dir")
    if cfg:
        return os.path.abspath(os.path.expanduser(cfg))
    return os.path.abspath(os.path.expanduser("~/memlite-data"))


def _out(obj, as_json: bool, human: str = ""):
    if as_json:
        print(json.dumps(obj, ensure_ascii=False, indent=2))
    else:
        print(human or json.dumps(obj, ensure_ascii=False, indent=2))


def _load_store(data_dir: str):
    from .store import Store

    return Store(data_dir)


# ---------- 命令 ----------


def cmd_init(args) -> int:
    data_dir = _data_dir(args)
    os.makedirs(data_dir, exist_ok=True)
    os.makedirs(os.path.dirname(CONFIG_PATH), exist_ok=True)
    cfg = _config()
    cfg["data_dir"] = data_dir
    with open(CONFIG_PATH, "w", encoding="utf-8") as f:
        json.dump(cfg, f, ensure_ascii=False, indent=2)
    import subprocess

    if not os.path.isdir(os.path.join(data_dir, ".git")):
        subprocess.run(["git", "init", "-b", "main", data_dir], check=True, capture_output=True)
    store = _load_store(data_dir)
    _out({"data_dir": data_dir, "config": CONFIG_PATH, "records": store.count()},
         args.json, f"数据目录: {data_dir}\n配置: {CONFIG_PATH}\n记忆条数: {store.count()}")
    return 0


def cmd_remember(args) -> int:
    store = _load_store(_data_dir(args))
    tags = [t for t in (args.tags or "").split(",") if t.strip()]
    title = (args.title or "").strip()
    if not title:
        # 缺省标题：截正文首句（≤40 字），保证有词面可检索
        head = args.content.strip().split("\n", 1)[0]
        for sep in ("。", "！", "？", ". ", "! ", "? "):
            if sep in head:
                head = head.split(sep, 1)[0]
                break
        title = head[:40]
    # 配了嵌入 API 就顺手算向量进侧车；失败不拦存入（词面档仍可检索）
    from . import embed as embedm
    from .vectors import VectorIndex

    emb, emb_note = None, ""
    client = embedm.get_client()
    if client:
        try:
            emb = client.embed(f"{title}\n{args.content}")
        except embedm.EmbeddingError as e:
            emb_note = f"（嵌入失败，仅词面可查：{e}）"
    rid = store.put(args.content, title=title, tags=tags, project=args.project or "")
    if emb is not None:
        VectorIndex(_data_dir(args)).put(rid, emb)
    _out({"id": rid, "title": title, "embedded": emb is not None, "records": store.count()},
         args.json, f"已存 {rid}「{title}」{emb_note}（共 {store.count()} 条）")
    return 0


def cmd_recall(args) -> int:
    from .search import search, semantic_search
    from . import embed as embedm
    from .vectors import VectorIndex

    store = _load_store(_data_dir(args))
    backend = "bm25"
    hits: list = []
    # 语义档：配了 API 且查询向量算得出才走；任何失败静默回退词面
    client = embedm.get_client()
    if client and not args.bm25:
        try:
            qemb = client.embed(args.query)
            vidx = VectorIndex(_data_dir(args))
            hits = semantic_search(store.all(), qemb, limit=args.limit,
                                   project=args.project or "", vec_getter=vidx.get)
            backend = "semantic"
        except embedm.EmbeddingError:
            hits = []
    if backend != "semantic" or not hits:
        backend = "bm25"
        hits = search(store.all(), args.query, limit=args.limit, project=args.project or "")
    if args.json:
        print(json.dumps(
            {"backend": backend,
             "hits": [{"score": round(s, 4),
                       **{k: r.get(k) for k in ("id", "ts", "title", "content", "tags", "project")}}
                      for s, r in hits]},
            ensure_ascii=False, indent=2))
    else:
        print(f"（{backend} 检索）", file=sys.stderr)
        if not hits:
            print("（无命中）")
        for s, r in hits:
            head = (r.get("title") or r.get("content", "")[:40]).replace("\n", " ")
            print(f"[{s:.3f}] {r['id']}  {head}")
    return 0


def cmd_show(args) -> int:
    store = _load_store(_data_dir(args))
    rec = store.get(args.id) or next(
        (r for r in store.all() if r["id"].startswith(args.id)), None)
    if not rec:
        print(f"记忆不存在: {args.id}", file=sys.stderr)
        return 1
    _out(rec, args.json,
         f"id: {rec['id']}\ncreated: {rec['ts']}\ntitle: {rec.get('title','')}\n"
         f"tags: {', '.join(rec.get('tags', []))}\nproject: {rec.get('project','')}\n---\n{rec['content']}")
    return 0


def cmd_list(args) -> int:
    store = _load_store(_data_dir(args))
    recs = sorted(store.all(), key=lambda r: -r.get("ts", 0))[: args.limit]
    if args.json:
        print(json.dumps(recs, ensure_ascii=False, indent=2))
    else:
        for r in recs:
            head = (r.get("title") or r.get("content", "")[:40]).replace("\n", " ")
            print(f"{r['id']}  {head}")
    return 0


def cmd_forget(args) -> int:
    store = _load_store(_data_dir(args))
    rec = store.get(args.id) or next(
        (r for r in store.all() if r["id"].startswith(args.id)), None)
    if not rec:
        print(f"记忆不存在: {args.id}", file=sys.stderr)
        return 1
    store.delete(rec["id"])
    _out({"deleted": rec["id"]}, args.json, f"已删 {rec['id']}")
    return 0


def cmd_sync(args) -> int:
    from . import sync as syncmod

    data_dir = _data_dir(args)
    try:
        if args.dry_run:
            _out(syncmod.status(data_dir), args.json,
                 json.dumps(syncmod.status(data_dir), ensure_ascii=False, indent=2))
        else:
            out = syncmod.sync(data_dir)
            _out(out, args.json, json.dumps(out, ensure_ascii=False, indent=2))
        return 0
    except syncmod.SyncError as e:
        print(f"同步失败: {e}", file=sys.stderr)
        return 1


def cmd_inject(args) -> int:
    from . import inject as injm

    data_dir = _data_dir(args)
    if args.probe:
        _out(injm.probe(), args.json, json.dumps(injm.probe(), ensure_ascii=False, indent=2))
        return 0
    if args.remove:
        out = injm.remove(args.remove)
        _out(out, args.json, json.dumps(out, ensure_ascii=False, indent=2))
        return 0
    if args.all:
        results = injm.inject_all(data_dir, force=args.force)
    elif args.target:
        results = [injm.inject(args.target, data_dir, force=args.force)]
    else:
        print("须指定 --target <名> 或 --all 或 --probe 或 --remove <名>", file=sys.stderr)
        return 2
    _out(results, args.json, json.dumps(results, ensure_ascii=False, indent=2))
    return 0


def cmd_reembed(args) -> int:
    """回填向量：给侧车还没有向量的记忆补算（配了 API 才有意义）。"""
    from . import embed as embedm
    from .vectors import VectorIndex

    store = _load_store(_data_dir(args))
    client = embedm.get_client()
    if not client:
        print("未配置嵌入 API（config 的 embedding 段或 MEMLITE_API_BASE/KEY/MODEL）",
              file=sys.stderr)
        return 2
    vidx = VectorIndex(_data_dir(args))
    done = failed = 0
    for rec in store.all():
        if vidx.has(rec["id"]):
            continue
        try:
            vidx.put(rec["id"], client.embed(f"{rec.get('title','')}\n{rec['content']}"))
            done += 1
        except embedm.EmbeddingError as e:
            print(f"跳过 {rec['id']}: {e}", file=sys.stderr)
            failed += 1
    out = {"embedded": done, "failed": failed, "vectors": vidx.count(), "total": store.count()}
    _out(out, args.json,
         f"回填 {done} 条，失败 {failed}，侧车向量 {vidx.count()}，库容 {store.count()}")
    return 0 if failed == 0 else 1


def cmd_status(args) -> int:
    from . import sync as syncmod
    from . import embed as embedm
    from .vectors import VectorIndex

    data_dir = _data_dir(args)
    store = _load_store(data_dir)
    ecfg = embedm.embedding_config()
    info = {
        "data_dir": data_dir,
        "records": store.count(),
        "lines": store.line_count(),
        "bad_lines": store.bad_lines,
        "files": store.files(),
        "vectors": VectorIndex(data_dir).count(),
        "embedding": {
            "configured": bool(ecfg.get("api_base") and ecfg.get("api_key") and ecfg.get("model")),
            "api_base": ecfg.get("api_base", ""),
            "model": ecfg.get("model", ""),
            # api_key 刻意不回显
        },
        "sync": syncmod.status(data_dir),
    }
    _out(info, args.json, json.dumps(info, ensure_ascii=False, indent=2))
    return 0


# ---------- 入口 ----------


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="memlite", description="简化嵌入式记忆系统")
    p.add_argument("--data-dir", help="数据目录（默认取配置）")
    p.add_argument("--json", action="store_true", help="JSON 输出")
    # 子命令也认这两个旗（SUPPRESS 默认防子解析器覆盖主解析器的值），
    # 支持 `memlite status --json` 这种自然写法
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--data-dir", default=argparse.SUPPRESS, help="数据目录")
    common.add_argument("--json", action="store_true", default=argparse.SUPPRESS, help="JSON 输出")
    sub = p.add_subparsers(dest="cmd", required=True)

    sp = sub.add_parser("init", parents=[common], help="初始化")
    sp.set_defaults(fn=cmd_init)

    sp = sub.add_parser("remember", parents=[common], help="存记忆")
    sp.add_argument("content")
    sp.add_argument("--title", default="",
                    help="事实句标题：主语+动作+关键实体或结果（如「nginx 反代 504 调 timeout 解决」），"
                         "禁只写主题分类名（如「运维记录」）；缺省则截取正文首句")
    sp.add_argument("--tags", default="")
    sp.add_argument("--project", default="")
    sp.set_defaults(fn=cmd_remember)

    sp = sub.add_parser("recall", parents=[common], help="检索（配了嵌入 API 走语义，否则词面 BM25）")
    sp.add_argument("query")
    sp.add_argument("--limit", type=int, default=10)
    sp.add_argument("--project", default="")
    sp.add_argument("--bm25", action="store_true", help="强制词面档，跳过语义")
    sp.set_defaults(fn=cmd_recall)

    sp = sub.add_parser("show", parents=[common], help="看全文")
    sp.add_argument("id")
    sp.set_defaults(fn=cmd_show)

    sp = sub.add_parser("list", parents=[common], help="列最近")
    sp.add_argument("--limit", type=int, default=20)
    sp.set_defaults(fn=cmd_list)

    sp = sub.add_parser("forget", parents=[common], help="删记忆")
    sp.add_argument("id")
    sp.set_defaults(fn=cmd_forget)

    sp = sub.add_parser("sync", parents=[common], help="git 同步")
    sp.add_argument("--dry-run", action="store_true")
    sp.set_defaults(fn=cmd_sync)

    sp = sub.add_parser("inject", parents=[common], help="rules 注入")
    sp.add_argument("--target")
    sp.add_argument("--all", action="store_true")
    sp.add_argument("--probe", action="store_true")
    sp.add_argument("--remove")
    sp.add_argument("--force", action="store_true")
    sp.set_defaults(fn=cmd_inject)

    sp = sub.add_parser("reembed", parents=[common], help="给无向量的记忆回填嵌入（须配 API）")
    sp.set_defaults(fn=cmd_reembed)

    sp = sub.add_parser("status", parents=[common], help="状态")
    sp.set_defaults(fn=cmd_status)
    return p


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
