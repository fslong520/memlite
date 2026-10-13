//! 存储层：append-only JSONL + 16MB 分块轮转 + 内存索引。
//! 语义与 Python 版一致：归档旧→新、活跃文件恒末，末写胜出。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const DATA_FILE: &str = "memory.jsonl";
pub const CHUNK_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Serialize, Deserialize, Clone)]
pub struct Record {
    pub op: String,
    pub id: String,
    pub ts: f64,
    #[serde(default)]
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub project: String,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

pub struct Store {
    pub data_dir: PathBuf,
    pub records: HashMap<String, Record>,
    pub deleted: HashSet<String>,
    pub bad_lines: u64,
    chunk_bytes: u64,
}

impl Store {
    pub fn open(data_dir: impl AsRef<Path>) -> Self {
        Self::open_with_chunk(data_dir, CHUNK_BYTES)
    }

    pub fn open_with_chunk(data_dir: impl AsRef<Path>, chunk_bytes: u64) -> Self {
        let mut s = Store {
            data_dir: data_dir.as_ref().to_path_buf(),
            records: HashMap::new(),
            deleted: HashSet::new(),
            bad_lines: 0,
            chunk_bytes,
        };
        s.load();
        s
    }

    /// 归档按序号升序（旧→新）
    fn archive_paths(&self) -> Vec<PathBuf> {
        let mut found: Vec<(u32, PathBuf)> = Vec::new();
        if let Ok(rd) = fs::read_dir(&self.data_dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if let Some(rest) = name.strip_prefix("memory-") {
                    if let Some(num) = rest.strip_suffix(".jsonl") {
                        if let Ok(n) = num.parse::<u32>() {
                            found.push((n, e.path()));
                        }
                    }
                }
            }
        }
        found.sort_by_key(|(n, _)| *n);
        found.into_iter().map(|(_, p)| p).collect()
    }

    /// 归档在前（旧），活跃在后（新）
    fn files(&self) -> Vec<PathBuf> {
        let mut paths = self.archive_paths();
        let active = self.data_dir.join(DATA_FILE);
        if active.exists() {
            paths.push(active);
        }
        paths
    }

    fn load(&mut self) {
        for path in self.files() {
            let Ok(text) = fs::read_to_string(&path) else { continue };
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_str::<Record>(line) {
                    Ok(op) => self.apply(op),
                    Err(_) => self.bad_lines += 1,
                }
            }
        }
    }

    fn apply(&mut self, op: Record) {
        match op.op.as_str() {
            "put" => {
                let id = op.id.clone();
                self.records.insert(id.clone(), op);
                self.deleted.remove(&id);
            }
            "del" => {
                self.deleted.insert(op.id.clone());
                self.records.remove(&op.id);
            }
            _ => self.bad_lines += 1,
        }
    }

    fn append(&mut self, rec: &Record) -> std::io::Result<()> {
        fs::create_dir_all(&self.data_dir)?;
        let active = self.data_dir.join(DATA_FILE);
        let line = serde_json::to_string(rec)?;
        let mut f = fs::OpenOptions::new().create(true).append(true).open(&active)?;
        writeln!(f, "{line}")?;
        f.sync_all()?;
        self.maybe_rotate()?;
        Ok(())
    }

    /// 活跃文件超限即轮转（改名归档，取最大序号+1）
    fn maybe_rotate(&self) -> std::io::Result<()> {
        let active = self.data_dir.join(DATA_FILE);
        let Ok(md) = fs::metadata(&active) else { return Ok(()) };
        if md.len() < self.chunk_bytes {
            return Ok(());
        }
        let archives = self.archive_paths();
        let seq = archives
            .last()
            .and_then(|p| {
                p.file_name()?
                    .to_str()?
                    .strip_prefix("memory-")?
                    .strip_suffix(".jsonl")?
                    .parse::<u32>()
                    .ok()
            })
            .unwrap_or(0)
            + 1;
        fs::rename(&active, self.data_dir.join(format!("memory-{seq:04}.jsonl")))?;
        Ok(())
    }

    pub fn put(&mut self, content: &str, title: &str, tags: &[String], project: &str) -> Result<String, String> {
        if content.trim().is_empty() {
            return Err("content 为空".into());
        }
        let rid = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let rec = Record {
            op: "put".into(),
            id: rid.clone(),
            ts: now_ts(),
            title: title.trim().into(),
            content: content.trim().into(),
            tags: tags.iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
            project: project.trim().into(),
            extra: HashMap::new(),
        };
        self.append(&rec).map_err(|e| e.to_string())?;
        self.apply(rec);
        Ok(rid)
    }

    pub fn delete(&mut self, rid: &str) -> Result<(), String> {
        if !self.records.contains_key(rid) {
            return Err(format!("记忆不存在: {rid}"));
        }
        let rec = Record {
            op: "del".into(),
            id: rid.to_string(),
            ts: now_ts(),
            title: String::new(),
            content: String::new(),
            tags: vec![],
            project: String::new(),
            extra: HashMap::new(),
        };
        self.append(&rec).map_err(|e| e.to_string())?;
        self.apply(rec);
        Ok(())
    }

    /// 就地改：同 id 追加新 put（末写胜出，id 不变）
    pub fn put_with_id(
        &mut self,
        rid: &str,
        content: &str,
        title: &str,
        tags: &[String],
        project: &str,
    ) -> Result<(), String> {
        if content.trim().is_empty() {
            return Err("content 为空".into());
        }
        let rec = Record {
            op: "put".into(),
            id: rid.to_string(),
            ts: now_ts(),
            title: title.trim().into(),
            content: content.trim().into(),
            tags: tags.iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
            project: project.trim().into(),
            extra: HashMap::new(),
        };
        self.append(&rec).map_err(|e| e.to_string())?;
        self.apply(rec);
        Ok(())
    }

    pub fn count(&self) -> usize {
        self.records.len()
    }

    pub fn all(&self) -> Vec<&Record> {
        self.records.values().collect()
    }

    pub fn get(&self, rid: &str) -> Option<&Record> {
        self.records.get(rid)
    }

    /// 短 id 前缀解析
    pub fn resolve(&self, prefix: &str) -> Option<&Record> {
        if let Some(r) = self.records.get(prefix) {
            return Some(r);
        }
        let mut it = self.records.values().filter(|r| r.id.starts_with(prefix));
        let hit = it.next();
        if it.next().is_some() {
            None // 歧义
        } else {
            hit
        }
    }

    pub fn line_count(&self) -> u64 {
        self.files()
            .iter()
            .map(|p| {
                fs::read_to_string(p)
                    .map(|t| t.lines().count() as u64)
                    .unwrap_or(0)
            })
            .sum()
    }

    pub fn file_stats(&self) -> Vec<(String, u64)> {
        self.files()
            .iter()
            .map(|p| {
                let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let size = fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                (name, size)
            })
            .collect()
    }

    /// 压实：只留存活重写回单文件，删归档
    pub fn compact(&mut self) -> std::io::Result<u64> {
        let before = self.line_count();
        let active = self.data_dir.join(DATA_FILE);
        let tmp = self.data_dir.join(format!("{DATA_FILE}.tmp"));
        {
            let mut f = fs::File::create(&tmp)?;
            for rec in self.records.values() {
                writeln!(f, "{}", serde_json::to_string(rec)?)?;
            }
            f.sync_all()?;
        }
        for p in self.archive_paths() {
            fs::remove_file(p)?;
        }
        fs::rename(&tmp, active)?;
        Ok(before)
    }
}

