//! 同步层：数据仓 git 同步。
//! 白名单 glob（memory*.jsonl/meta.json/README.md）+ push 后双通道核验 sha。

use std::path::Path;
use std::process::Command;

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| format!("git 启动失败: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {} 失败: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn has_remote(repo: &Path) -> bool {
    git(repo, &["remote", "get-url", "origin"]).map(|s| !s.is_empty()).unwrap_or(false)
}

/// 展开白名单：真实存在才 add（git pathspec 无匹配会报错）
fn whitelisted(repo: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(repo) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if (name.starts_with("memory") && name.ends_with(".jsonl"))
                || name == "meta.json"
                || name == "README.md"
            {
                out.push(name);
            }
        }
    }
    out.sort();
    out
}

pub struct SyncReport {
    pub committed: bool,
    pub local_sha: String,
    pub remote_sha: String,
    pub verified: bool,
}

pub fn status(repo: &Path) -> serde_json::Value {
    let remote = has_remote(repo);
    let dirty = whitelisted(repo)
        .iter()
        .filter(|f| {
            Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["status", "--porcelain", "--", f])
                .output()
                .map(|o| !o.stdout.is_empty())
                .unwrap_or(false)
        })
        .count()
        > 0;
    let local = git(repo, &["rev-parse", "HEAD"]).unwrap_or_default();
    serde_json::json!({
        "repo": repo.to_string_lossy(),
        "has_remote": remote,
        "dirty": dirty,
        "local_sha": local,
    })
}

pub fn sync(repo: &Path) -> Result<SyncReport, String> {
    if !repo.join(".git").is_dir() {
        return Err(format!("不是 git 仓: {}", repo.to_string_lossy()));
    }
    if !has_remote(repo) {
        return Err("未配置 origin；先 git remote add origin <私有仓URL>".into());
    }

    // 1. 只 add 白名单
    let files = whitelisted(repo);
    if !files.is_empty() {
        let mut args = vec!["add", "--"];
        for f in &files {
            args.push(f);
        }
        git(repo, &args)?;
    }

    // 2. 有暂存变更才 commit
    let staged = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["diff", "--cached", "--quiet"])
        .output()
        .map_err(|e| e.to_string())?;
    let committed = !staged.status.success();
    if committed {
        let msg = format!("memlite sync {}", chrono_lite());
        git(repo, &["commit", "-m", &msg])?;
    }

    // 3. fetch；远端分支已存在才 rebase（首推时远端可能为空仓）
    git(repo, &["fetch", "origin"])?;
    let branch = git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let remote_ref = git(repo, &["ls-remote", "--heads", "origin", &branch]).unwrap_or_default();
    if !remote_ref.is_empty() {
        git(repo, &["pull", "--rebase", "origin", &branch])?;
    }

    // 4. push -u
    git(repo, &["push", "-u", "origin", &branch])?;

    // 5. 双通道核验
    let local_sha = git(repo, &["rev-parse", "HEAD"])?;
    let ls = git(repo, &["ls-remote", "origin", &format!("refs/heads/{branch}")])?;
    let remote_sha = ls.split_whitespace().next().unwrap_or("").to_string();
    let verified = !remote_sha.is_empty() && local_sha == remote_sha;
    if !verified {
        return Err(format!(
            "push 核验失败: local={} remote={}",
            &local_sha[..8.min(local_sha.len())],
            &remote_sha[..8.min(remote_sha.len())]
        ));
    }
    Ok(SyncReport {
        committed,
        local_sha,
        remote_sha,
        verified,
    })
}

/// 本地时间格式化（避免引入 chrono 依赖）
fn chrono_lite() -> String {
    let out = Command::new("date")
        .args(["+%Y-%m-%d %H:%M:%S"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if out.is_empty() {
        "sync".into()
    } else {
        out
    }
}
