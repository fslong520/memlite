//! memlite 本地嵌入 API 服务：OpenAI 兼容 /embeddings，包本机 bge-m3 ONNX 量化版。
//!
//! Python 版（原 memlite/embed_server.py，已删）的 Rust 重写——同契约、同池化、同模型回退：
//! - 模型：--model-dir → MEMLITE_MODEL_DIR → ~/.rsrs/models/bge-m3 → /usr/lib/respire/models/bge-m3
//! - 池化：bge 系 CLS（last_hidden_state[:, 0, :]）+ L2 归一化（下限 1e-12，同 np.clip）
//! - 截断 512（LongestFirst）、批内按最长补 0（BatchLongest、pad_id=0），token_type_ids 恒 0、
//!   按模型输入有无取舍——逐行对齐 python encode() 的行为
//! - GET /health /v1/health → {"status":"ok",...}；POST /v1/embeddings /embeddings；
//!   未知路径 404 {"error":{"message":"未知路径 …"}}；坏 JSON 400；推理失败 500
//! - 零框架：std TcpListener 手写 HTTP 子集（同 memlite web 手法），只绑回环地址
//! - 推理线程：默认 min(cpu/2, 4)，MEMLITE_EMBED_THREADS 覆盖（本机供电敏感，宁慢勿快）
//!
//! 构建：`cargo build --release`（已并入主二进制）
//! 运行：`memlite embed-server [--host 127.0.0.1] [--port 18321] [--model-dir DIR]`

use ort::session::Session;
use ort::value::TensorRef;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokenizers::tokenizer::{
    PaddingParams, PaddingStrategy, Tokenizer, TruncationParams, TruncationDirection,
    TruncationStrategy,
};
use tokenizers::PaddingDirection;

const MODEL_NAME: &str = "bge-m3-local";
const MAX_LEN: usize = 512;
const INPUT_MSG: &str = "input 须为非空字符串或非空字符串数组";

/// 模型目录三级回退（与旧 Python 版一致）。
fn find_model_dir(explicit: Option<String>) -> Result<PathBuf, String> {
    if let Some(d) = explicit {
        return Ok(PathBuf::from(d));
    }
    let home = crate::config::home_dir();
    let candidates = [
        std::env::var("MEMLITE_MODEL_DIR").ok(),
        if home.is_empty() {
            None
        } else {
            Some(format!("{home}/.rsrs/models/bge-m3"))
        },
        Some("/usr/lib/respire/models/bge-m3".to_string()),
    ];
    for c in candidates.into_iter().flatten() {
        if Path::new(&c).join("onnx/model_quantized.onnx").is_file() {
            return Ok(PathBuf::from(c));
        }
    }
    Err("找不到 bge-m3 ONNX 模型；设 MEMLITE_MODEL_DIR 或安装 respire 模型包".to_string())
}

struct Embedder {
    tokenizer: Tokenizer,
    session: Mutex<Session>,
}

