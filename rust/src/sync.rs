//! 同步层：git 同步（数据仓本身，或工作区仓——项目级记忆文件夹随外层仓走）。
//! 白名单 glob（memory*.jsonl/meta.json/README.md）+ push 后双通道核验 sha。

use std::path::{Path, PathBuf};
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

/// 展开白名单：data_dir 下真实存在的文件（git pathspec 无匹配会报错）
fn whitelisted(data_dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(data_dir) {
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

/// 找 data_dir 之上的 git 仓根：自身是仓 → 用它；否则向上找 .git（项目级记忆随工作区仓同步）
fn find_repo(data_dir: &Path) -> Option<PathBuf> {
    if data_dir.join(".git").exists() {
        return Some(data_dir.to_path_buf());
    }
    let mut cur = data_dir.to_path_buf();
    while cur.pop() {
        if cur.join(".git").exists() {
            return Some(cur);
        }
    }
    None
}

pub struct SyncReport {
    pub committed: bool,
    pub local_sha: String,
    pub remote_sha: String,
    pub verified: bool,
    #[allow(dead_code)] // 调试/展示用：实际同步的仓库根
    pub repo: PathBuf,
}

pub fn status(data_dir: &Path) -> serde_json::Value {
    let repo = find_repo(data_dir);
    let Some(repo) = repo else {
        return serde_json::json!({
            "data_dir": data_dir.to_string_lossy(),
            "repo": null,
            "has_remote": false,
            "dirty": false,
            "local_sha": "",
            "note": "不在任何 git 仓内（项目级记忆须在 git 工作区内，或 git init 数据仓）",
        });
    };
    let remote = has_remote(&repo);
    // data_dir 相对仓库根的前缀（仓本身时为空）
    let prefix = data_dir.strip_prefix(&repo).unwrap_or(Path::new(""));
    let dirty = whitelisted(data_dir)
        .iter()
        .any(|f| {
            let rel = prefix.join(f);
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["status", "--porcelain", "--"])
                .arg(&rel)
                .output()
                .map(|o| !o.stdout.is_empty())
                .unwrap_or(false)
        });
    let head = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    serde_json::json!({
        "data_dir": data_dir.to_string_lossy(),
        "repo": repo.to_string_lossy(),
        "has_remote": remote,
        "dirty": dirty,
        "local_sha": head,
    })
}

/// 一次完整同步。data_dir 可以是仓本身或仓内子目录（项目级记忆文件夹）——
/// 白名单文件在 data_dir 下找，git 操作在仓库根做，add 用相对仓库根的路径。
pub fn sync(data_dir: &Path) -> Result<SyncReport, String> {
    let repo = find_repo(data_dir).ok_or_else(|| {
        format!("{} 及其上层目录都不是 git 仓；先 git init 或 git clone", data_dir.display())
    })?;
    if !has_remote(&repo) {
        return Err(
            "未配置 origin；先 git remote add origin <私有仓URL>（项目级可省——工作区仓已有 remote 则直接用）".into(),
        );
    }
    // data_dir 相对仓库根的前缀（仓本身时为空）
    let prefix = data_dir.strip_prefix(&repo).unwrap_or(Path::new(""));

    // 1. 只 add 白名单（仓根视角的相对路径）
    let files = whitelisted(data_dir);
    let add_paths: Vec<String> = files
        .iter()
        .map(|f| prefix.join(f).to_string_lossy().into_owned())
        .collect();
    if !add_paths.is_empty() {
        let mut args = vec!["add", "--"];
        for f in &add_paths {
            args.push(f);
        }
        git(&repo, &args)?;
    }

    // 2. 只提交白名单文件——commit 必须带路径限定！否则会把工作区里
    //    用户已暂存的无关改动一并卷进 memlite 的同步提交（2026-10-14 复现）。
    //    用 `git diff --cached --quiet -- <paths>` 判「白名单自身有无变更」。
    let staged = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["diff", "--cached", "--quiet", "--"])
        .args(&add_paths)
        .output()
        .map_err(|e| e.to_string())?;
    let committed = add_paths.is_empty() == false && !staged.status.success();
    if committed {
        let msg = format!("memlite sync {}", chrono_lite());
        let mut args = vec!["commit", "-m", &msg, "--"];
        for f in &add_paths {
            args.push(f);
        }
        git(&repo, &args)?;
    }

    // 3. fetch；远端分支已存在才 rebase（首推时远端可能为空仓）
    git(&repo, &["fetch", "origin"])?;
    let branch = git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let remote_ref = git(&repo, &["ls-remote", "--heads", "origin", &branch]).unwrap_or_default();
    if !remote_ref.is_empty() {
        // --autostash：工作区有用户未提交/已暂存改动时先 stash 再 rebase、完事还原——
        // 否则项目级记忆随工作区仓同步会被用户手头的半成品卡死（2026-10-14 复现）
        git(&repo, &["pull", "--rebase", "--autostash", "origin", &branch])?;
    }

    // 4. push -u
    git(&repo, &["push", "-u", "origin", &branch])?;

    // 5. 双通道核验
    let local_sha = git(&repo, &["rev-parse", "HEAD"])?;
    let ls = git(&repo, &["ls-remote", "origin", &format!("refs/heads/{branch}")])?;
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
        repo,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitelist_covers_querylog() {
        let d = std::env::temp_dir().join(format!("memlite-wl-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        for f in ["memory.jsonl", "memory.querylog.jsonl", "memory-1.jsonl", "emb.f32", "notes.txt"] {
            std::fs::write(d.join(f), "x").unwrap();
        }
        let w = whitelisted(&d);
        assert!(w.contains(&"memory.jsonl".to_string()));
        assert!(w.contains(&"memory.querylog.jsonl".to_string()), "检索日志须随 sync 走白名单: {w:?}");
        assert!(w.contains(&"memory-1.jsonl".to_string()));
        assert!(!w.iter().any(|f| f == "emb.f32" || f == "notes.txt"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn find_repo_walks_up() {
        // tmp 仓：data_dir 是仓内子目录时应找到外层仓
        let d = std::env::temp_dir().join(format!("memlite-synctest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let data = d.join("proj/memlite-data");
        std::fs::create_dir_all(&data).unwrap();
        std::process::Command::new("git").args(["init", "-q"]).arg(&d).output().unwrap();
        let repo = find_repo(&data).unwrap();
        assert_eq!(repo, d);
        // 无仓时 None
        let bare = std::env::temp_dir().join(format!("memlite-norepo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&bare);
        std::fs::create_dir_all(&bare).unwrap();
        // /tmp 本身无 .git，bare 向上到根应 None（除非 /tmp 是仓——非常态）
        if find_repo(&bare).is_some() {
            eprintln!("注意：/tmp 在 git 仓内，跳过 None 断言");
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
