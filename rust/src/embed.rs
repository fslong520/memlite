//! 嵌入客户端：OpenAI 兼容 /embeddings，配置三来源合并。
//! config.json 的 embedding 段 + MEMLITE_API_BASE/KEY/MODEL 环境变量覆盖。

use serde::Deserialize;
use std::path::PathBuf;

#[derive(Default, Deserialize)]
struct EmbedCfg {
    #[serde(rename = "api_base")]
    api_base: Option<String>,
    #[serde(rename = "api_key")]
    api_key: Option<String>,
    #[serde(rename = "model")]
    model: Option<String>,
    #[serde(rename = "timeout")]
    timeout: Option<u64>,
}

fn config_path() -> PathBuf {
    std::env::var("MEMLITE_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            PathBuf::from(home).join(".memlite/config.json")
        })
}

pub struct EmbedConfig {
    pub api_base: String,
    pub api_key: String,
    pub model: String,
    pub timeout: u64,
}

/// 合并 config 与环境变量；三件齐全才算配置了嵌入
pub fn embedding_config() -> Option<EmbedConfig> {
    let mut base = String::new();
    let mut key = String::new();
    let mut model = String::new();
    let mut timeout = 60u64;
    if let Ok(text) = std::fs::read_to_string(config_path()) {
        // config 形如 {"embedding": {api_base, api_key, model, timeout}, "data_dir": ...}
        if let Ok(root) = serde_json::from_str::<serde_json::Value>(&text) {
            let sec = root.get("embedding").cloned().unwrap_or(root);
            if let Ok(c) = serde_json::from_value::<EmbedCfg>(sec) {
                base = c.api_base.unwrap_or_default();
                key = c.api_key.unwrap_or_default();
                model = c.model.unwrap_or_default();
                timeout = c.timeout.unwrap_or(60);
            }
        }
    }
    if let Ok(v) = std::env::var("MEMLITE_API_BASE") {
        base = v;
    }
    if let Ok(v) = std::env::var("MEMLITE_API_KEY") {
        key = v;
    }
    if let Ok(v) = std::env::var("MEMLITE_EMBED_MODEL") {
        model = v;
    }
    if base.is_empty() || key.is_empty() || model.is_empty() {
        None
    } else {
        Some(EmbedConfig {
            api_base: base,
            api_key: key,
            model,
            timeout,
        })
    }
}

/// 单条嵌入；失败返回 Err（调用方按词面兜底）
pub fn embed(cfg: &EmbedConfig, text: &str) -> Result<Vec<f32>, String> {
    let url = format!("{}/embeddings", cfg.api_base.trim_end_matches('/'));
    let body = serde_json::json!({"model": cfg.model, "input": text});
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(cfg.timeout))
        .build();
    let resp = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {}", cfg.api_key))
        .send_json(body)
        .map_err(|e| format!("嵌入 API 不可达: {e}"))?;
    let v: serde_json::Value = resp.into_json().map_err(|e| e.to_string())?;
    v["data"][0]["embedding"]
        .as_array()
        .and_then(|arr| {
            arr.iter()
                .map(|x| x.as_f64().map(|f| f as f32))
                .collect::<Option<Vec<f32>>>()
        })
        .ok_or_else(|| format!("响应形状不合: {}", &v.to_string()[..200.min(v.to_string().len())]))
}
