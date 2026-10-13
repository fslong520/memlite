//! 检索层：BM25-lite（中英混合分词）+ 余弦语义，与 Python 版同参数。

use crate::store::Record;
use std::collections::{HashMap, HashSet};

/// 中英混合分词：ASCII 整词小写；CJK 段取单字+bigram
pub fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut ascii = String::new();
    let mut cjk = String::new();
    let flush_ascii = |a: &mut String, out: &mut Vec<String>| {
        if !a.is_empty() {
            out.push(a.to_lowercase());
            a.clear();
        }
    };
    let flush_cjk = |c: &mut String, out: &mut Vec<String>| {
        let chars: Vec<char> = c.chars().collect();
        for ch in &chars {
            out.push(ch.to_string());
        }
        for w in chars.windows(2) {
            let s: String = w.iter().collect();
            out.push(s);
        }
        c.clear();
    };
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            flush_cjk(&mut cjk, &mut out);
            ascii.push(ch);
        } else if is_cjk(ch) {
            flush_ascii(&mut ascii, &mut out);
            cjk.push(ch);
        } else {
            flush_ascii(&mut ascii, &mut out);
            flush_cjk(&mut cjk, &mut out);
        }
    }
    flush_ascii(&mut ascii, &mut out);
    flush_cjk(&mut cjk, &mut out);
    out
}

fn is_cjk(c: char) -> bool {
    // 与 Python 版 _TOKEN_RE [一-鿿] 严格一致：仅 U+4E00–U+9FFF。
    // 全角标点（，。 U+3000/FF00 段）是分隔符不是词——当词切会改变文档长度，
    // BM25 分数随之漂移（对拍发现过 0.7% 差）。
    matches!(c as u32, 0x4E00..=0x9FFF)
}

/// 记录的检索文本：title×3 权重 + content + tags
fn doc_tokens(r: &Record) -> Vec<String> {
    let mut toks = tokenize(&r.content);
    toks.extend(tokenize(&r.tags.join(" ")));
    for _ in 0..3 {
        toks.extend(tokenize(&r.title));
    }
    toks
}

