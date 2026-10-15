//! 检索日志（query_log）：一次 recall/检索落一行，外加按条聚合的召回次数。
//! 训练与复盘取材用——写在 CLI recall 与 web /api/search 两条路上，bench 评测不入日志。
//!
//! 文件：data_dir/memory.querylog.jsonl（追加式；名字过白名单 memory*.jsonl 随 sync 走，
//! 且不撞 Store 的 memory-<n>.jsonl 归档命名）。次数不另设可变计数文件——
//! 全量聚合自日志，进程内常驻（web 起服务载一次、每次检索就地增量）。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

pub const LOG_FILE: &str = "memory.querylog.jsonl";
/// 载入内存保留的最近条数（recent/stats 展示窗口；counts 按全量行聚合）
const KEEP: usize = 50_000;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Hit {
    pub id: String,
    /// 原始分（未四舍五入）
    pub score: f64,
    #[serde(default)]
    pub title: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Entry {
    pub ts: f64,
    pub query: String,
    /// semantic | bm25
    pub backend: String,
    pub limit: usize,
    /// 空＝本库命中；"user"＝项目级无命中回退用户级
    #[serde(default)]
    pub fallback: String,
    /// cli | web
    #[serde(default)]
    pub source: String,
    /// 端到端耗时（毫秒，含回退那次）
    #[serde(default)]
    pub ms: u64,
    #[serde(default)]
    pub hits: Vec<Hit>,
}

/// 日志在进程内的常驻形态：尾窗条目 + 全量命中次数
pub struct Log {
    entries: Vec<Entry>,
    counts: HashMap<String, u64>,
    /// 全量行数（含被尾窗挤出的）
    total: u64,
    bad_lines: u64,
}

fn path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join(LOG_FILE)
}

impl Log {
    /// 载入：全文件扫一遍聚 counts（total/bad_lines 全量），entries 只留末 KEEP 条
    pub fn load(data_dir: &Path) -> Log {
        let mut log = Log {
            entries: Vec::new(),
            counts: HashMap::new(),
            total: 0,
            bad_lines: 0,
        };
        let Ok(text) = std::fs::read_to_string(path(data_dir)) else {
            return log;
        };
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        let start = lines.len().saturating_sub(KEEP);
        for (i, line) in lines.iter().enumerate() {
            match serde_json::from_str::<Entry>(line) {
                Ok(e) => {
                    for h in &e.hits {
                        *log.counts.entry(h.id.clone()).or_insert(0) += 1;
                    }
                    if i >= start {
                        log.entries.push(e);
                    }
                    log.total += 1;
                }
                Err(_) => log.bad_lines += 1,
            }
        }
        log
    }

    /// 落盘一行并就地更新内存视图（memory 持有时）
    pub fn append(&mut self, data_dir: &Path, e: Entry) -> std::io::Result<()> {
        append_line(data_dir, &e)?;
        for h in &e.hits {
            *self.counts.entry(h.id.clone()).or_insert(0) += 1;
        }
        self.total += 1;
        self.entries.push(e);
        if self.entries.len() > KEEP {
            let cut = self.entries.len() - KEEP;
            self.entries.drain(..cut);
        }
        Ok(())
    }

