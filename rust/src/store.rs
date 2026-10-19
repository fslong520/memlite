//! 存储层：append-only JSONL + 16MB 分块轮转 + 内存索引。
//! 归档旧→新、活跃文件恒末，末写胜出。

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

impl Record {
    /// 因果树父指针：extra.parent_id（迁移库带此字段；无则空串=根）。
    pub fn parent_id(&self) -> &str {
        self.extra.get("parent_id").and_then(|v| v.as_str()).unwrap_or("")
    }

    /// 重要性（respire 迁移数据 important/trivial；新条可缺省）。
    pub fn importance(&self) -> &str {
        self.extra.get("importance").and_then(|v| v.as_str()).unwrap_or("")
    }

    /// 记忆形态（Context/Task/Skill/...；迁移库带，新条可缺省）。
    pub fn kind(&self) -> &str {
        self.extra.get("kind").and_then(|v| v.as_str()).unwrap_or("")
    }
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
        self.put_child(content, title, tags, project, "")
    }

    /// 存一条并显式认因（parent 非空须已在库——不产生悬空父指针；空串=根）。
    pub fn put_child(
        &mut self,
        content: &str,
        title: &str,
        tags: &[String],
        project: &str,
        parent: &str,
    ) -> Result<String, String> {
        if content.trim().is_empty() {
            return Err("content 为空".into());
        }
        if !parent.is_empty() && !self.records.contains_key(parent) {
            return Err(format!("目标因（父）不存在: {parent}"));
        }
        let rid = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let mut extra = HashMap::new();
        extra.insert("parent_id".to_string(), Value::String(parent.to_string()));
        let rec = Record {
            op: "put".into(),
            id: rid.clone(),
            ts: now_ts(),
            title: title.trim().into(),
            content: content.trim().into(),
            tags: tags.iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
            project: project.trim().into(),
            extra,
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

    /// 就地改：同 id 追加新 put（末写胜出，id 不变）。
    /// extra 全量保留（parent_id/importance/kind 随迁——改前文不能丢因链，2026-10-16 修）。
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
        let extra = self
            .records
            .get(rid)
            .map(|r| r.extra.clone())
            .unwrap_or_default();
        let rec = Record {
            op: "put".into(),
            id: rid.to_string(),
            ts: now_ts(),
            title: title.trim().into(),
            content: content.trim().into(),
            tags: tags.iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
            project: project.trim().into(),
            extra,
        };
        self.append(&rec).map_err(|e| e.to_string())?;
        self.apply(rec);
        Ok(())
    }

    /// 改挂因（新父）：追加同 id 新 put，仅换 parent_id（空串=升根）。
    /// 防环由调用方查证（reparent_checked）；本函数只保证两节点存在。
    pub fn reparent(&mut self, rid: &str, new_parent: &str) -> Result<(), String> {
        let Some(old) = self.records.get(rid).cloned() else {
            return Err(format!("记忆不存在: {rid}"));
        };
        if !new_parent.is_empty() && !self.records.contains_key(new_parent) {
            return Err(format!("目标因（父）不存在: {new_parent}"));
        }
        let mut rec = old;
        rec.extra.insert("parent_id".to_string(), Value::String(new_parent.to_string()));
        // ts 保留原值：改挂是结构调整，不该把老条顶进 list「最近」时间线
        self.append(&rec).map_err(|e| e.to_string())?;
        self.apply(rec);
        Ok(())
    }

    /// 改挂带防环：新父不得为本条、也不得在本条因链上（即新父是本条后代）。
    pub fn reparent_checked(&mut self, rid: &str, new_parent: &str) -> Result<(), String> {
        if rid == new_parent {
            return Err("不能挂自己".into());
        }
        if !new_parent.is_empty() {
            for anc in self.ancestor_chain(new_parent) {
                if anc == rid {
                    return Err("防环：新父是本条的后代".into());
                }
            }
        }
        self.reparent(rid, new_parent)
    }

    /// 因链回溯（父→祖父→…），返回**自根至父**顺序（不含本条）。
    /// 防环封顶 64 层（脏数据不挂死）；父不在库即断链止步。
    pub fn ancestor_chain(&self, rid: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut cur = rid.to_string();
        for _ in 0..64 {
            let Some(pid) = self.records.get(&cur).map(|r| r.parent_id().to_string()) else {
                break;
            };
            if pid.is_empty() {
                break;
            }
            chain.push(pid.clone());
            cur = pid;
        }
        chain.reverse();
        chain
    }

    /// 直接子条（果），按 ts 升序（存入序，树渲染稳定）。
    pub fn children(&self, rid: &str) -> Vec<&Record> {
        let mut v: Vec<&Record> = self.records.values().filter(|r| r.parent_id() == rid).collect();
        v.sort_by(|a, b| a.ts.partial_cmp(&b.ts).unwrap_or(std::cmp::Ordering::Equal));
        v
    }

    /// 根集：parent_id 为空（含老数据无此字段）。
    #[allow(dead_code)] // bin 主流程走 TreeScope.roots；测试用
    pub fn roots(&self) -> Vec<&Record> {
        let mut v: Vec<&Record> = self.records.values().filter(|r| r.parent_id().is_empty()).collect();
        v.sort_by(|a, b| a.ts.partial_cmp(&b.ts).unwrap_or(std::cmp::Ordering::Equal));
        v
    }

    pub fn count(&self) -> usize {
        self.records.len()
    }

    pub fn all(&self) -> Vec<&Record> {
        self.records.values().collect()
    }

    #[allow(dead_code)] // bin 主流程用 resolve；测试与库消费者用 get
    pub fn get(&self, rid: &str) -> Option<&Record> {
        self.records.get(rid)
    }

    /// id / 纲名（精确标题唯一命中）/ 前缀 三级解析——与 respire resolve_prefix 同序。
    pub fn resolve(&self, prefix: &str) -> Option<&Record> {
        let prefix = prefix.trim_start_matches('#');
        if let Some(r) = self.records.get(prefix) {
            return Some(r);
        }
        // 纲名直挂：精确标题唯一命中即取（词库纲名即挂载名）
        let mut by_title = self.records.values().filter(|r| r.title == prefix);
        if let Some(hit) = by_title.next() {
            if by_title.next().is_none() {
                return Some(hit);
            }
        }
        let mut it = self.records.values().filter(|r| r.id.starts_with(prefix));
        let hit = it.next();
        if it.next().is_some() {
            None // 歧义
        } else {
            hit
        }
    }

    /// 同 id 的全部历史版本（append-only 日志天然保历史）：
    /// 按文件序（旧→新）扫 put 行，返回按时间排列的版本列表（含被 update 覆盖的旧版）。
    pub fn versions(&self, rid: &str) -> Vec<Record> {
        let mut out = Vec::new();
        for path in self.files() {
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() { continue }
                if let Ok(rec) = serde_json::from_str::<Record>(line) {
                    if rec.op == "put" && rec.id == rid {
                        out.push(rec);
                    }
                }
            }
        }
        out
    }

    /// 全库标签频次（降序）
    pub fn tag_counts(&self) -> Vec<(String, usize)> {
        let mut m: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for r in self.records.values() {
            for t in &r.tags {
                *m.entry(t.clone()).or_insert(0) += 1;
            }
        }
        let mut v: Vec<(String, usize)> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }

    /// 全库 project 频次（降序，空 project 归「（无）」）
    pub fn project_counts(&self) -> Vec<(String, usize)> {
        let mut m: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for r in self.records.values() {
            let k = if r.project.is_empty() { "（无）".to_string() } else { r.project.clone() };
            *m.entry(k).or_insert(0) += 1;
        }
        let mut v: Vec<(String, usize)> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
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
    fn querylog_file_not_loaded_as_records() {
        let d = tmp();
        let mut s = Store::open(&d);
        s.put("真记忆", "真", &[], "").unwrap();
        // 检索日志同目录共存：既不当归档分块，也不计入坏行
        std::fs::write(
            d.join("memory.querylog.jsonl"),
            r#"{"ts":1.0,"query":"q","backend":"bm25","limit":10,"hits":[]}"#,
        )
        .unwrap();
        let s2 = Store::open(&d);
        assert_eq!(s2.count(), 1);
        assert_eq!(s2.bad_lines, 0);
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

    #[test]
    fn resolve_by_exact_title_unique() {
        let d = tmp();
        let mut s = Store::open(&d);
        let rid = s.put("x", "编程开发", &[], "").unwrap();
        assert_eq!(s.resolve("编程开发").unwrap().id, rid);
        // 重复标题不取（歧义回落前缀路）
        s.put("y", "编程开发", &[], "").unwrap();
        assert!(s.resolve("编程开发").is_none());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn put_child_requires_existing_parent() {
        let d = tmp();
        let mut s = Store::open(&d);
        assert!(s.put_child("c", "", &[], "", "nope").is_err());
        let pid = s.put_child("p", "父", &[], "", "").unwrap();
        let cid = s.put_child("c", "子", &[], "", &pid).unwrap();
        assert_eq!(s.get(&cid).unwrap().parent_id(), pid);
        assert_eq!(s.ancestor_chain(&cid), vec![pid.clone()]);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn reparent_checked_blocks_cycle_and_self() {
        let d = tmp();
        let mut s = Store::open(&d);
        let a = s.put_child("a", "甲", &[], "", "").unwrap();
        let b = s.put_child("b", "乙", &[], "", &a).unwrap();
        let c = s.put_child("c", "丙", &[], "", &b).unwrap();
        // 自挂
        assert!(s.reparent_checked(&a, &a).is_err());
        // 挂后代成环（a 挂到 c 下 → c 是 a 后代）
        assert!(s.reparent_checked(&a, &c).is_err());
        // 合法改挂：c 升到 a 下
        s.reparent_checked(&c, &a).unwrap();
        assert_eq!(s.ancestor_chain(&c), vec![a.clone()]);
        // 升根
        s.reparent_checked(&b, "").unwrap();
        assert!(s.ancestor_chain(&b).is_empty());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn update_put_with_id_preserves_parent_and_meta() {
        let d = tmp();
        let mut s = Store::open(&d);
        let pid = s.put_child("p", "父", &[], "", "").unwrap();
        let cid = s.put_child("c", "子", &[], "", &pid).unwrap();
        // 直写一行带 importance/kind 的 put（模拟迁移库字段），再走 update
        let line = format!(
            r#"{{"op":"put","id":"{cid}","ts":2.0,"title":"子","content":"c","tags":[],"project":"","parent_id":"{pid}","importance":"important","kind":"Task"}}"#
        );
        use std::io::Write;
        let mut f = fs::OpenOptions::new().append(true).open(d.join(DATA_FILE)).unwrap();
        writeln!(f, "{line}").unwrap();
        drop(f);
        // 重开装载手写行（内存索引不会自发重载），再走 update
        let mut s = Store::open(&d);
        s.put_with_id(&cid, "改后正文", "改题", &["t".into()], "").unwrap();
        let after = s.get(&cid).unwrap();
        assert_eq!(after.parent_id(), pid, "update 后 parent_id 丢失");
        assert_eq!(after.importance(), "important", "update 后 importance 丢失");
        assert_eq!(after.kind(), "Task", "update 后 kind 丢失");
        assert_eq!(after.content, "改后正文");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn children_sorted_by_ts_roots_excluded() {
        let d = tmp();
        let mut s = Store::open(&d);
        let p = s.put_child("p", "父", &[], "", "").unwrap();
        let k1 = s.put_child("k1", "", &[], "", &p).unwrap();
        let k2 = s.put_child("k2", "", &[], "", &p).unwrap();
        let kids: Vec<String> = s.children(&p).iter().map(|r| r.id.clone()).collect();
        assert_eq!(kids, vec![k1.clone(), k2.clone()]);
        assert_eq!(s.roots().len(), 1);
        assert_eq!(s.children("").len(), 1); // 空父=根集
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn ancestor_chain_survives_dirty_cycle_data() {
        // 脏数据造环（绕过 reparent_checked 直写文件），chain 封顶不挂死
        let d = tmp();
        let mut s = Store::open(&d);
        let a = s.put_child("a", "", &[], "", "").unwrap();
        let b = s.put_child("b", "", &[], "", &a).unwrap();
        let _ = s.reparent(&a, &b); // 故意造环：a→b→a
        let chain = Store::open(&d).ancestor_chain(&a);
        assert!(chain.len() <= 64, "chain 封顶 64 层，实得 {}", chain.len());
        std::fs::remove_dir_all(&d).ok();
    }
}