impl Embedder {
    fn load(dir: &Path, threads: usize) -> Result<Self, String> {
        let tok_path = dir.join("tokenizer.json");
        let onnx = dir.join("onnx").join("model_quantized.onnx");
        let mut tokenizer =
            Tokenizer::from_file(&tok_path).map_err(|e| format!("加载 tokenizer.json 失败: {e}"))?;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: MAX_LEN,
                strategy: TruncationStrategy::LongestFirst,
                stride: 0,
                direction: TruncationDirection::Right,
            }))
            .map_err(|e| format!("设置截断失败: {e}"))?;
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            direction: PaddingDirection::Right,
            pad_to_multiple_of: None,
            pad_id: 0,
            pad_type_id: 0,
            pad_token: "[PAD]".to_string(),
        }));
        let session = Session::builder()
            .map_err(|e| format!("ort builder: {e}"))?
            .with_intra_threads(threads)
            .map_err(|e| format!("设置推理线程失败: {e}"))?
            .with_inter_threads(1)
            .map_err(|e| format!("设置 inter 线程失败: {e}"))?
            .with_intra_op_spinning(false)
            .map_err(|e| format!("关闭自旋失败: {e}"))?
            .with_execution_providers([ort::ep::CPU::default().build().error_on_failure()])
            .map_err(|e| format!("CPU 执行器失败: {e}"))?
            .commit_from_file(&onnx)
            .map_err(|e| format!("加载 {} 失败: {e}", onnx.display()))?;
        Ok(Self {
            tokenizer,
            session: Mutex::new(session),
        })
    }

    /// 分词 → 推理 → CLS 池化 → L2 归一化（对齐 python encode()）。
    fn encode(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let inputs: Vec<tokenizers::EncodeInput> = texts
            .iter()
            .map(|t| tokenizers::EncodeInput::Single(t.clone().into()))
            .collect();
        let encs = self
            .tokenizer
            .encode_batch(inputs, true)
            .map_err(|e| format!("分词失败: {e}"))?;
        if encs.is_empty() {
            return Err("分词结果为空".to_string());
        }
        let batch = encs.len();
        let seq = encs[0].get_ids().len();
        if seq == 0 {
            return Err("token 序列为空".to_string());
        }
        let mut ids = Vec::with_capacity(batch * seq);
        let mut mask = Vec::with_capacity(batch * seq);
        let mut types = Vec::with_capacity(batch * seq);
        for e in &encs {
            // BatchLongest 已把各行补到同长；仍按行核验，形状不齐宁可报错不塞错位数据
            if e.get_ids().len() != seq {
                return Err("批内序列长度不齐".to_string());
            }
            ids.extend(e.get_ids().iter().map(|&x| x as i64));
            mask.extend(e.get_attention_mask().iter().map(|&x| x as i64));
            types.extend(e.get_type_ids().iter().map(|&x| x as i64));
        }

        let mut session = self.session.lock().map_err(|_| "推理会话锁中毒".to_string())?;
        let has_types = session
            .inputs()
            .iter()
            .any(|i| i.name() == "token_type_ids");
        let ids_t = TensorRef::from_array_view(([batch, seq], ids.as_slice()))
            .map_err(|e| format!("构造 input_ids 失败: {e}"))?;
        let mask_t = TensorRef::from_array_view(([batch, seq], mask.as_slice()))
            .map_err(|e| format!("构造 attention_mask 失败: {e}"))?;
        let output = if has_types {
            let types_t = TensorRef::from_array_view(([batch, seq], types.as_slice()))
                .map_err(|e| format!("构造 token_type_ids 失败: {e}"))?;
            session
                .run(ort::inputs![ids_t, mask_t, types_t])
                .map_err(|e| format!("推理失败: {e}"))?
        } else {
            session
                .run(ort::inputs![ids_t, mask_t])
                .map_err(|e| format!("推理失败: {e}"))?
        };
        let (shape, values) = output[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("读取输出张量失败: {e}"))?;
        if shape.len() != 3 || shape[0] as usize != batch || shape[1] as usize != seq {
            return Err(format!("非法输出形状: {shape:?}"));
        }
        let hidden = shape[2] as usize;
        if values.len() != batch * seq * hidden {
            return Err(format!(
                "输出元素数不符: {} != {batch}*{seq}*{hidden}",
                values.len()
            ));
        }

        // CLS（每行首 token）+ L2 归一化，clip 下限 1e-12 同 np.clip(norms, 1e-12, None)
        let mut out = Vec::with_capacity(batch);
        for r in 0..batch {
            let base = r * seq * hidden;
            let cls = &values[base..base + hidden];
            let norm = cls.iter().map(|v| v * v).sum::<f32>().sqrt();
            let denom = if norm.is_finite() && norm > 1e-12 { norm } else { 1e-12 };
            out.push(cls.iter().map(|v| v / denom).collect::<Vec<f32>>());
        }
        Ok(out)
    }
}

enum Fail {
    BadRequest(String),
    Server(String),
}

