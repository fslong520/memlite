//! 嵌入客户端：OpenAI 兼容 /embeddings，配置三来源合并。
//! config.json 的 embedding 段 + MEMLITE_API_BASE/KEY/MODEL 环境变量覆盖。


pub struct EmbedConfig {
    pub api_base: String,
    pub api_key: String,
    pub model: String,
    pub timeout: u64,
}

/// 合并配置（env > 项目级 > 用户级，见 config.rs）；三件齐全才算配置了嵌入
pub fn embedding_config() -> Option<EmbedConfig> {
    let merged = crate::config::embedding_merged();
    let get = |k: &str| merged[k].as_str().unwrap_or("").to_string();
    let (base, key, model) = (get("api_base"), get("api_key"), get("model"));
    let timeout = merged["timeout"].as_u64().unwrap_or(60);
    if base.is_empty() || key.is_empty() || model.is_empty() {
        None
    } else {
        Some(EmbedConfig { api_base: base, api_key: key, model, timeout })
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