    pub fn count_of(&self, id: &str) -> u64 {
        self.counts.get(id).copied().unwrap_or(0)
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    /// 最近 n 条（新→旧）
    pub fn recent(&self, n: usize) -> Vec<&Entry> {
        self.entries.iter().rev().take(n).collect()
    }

    /// 汇总：次数、档位/来源分布、平均与零命中、命中最多的条目 top
    pub fn stats(&self, top: usize) -> Value {
        let mut since = f64::MAX;
        let mut until = 0f64;
        let (mut sem, mut bm, mut fb, mut cli, mut web) = (0u64, 0u64, 0u64, 0u64, 0u64);
        let (mut hit_sum, mut zero) = (0u64, 0u64);
        for e in &self.entries {
            since = since.min(e.ts);
            until = until.max(e.ts);
            match e.backend.as_str() {
                "semantic" => sem += 1,
                _ => bm += 1,
            }
            if !e.fallback.is_empty() {
                fb += 1;
            }
            match e.source.as_str() {
                "web" => web += 1,
                _ => cli += 1,
            }
            if e.hits.is_empty() {
                zero += 1;
            }
            hit_sum += e.hits.len() as u64;
        }
        let n = self.entries.len() as u64;
        // top 按次数排；标题取最近一次命中的
        let mut title_of: HashMap<&str, &str> = HashMap::new();
        for e in self.entries.iter().rev() {
            for h in &e.hits {
                title_of.entry(h.id.as_str()).or_insert(h.title.as_str());
            }
        }
        let mut topv: Vec<(&str, u64)> = self
            .counts
            .iter()
            .map(|(id, c)| (id.as_str(), *c))
            .collect();
        // 次数降序，同次数按 id 升序（HashMap 迭代无序，不钉死顺序输出会抖）
        topv.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        let top_arr: Vec<Value> = topv
            .into_iter()
            .take(top)
            .map(|(id, c)| {
                serde_json::json!({"id": id, "count": c, "title": title_of.get(id).copied().unwrap_or("")})
            })
            .collect();
        serde_json::json!({
            "queries_window": n,
            "queries_total": self.total,
            "bad_lines": self.bad_lines,
            "since": if n > 0 { since } else { 0.0 },
            "until": until,
            "backend": {"semantic": sem, "bm25": bm},
            "fallback_user": fb,
            "source": {"cli": cli, "web": web},
            "hits_avg": if n > 0 { (hit_sum as f64 / n as f64 * 100.0).round() / 100.0 } else { 0.0 },
            "zero_hit": zero,
            "distinct_recalled": self.counts.len(),
            "top": top_arr,
        })
    }

    /// 最近条目的 JSON 视图（分数四舍五入到 4 位，别把 f32 噪声写进训练集）
    pub fn recent_json(&self, n: usize) -> Vec<Value> {
        self.recent(n)
            .into_iter()
            .map(|e| {
                let hits: Vec<Value> = e
                    .hits
                    .iter()
                    .map(|h| {
                        serde_json::json!({
                            "id": h.id,
                            "score": (h.score * 10000.0).round() / 10000.0,
                            "title": h.title,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "ts": e.ts, "query": e.query, "backend": e.backend,
                    "limit": e.limit, "fallback": e.fallback, "source": e.source,
                    "ms": e.ms, "hits": hits,
                })
            })
            .collect()
    }
}

/// 只落盘、不载内存（CLI 短进程用）
pub fn append_line(data_dir: &Path, e: &Entry) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path(data_dir))?;
    writeln!(f, "{}", serde_json::to_string(e)?)?;
    Ok(())
}

/// 导出训练集：每行一次查询 {ts, query, backend, fallback, limit, positive(ids 按名次), hits}
/// since 为 unix 秒下限（0＝全部）；返回导出条数
pub fn export(data_dir: &Path, out: &Path, since: f64) -> Result<usize, String> {
    let text =
        std::fs::read_to_string(path(data_dir)).map_err(|e| format!("读日志失败: {e}"))?;
    let mut n = 0usize;
    let mut buf = String::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(e) = serde_json::from_str::<Entry>(line) else {
            continue;
        };
        if e.ts < since {
            continue;
        }
        let positive: Vec<&str> = e.hits.iter().map(|h| h.id.as_str()).collect();
        let v = serde_json::json!({
            "ts": e.ts,
            "query": e.query,
            "backend": e.backend,
            "fallback": e.fallback,
            "limit": e.limit,
            "positive": positive,
            "hits": e.hits.iter().map(|h| serde_json::json!({
                "id": h.id, "score": (h.score * 10000.0).round() / 10000.0, "title": h.title
            })).collect::<Vec<_>>(),
        });
        buf.push_str(&serde_json::to_string(&v).map_err(|e| e.to_string())?);
        buf.push('\n');
        n += 1;
    }
    if let Some(p) = out.parent() {
        if !p.as_os_str().is_empty() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
    }
    std::fs::write(out, buf).map_err(|e| format!("写导出文件失败: {e}"))?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("memlite-qlog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn entry(q: &str, ids: &[&str]) -> Entry {
        Entry {
            ts: 1000.0,
            query: q.into(),
            backend: "semantic".into(),
            limit: 10,
            fallback: "".into(),
            source: "cli".into(),
            ms: 12,
            hits: ids
                .iter()
                .map(|i| Hit {
                    id: (*i).into(),
                    score: 0.5,
                    title: "题".into(),
                })
                .collect(),
        }
    }

    #[test]
    fn append_then_load_counts() {
        let d = tmp();
        append_line(&d, &entry("甲", &["a", "b"])).unwrap();
        append_line(&d, &entry("乙", &["b", "c"])).unwrap();
        let log = Log::load(&d);
        assert_eq!(log.total(), 2);
        assert_eq!(log.count_of("b"), 2);
        assert_eq!(log.count_of("a"), 1);
        assert_eq!(log.count_of("zz"), 0);
        assert_eq!(log.recent(1)[0].query, "乙");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn append_updates_in_memory() {
        let d = tmp();
        let mut log = Log::load(&d);
        log.append(&d, entry("甲", &["a"])).unwrap();
        log.append(&d, entry("乙", &["a", "b"])).unwrap();
        assert_eq!(log.count_of("a"), 2);
        assert_eq!(log.total(), 2);
        // 与冷载一致
        let cold = Log::load(&d);
        assert_eq!(cold.count_of("a"), 2);
        assert_eq!(cold.total(), 2);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn bad_lines_counted_not_fatal() {
        let d = tmp();
        append_line(&d, &entry("甲", &["a"])).unwrap();
        std::fs::write(d.join(LOG_FILE), "{坏行}\n").unwrap();
        let log = Log::load(&d);
        assert_eq!(log.total(), 0);
        assert_eq!(log.stats(1)["bad_lines"], 1);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn stats_and_export() {
        let d = tmp();
        append_line(&d, &entry("甲", &["a", "b"])).unwrap();
        let mut miss = entry("乙", &[]);
        miss.backend = "bm25".into();
        miss.source = "web".into();
        miss.fallback = "user".into();
        append_line(&d, &miss).unwrap();
        let log = Log::load(&d);
        let st = log.stats(5);
        assert_eq!(st["queries_total"], 2);
        assert_eq!(st["zero_hit"], 1);
        assert_eq!(st["backend"]["semantic"], 1);
        assert_eq!(st["source"]["web"], 1);
        assert_eq!(st["fallback_user"], 1);
        // 并列次数的排序不稳定，按 id 查而非取首元素
        let top = st["top"].as_array().unwrap();
        assert_eq!(top.len(), 2);
        for want in ["a", "b"] {
            let hit = top.iter().find(|t| t["id"] == want);
            assert!(hit.is_some(), "top 应含 {want}");
            assert_eq!(hit.unwrap()["count"], 1);
        }
        assert_eq!(st["distinct_recalled"], 2);

        let out = d.join("export.jsonl");
        let n = export(&d, &out, 0.0).unwrap();
        assert_eq!(n, 2);
        let text = std::fs::read_to_string(&out).unwrap();
        let first: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(first["query"], "甲");
        assert_eq!(first["positive"][0], "a");
        // since 过滤
        assert_eq!(export(&d, &out, 5000.0).unwrap(), 0);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