/// BM25 打分（k1=1.2, b=0.75），降序返回 (score, record)
pub fn bm25<'a>(records: &[&'a Record], query: &str, limit: usize, project: &str) -> Vec<(f64, &'a Record)> {
    let q = tokenize(query);
    if q.is_empty() {
        return vec![];
    }
    let mut qtf: HashMap<String, usize> = HashMap::new();
    for t in &q {
        *qtf.entry(t.clone()).or_insert(0) += 1;
    }
    let pool: Vec<&Record> = records
        .iter()
        .copied()
        .filter(|r| project.is_empty() || r.project == project)
        .collect();
    if pool.is_empty() {
        return vec![];
    }
    let docs: Vec<(HashMap<String, usize>, usize)> = pool
        .iter()
        .map(|r| {
            let toks = doc_tokens(r);
            let mut tf: HashMap<String, usize> = HashMap::new();
            for t in &toks {
                *tf.entry(t.clone()).or_insert(0) += 1;
            }
            (tf, toks.len())
        })
        .collect();
    let mut df: HashMap<String, usize> = HashMap::new();
    for (tf, _) in &docs {
        for t in tf.keys() {
            *df.entry(t.clone()).or_insert(0) += 1;
        }
    }
    let n = docs.len() as f64;
    let avg_dl = docs.iter().map(|(_, l)| *l).sum::<usize>() as f64 / n;
    let (k1, b) = (1.2, 0.75);
    let mut scored: Vec<(f64, &Record)> = Vec::new();
    for (i, (tf, dl)) in docs.iter().enumerate() {
        let mut s = 0.0f64;
        for (term, qf) in &qtf {
            let Some(&tfc) = tf.get(term) else { continue };
            let d = *df.get(term).unwrap_or(&1) as f64;
            let idf = (1.0 + (n - d + 0.5) / (d + 0.5)).ln();
            s += *qf as f64
                * idf
                * (tfc as f64 * (k1 + 1.0))
                / (tfc as f64 + k1 * (1.0 - b + b * *dl as f64 / avg_dl.max(1e-9)));
        }
        if s > 0.0 {
            scored.push((s, pool[i]));
        }
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(b.1.ts.partial_cmp(&a.1.ts).unwrap()));
    scored.truncate(limit);
    scored
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// 余弦语义：vec_getter 从侧车取向量，只排有向量的记录
pub fn semantic<'a, F: FnMut(&str) -> Option<Vec<f32>>>(
    records: &[&'a Record],
    query_emb: &[f32],
    limit: usize,
    project: &str,
    getter: &mut F,
) -> Vec<(f64, &'a Record)> {
    if query_emb.is_empty() {
        return vec![];
    }
    let mut scored: Vec<(f64, &Record)> = Vec::new();
    for r in records.iter() {
        if !project.is_empty() && r.project != project {
            continue;
        }
        let Some(v) = getter(&r.id) else { continue };
        let s = cosine(query_emb, &v);
        if s > 0.0 {
            scored.push((s as f64, *r));
        }
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(b.1.ts.partial_cmp(&a.1.ts).unwrap()));
    scored.truncate(limit);
    scored
}

/// 标题铁律：缺省截正文首句 ≤40 字
pub fn default_title(content: &str) -> String {
    let head = content.lines().next().unwrap_or("");
    let mut cut = head;
    for sep in ["。", "！", "？", ". ", "! ", "? "] {
        if let Some(p) = head.find(sep) {
            let cand = &head[..p];
            if cand.len() < cut.len() {
                cut = cand;
            }
        }
    }
    // 截 40 字节（中文 3 字节/字，粗取 40 字符上限）
    cut.chars().take(40).collect()
}

#[allow(dead_code)]
fn _unused(_: &HashSet<u8>) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, ts: f64, title: &str, content: &str) -> Record {
        Record {
            op: "put".into(), id: id.into(), ts,
            title: title.into(), content: content.into(),
            tags: vec![], project: String::new(),
            extra: Default::default(),
        }
    }

    #[test]
    fn tokenize_mixed() {
        let t = tokenize("memlite 是 Python 写的");
        assert!(t.contains(&"memlite".to_string()));
        assert!(t.contains(&"python".to_string()));
        assert!(t.contains(&"写".to_string()));
    }

    #[test]
    fn tokenize_cjk_bigram_plus_unigram() {
        let t = tokenize("记忆系统");
        assert!(t.contains(&"记忆".to_string()));
        assert!(t.contains(&"忆系".to_string()));
        assert!(t.contains(&"系统".to_string()));
        assert!(t.contains(&"记".to_string())); // 单字保查询可中
    }

    #[test]
    fn bm25_finds_and_ranks() {
        let r1 = rec("1", 1.0, "rust 编译", "cargo build 慢用 sccache");
        let r2 = rec("2", 2.0, "菜谱", "番茄炒蛋的做法");
        let r3 = rec("3", 3.0, "", "rust 编译缓存 sccache 配置");
        let all = vec![&r1, &r2, &r3];
        let hits = bm25(&all, "rust 编译", 3, "");
        assert!(!hits.is_empty());
        assert!(hits[0].1.id == "1" || hits[0].1.id == "3");
    }

    #[test]
    fn bm25_title_weight() {
        let r1 = rec("1", 1.0, "kubernetes 部署", "无关内容");
        let r2 = rec("2", 2.0, "别的", "提到一次 kubernetes");
        let all = vec![&r1, &r2];
        let hits = bm25(&all, "kubernetes", 3, "");
        assert_eq!(hits[0].1.id, "1");
    }

    #[test]
    fn bm25_project_filter() {
        let mut r1 = rec("1", 1.0, "", "金秤 策略");
        r1.project = "goldscale".into();
        let r2 = rec("2", 2.0, "", "金秤 记忆");
        let all = vec![&r1, &r2];
        let hits = bm25(&all, "金秤", 3, "goldscale");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1.id, "1");
    }

    #[test]
    fn cosine_range_and_mismatch() {
        assert_eq!(cosine(&[1.0, 0.0], &[1.0, 0.0]), 1.0);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0);
        assert_eq!(cosine(&[], &[1.0]), 0.0);
    }

    #[test]
    fn default_title_first_sentence() {
        assert_eq!(default_title("nginx 反代 504 超时。调大 timeout 解决。"), "nginx 反代 504 超时");
        let long = "很长的正文".repeat(30);
        assert!(default_title(&long).chars().count() <= 40);
    }
}