pub fn now_ts() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("memlite-rs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn put_get_delete_reload() {
        let d = tmp();
        let mut s = Store::open(&d);
        let rid = s.put("测试记忆：Python 3.12", "py312", &["python".into()], "demo").unwrap();
        assert_eq!(s.count(), 1);
        assert_eq!(s.get(&rid).unwrap().tags, vec!["python"]);
        s.delete(&rid).unwrap();
        assert_eq!(s.count(), 0);
        let s2 = Store::open(&d);
        assert!(s2.get(&rid).is_none());
        assert!(s2.deleted.contains(&rid));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn empty_content_rejected() {
        let d = tmp();
        let mut s = Store::open(&d);
        assert!(s.put("   ", "", &[], "").is_err());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn bad_line_tolerated() {
        let d = tmp();
        let mut s = Store::open(&d);
        s.put("正常条", "", &[], "").unwrap();
        let mut f = fs::OpenOptions::new().append(true).open(d.join(DATA_FILE)).unwrap();
        writeln!(f, "{{bad json").unwrap();
        writeln!(f, r#"{{"op":"put","id":"x1","ts":1.0,"title":"","content":"坏行后正常条","tags":[],"project":""}}"#).unwrap();
        let s2 = Store::open(&d);
        assert_eq!(s2.bad_lines, 1);
        assert_eq!(s2.count(), 2);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn rotation_and_reload() {
        let d = tmp();
        let mut s = Store::open_with_chunk(&d, 500);
        for i in 0..20 {
            s.put(&format!("记忆内容第 {i} 条，{}", "充长度".repeat(10)), &format!("t{i}"), &[], "").unwrap();
        }
        let stats = s.file_stats();
        assert!(stats.len() > 1, "应发生轮转");
        assert_eq!(stats.last().unwrap().0, DATA_FILE);
        let s2 = Store::open_with_chunk(&d, 500);
        assert_eq!(s2.count(), 20);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn tombstone_in_new_chunk_overrides_old() {
        let d = tmp();
        let mut s = Store::open_with_chunk(&d, 400);
        let rid = s.put("第一条老记忆", "old", &[], "").unwrap();
        for i in 0..10 {
            s.put(&format!("填充记忆 {i} {}", "字".repeat(30)), &format!("f{i}"), &[], "").unwrap();
        }
        s.delete(&rid).unwrap();
        let s2 = Store::open_with_chunk(&d, 400);
        assert!(s2.get(&rid).is_none());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn compact_collapses_chunks() {
        let d = tmp();
        let mut s = Store::open_with_chunk(&d, 400);
        s.put("存活条", "k", &[], "").unwrap();
        for i in 0..10 {
            s.put(&format!("填充 {i} {}", "字".repeat(30)), &format!("f{i}"), &[], "").unwrap();
        }
        s.compact().unwrap();
        assert_eq!(s.file_stats().len(), 1);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn short_id_prefix_resolve() {
        let d = tmp();
        let mut s = Store::open(&d);
        let rid = s.put("x", "", &[], "").unwrap();
        assert!(s.resolve(&rid[..6]).is_some());
        assert!(s.resolve("zzzz").is_none());
        std::fs::remove_dir_all(&d).ok();
    }
}