fn build_embeddings(body: &[u8], emb: &Embedder) -> Result<Value, Fail> {
    let payload: Value =
        serde_json::from_slice(body).map_err(|_| Fail::BadRequest("body 不是合法 JSON".into()))?;
    let texts: Vec<String> = match payload.get("input").unwrap_or(&Value::String(String::new())) {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => {
            let mut v = Vec::with_capacity(a.len());
            for item in a {
                match item {
                    Value::String(s) => v.push(s.clone()),
                    _ => return Err(Fail::BadRequest(INPUT_MSG.into())),
                }
            }
            v
        }
        _ => return Err(Fail::BadRequest(INPUT_MSG.into())),
    };
    if texts.is_empty() || texts.iter().any(|t| t.is_empty()) {
        return Err(Fail::BadRequest(INPUT_MSG.into()));
    }

    let t0 = Instant::now();
    let vectors = emb.encode(&texts).map_err(Fail::Server)?;
    let prompt_tokens: usize = texts.iter().map(|t| t.chars().count()).sum();
    let latency_ms = ((t0.elapsed().as_secs_f64() * 1000.0) * 10.0).round() / 10.0;
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(MODEL_NAME);
    let data: Vec<Value> = vectors
        .into_iter()
        .enumerate()
        .map(|(i, embedding)| {
            json!({"object": "embedding", "index": i, "embedding": embedding})
        })
        .collect();
    Ok(json!({
        "object": "list",
        "model": model,
        "data": data,
        "usage": {
            "prompt_tokens": prompt_tokens,
            "total_tokens": prompt_tokens,
            "latency_ms": latency_ms,
        },
    }))
}

fn err404(path: &str) -> Value {
    json!({"error": {"message": format!("未知路径 {path}")}})
}

fn json_resp(stream: &mut TcpStream, code: u16, reason: &str, body: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    stream.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    stream.write_all(&bytes).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())
}

fn handle(mut stream: TcpStream, emb: &Arc<Mutex<Embedder>>) -> Result<(), String> {
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut req_line = String::new();
    reader.read_line(&mut req_line).map_err(|e| e.to_string())?;
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();

    let mut content_len = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            break;
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some(v) = l.strip_prefix("Content-Length:") {
            content_len = v.trim().parse().unwrap_or(0);
        }
    }

    match method.as_str() {
        "GET" => {
            if path == "/health" || path == "/v1/health" {
                json_resp(&mut stream, 200, "OK", &json!({"status": "ok", "model": MODEL_NAME}))
            } else {
                json_resp(&mut stream, 404, "Not Found", &err404(&path))
            }
        }
        "POST" => {
            if path != "/v1/embeddings" && path != "/embeddings" {
                return json_resp(&mut stream, 404, "Not Found", &err404(&path));
            }
            let mut body = vec![0u8; content_len];
            reader.read_exact(&mut body).map_err(|e| e.to_string())?;
            let guard = emb.lock().map_err(|_| "推理会话锁中毒".to_string())?;
            match build_embeddings(&body, &guard) {
                Ok(v) => json_resp(&mut stream, 200, "OK", &v),
                Err(Fail::BadRequest(m)) => {
                    json_resp(&mut stream, 400, "Bad Request", &json!({"error": {"message": m}}))
                }
                Err(Fail::Server(e)) => json_resp(
                    &mut stream,
                    500,
                    "Internal Server Error",
                    &json!({"error": {"message": format!("RuntimeError: {e}")}}),
                ),
            }
        }
        other => {
            // python BaseHTTPRequestHandler 对未支持方法回 501
            let _ = json_resp(
                &mut stream,
                501,
                "Not Implemented",
                &json!({"error": {"message": format!("Unsupported method ({other})")}}),
            );
            Ok(())
        }
    }
}

/// 常驻服务：加载模型→监听→循环 accept（错误路径 eprintln+exit，同旧版行为）。
pub fn serve(host: String, port: u16, model_dir: Option<String>) {
    let dir = match find_model_dir(model_dir) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let threads = std::env::var("MEMLITE_EMBED_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or_else(|| {
            let cpu = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(2);
            (cpu / 2).clamp(1, 4)
        });
    // 启动即加载，坏路径立刻炸不带病服务（同 python 行为）
    let emb = match Embedder::load(&dir, threads) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    eprintln!("memlite embed server 就绪: http://{host}:{port}/v1/embeddings");
    eprintln!("模型: {}（dim 探测于首次请求，ONNX 量化版，推理线程 {threads}）", dir.display());

    let addr = format!("{host}:{port}");
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("绑定 {addr} 失败: {e}");
            std::process::exit(1);
        }
    };
    let emb = Arc::new(Mutex::new(emb));
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let emb = Arc::clone(&emb);
                std::thread::spawn(move || {
                    if let Err(e) = handle(s, &emb) {
                        eprintln!("连接处理失败: {e}");
                    }
                });
            }
            Err(e) => eprintln!("accept 失败: {e}"),
        }
    }
}
