"""同步层：数据仓 git 同步。

规矩（源自 respire/忆时踩坑）：
- 只 add 指定数据文件，禁 `git add -A`（防凭据/杂物入库）。
- push 后双通道核验：`git rev-parse HEAD` vs `git ls-remote` 的远端 sha，
  两者一致才算推成（npm/CDN 延迟、假成功都见过）。
- 冲突走 `pull --rebase`；记忆 JSONL 是追加日志，两机各写不同行 rebase
  通常干净；同一行冲突极少，报出让用户裁。
"""

from __future__ import annotations

import os
import subprocess
import time

# 同步白名单（git pathspec glob）：正文分块 memory*.jsonl + 元数据 + README。
# 向量侧车 emb.f32/emb-ids.txt 是派生数据，不进 git（.gitignore 兜底），
# 他机 clone 后 memlite reembed 重建。
TRACKED_PATTERNS = ("memory*.jsonl", "meta.json", "README.md")


class SyncError(RuntimeError):
    pass


def _git(repo: str, *args: str, check: bool = True) -> str:
    r = subprocess.run(
        ["git", "-C", repo, *args],
        capture_output=True,
        text=True,
        timeout=120,
    )
    if check and r.returncode != 0:
        raise SyncError(f"git {' '.join(args)} 失败: {r.stderr.strip()}")
    return r.stdout.strip()


def status(repo: str) -> dict:
    """同步状态摘要。"""
    has_remote = bool(_git(repo, "remote", "get-url", "origin", check=False))
    tracked = _whitelisted(repo)
    dirty = _git(repo, "status", "--porcelain", "--", *tracked, check=False) if tracked else ""
    head = subprocess.run(
        ["git", "-C", repo, "rev-parse", "HEAD"], capture_output=True, text=True
    )
    local = head.stdout.strip() if head.returncode == 0 else ""
    ahead = _git(repo, "rev-list", "--count", "@{u}..HEAD", check=False) if has_remote else ""
    behind = _git(repo, "rev-list", "--count", "HEAD..@{u}", check=False) if has_remote else ""
    return {
        "repo": repo,
        "has_remote": has_remote,
        "dirty": bool(dirty.strip()),
        "local_sha": local,
        "ahead": ahead,
        "behind": behind,
    }


def _whitelisted(repo: str) -> list[str]:
    """展开白名单 glob，只留真实存在的文件（git pathspec 无匹配会报错）。"""
    import glob as globmod

    out: list[str] = []
    for pat in TRACKED_PATTERNS:
        out.extend(os.path.basename(p) for p in sorted(globmod.glob(os.path.join(repo, pat))))
    return out


def sync(repo: str, message: str = "") -> dict:
    """一次完整同步：add(白名单) → commit → pull --rebase → push → 双通道核验。"""
    if not os.path.isdir(os.path.join(repo, ".git")):
        raise SyncError(f"不是 git 仓: {repo}")
    if not _git(repo, "remote", "get-url", "origin", check=False):
        raise SyncError("未配置 origin；先 git remote add origin <私有仓URL>")

    out: dict = {"repo": repo}

    # 1. 只 add 白名单文件（glob 展开分块命名；凭据类永不匹配；新增归档自动涵盖）
    added = _whitelisted(repo)
    if added:
        _git(repo, "add", "--", *added)
    out["added"] = added

    # 2. 有暂存变更才 commit（diff --cached --quiet：0=无变更 1=有变更）
    staged = subprocess.run(
        ["git", "-C", repo, "diff", "--cached", "--quiet"],
        capture_output=True,
    )
    committed = False
    if staged.returncode != 0:  # 有暂存变更
        msg = message or f"memlite sync {time.strftime('%Y-%m-%d %H:%M:%S')}"
        _git(repo, "commit", "-m", msg)
        committed = True
    out["committed"] = committed

    # 3. 远端分支已存在才 rebase（首推时远端为空仓，无从 pull）
    branch = _branch(repo)
    _git(repo, "fetch", "origin")
    remote_ref = _git(repo, "ls-remote", "--heads", "origin", branch, check=False)
    if remote_ref:
        _git(repo, "pull", "--rebase", "origin", branch)
    out["rebased"] = bool(remote_ref)

    # 4. push（首推 -u 建立跟踪）
    _git(repo, "push", "-u", "origin", branch)
    out["pushed"] = True

    # 5. 双通道核验 sha
    local_sha = _git(repo, "rev-parse", "HEAD")
    ls = _git(repo, "ls-remote", "origin", f"refs/heads/{branch}")
    remote_sha = ls.split()[0] if ls else ""
    out["local_sha"] = local_sha
    out["remote_sha"] = remote_sha
    out["verified"] = bool(remote_sha) and local_sha == remote_sha
    if not out["verified"]:
        raise SyncError(
            f"push 核验失败: local={local_sha[:8]} remote={remote_sha[:8] or '无'}"
        )
    return out


def _branch(repo: str) -> str:
    return _git(repo, "rev-parse", "--abbrev-ref", "HEAD") or "main"
