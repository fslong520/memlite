//! 向量侧车：emb.f32（packed float32 LE）+ emb-ids.txt（id 顺序对齐）。
//! dim 自适应：首写定维，读时按 文件字节数/id 数 推导。

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub struct VectorIndex {
    dir: PathBuf,
    pub dim: Option<usize>,
    ids: Vec<String>,
    offset: HashMap<String, usize>,
    cache: Option<Vec<u8>>,
}

impl VectorIndex {
    pub fn open(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref().to_path_buf();
        let mut v = VectorIndex {
            dir: dir.clone(),
            dim: None,
            ids: Vec::new(),
            offset: HashMap::new(),
            cache: None,
        };
        if let Ok(text) = fs::read_to_string(dir.join("emb-ids.txt")) {
            for (i, line) in text.lines().enumerate() {
                if !line.is_empty() {
                    v.offset.insert(line.to_string(), i);
                    v.ids.push(line.to_string());
                }
            }
        }
        if v.dim.is_none() && !v.ids.is_empty() {
            if let Ok(md) = fs::metadata(dir.join("emb.f32")) {
                let n = md.len() as usize / 4;
                if n % v.ids.len() == 0 {
                    v.dim = Some(n / v.ids.len());
                }
            }
        }
        v
    }

    fn f32_path(&self) -> PathBuf {
        self.dir.join("emb.f32")
    }

    fn load_cache(&mut self) {
        if self.cache.is_some() {
            return;
        }
        self.cache = fs::read(self.f32_path()).ok();
    }

    pub fn get(&mut self, rid: &str) -> Option<Vec<f32>> {
        let idx = *self.offset.get(rid)?;
        let dim = self.dim?;
        self.load_cache();
        let raw = self.cache.as_ref()?;
        let start = idx * dim * 4;
        let end = start + dim * 4;
        if end > raw.len() {
            return None;
        }
        Some(
            raw[start..end]
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect(),
        )
    }

    pub fn put(&mut self, rid: &str, emb: &[f32]) -> Result<(), String> {
        if self.dim.is_none() {
            self.dim = Some(emb.len());
        }
        if Some(emb.len()) != self.dim {
            return Err(format!("向量维度不一致: {} != {:?}", emb.len(), self.dim));
        }
        self.cache = None; // 写前失效缓存
        let mut buf: Vec<u8> = Vec::with_capacity(emb.len() * 4);
        for x in emb {
            buf.extend_from_slice(&x.to_le_bytes());
        }
        if let Some(&idx) = self.offset.get(rid) {
            // 同 id 原地覆盖
            let mut f = fs::OpenOptions::new().write(true).open(self.f32_path()).map_err(|e| e.to_string())?;
            f.seek(SeekFrom::Start((idx * emb.len() * 4) as u64)).map_err(|e| e.to_string())?;
            f.write_all(&buf).map_err(|e| e.to_string())?;
            f.sync_all().map_err(|e| e.to_string())?;
            return Ok(());
        }
        {
            let mut f = fs::OpenOptions::new().create(true).append(true).open(self.f32_path()).map_err(|e| e.to_string())?;
            f.write_all(&buf).map_err(|e| e.to_string())?;
            f.sync_all().map_err(|e| e.to_string())?;
        }
        {
            let mut f = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.dir.join("emb-ids.txt")).map_err(|e| e.to_string())?;
            writeln!(f, "{rid}").map_err(|e| e.to_string())?;
            f.sync_all().map_err(|e| e.to_string())?;
        }
        self.offset.insert(rid.to_string(), self.ids.len());
        self.ids.push(rid.to_string());
        Ok(())
    }

    pub fn has(&self, rid: &str) -> bool {
        self.offset.contains_key(rid)
    }

    pub fn count(&self) -> usize {
        self.ids.len()
    }

}

#[allow(dead_code)]
fn _touch(_: &dyn Read) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("memlite-vec-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn put_get_roundtrip_dim_adaptive() {
        let d = tmp();
        let mut v = VectorIndex::open(&d);
        v.put("a", &[1.0, 2.0, 3.0]).unwrap();
        v.put("b", &[4.0, 5.0, 6.0]).unwrap();
        let mut v2 = VectorIndex::open(&d); // 重载从文件推 dim
        assert_eq!(v2.dim, Some(3));
        assert_eq!(v2.get("a").unwrap(), vec![1.0, 2.0, 3.0]);
        assert_eq!(v2.count(), 2);
        assert!(v2.put("c", &[1.0]).is_err()); // 维度不一致拒收
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn overwrite_in_place() {
        let d = tmp();
        let mut v = VectorIndex::open(&d);
        v.put("a", &[1.0, 0.0]).unwrap();
        v.put("a", &[0.0, 1.0]).unwrap();
        let mut v2 = VectorIndex::open(&d);
        assert_eq!(v2.count(), 1);
        assert_eq!(v2.get("a").unwrap(), vec![0.0, 1.0]);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn has_and_missing() {
        let d = tmp();
        let mut v = VectorIndex::open(&d);
        v.put("a", &[0.5]).unwrap();
        assert!(v.has("a"));
        assert!(!v.has("b"));
        assert!(v.get("b").is_none());
        std::fs::remove_dir_all(&d).ok();
    }
}
