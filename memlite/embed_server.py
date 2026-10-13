"""memlite 本地嵌入 API server：OpenAI 兼容 /embeddings，包本机 bge-m3 ONNX。

设计：
- 模型：BAAI/bge-m3 ONNX 量化版（~17MB），复用 respire 随机安装的模型文件，
  路径三级回退：MEMLITE_MODEL_DIR → ~/.rsrs/models/bge-m3 → /usr/lib/respire/models/bge-m3。
- 池化：bge 系取 CLS（last_hidden_state[:, 0]），L2 归一化。
- 契约：POST /v1/embeddings，body {"model": "...", "input": "文本"或["多条"]}，
  响应 {"data": [{"embedding": [...], "index": 0}], "model": "...", "usage": {...}}。
  memlite/embed.py 的客户端零改动直连。
- 零第三方框架：http.server 单线程足够（记忆检索 QPS 极低）。
- 启动：.venv/bin/python -m memlite.embed_server [--port 18321] [--host 127.0.0.1]
"""

from __future__ import annotations

import argparse
import json
import os
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODEL_NAME = "bge-m3-local"
MAX_LEN = 512


def _find_model_dir() -> str:
    candidates = [
        os.environ.get("MEMLITE_MODEL_DIR", ""),
        os.path.expanduser("~/.rsrs/models/bge-m3"),
        "/usr/lib/respire/models/bge-m3",
    ]
    for c in candidates:
        if c and os.path.isfile(os.path.join(c, "onnx", "model_quantized.onnx")):
            return c
    raise FileNotFoundError(
        "找不到 bge-m3 ONNX 模型；设 MEMLITE_MODEL_DIR 或安装 respire 模型包"
    )


class Embedder:
    def __init__(self, model_dir: str | None = None):
        import numpy as np
        import onnxruntime as ort
        from tokenizers import Tokenizer

        self.dir = model_dir or _find_model_dir()
        self.tokenizer = Tokenizer.from_file(os.path.join(self.dir, "tokenizer.json"))
        self.tokenizer.enable_truncation(max_length=MAX_LEN)
        self.tokenizer.enable_padding()
        so = ort.SessionOptions()
        so.intra_op_num_threads = max(1, (os.cpu_count() or 2) // 2)
        self.sess = ort.InferenceSession(
            os.path.join(self.dir, "onnx", "model_quantized.onnx"),
            sess_options=so,
            providers=["CPUExecutionProvider"],
        )
        self.np = np
        self.input_names = {i.name for i in self.sess.get_inputs()}
        self.dim = None  # 首次推理时探测

    def encode(self, texts: list[str]) -> list[list[float]]:
        np = self.np
        enc = self.tokenizer.encode_batch(texts)
        max_len = max(len(e.ids) for e in enc)
        input_ids = np.array([e.ids + [0] * (max_len - len(e.ids)) for e in enc], dtype=np.int64)
        attention = np.array(
            [e.attention_mask + [0] * (max_len - len(e.attention_mask)) for e in enc],
            dtype=np.int64,
        )
        token_types = np.zeros_like(input_ids)
        feed = {"input_ids": input_ids, "attention_mask": attention}
        if "token_type_ids" in self.input_names:
            feed["token_type_ids"] = token_types
        out = self.sess.run(None, feed)[0]  # (batch, seq, hidden)
        cls = out[:, 0, :]  # bge 系 CLS 池化
        norms = np.linalg.norm(cls, axis=1, keepdims=True)
        cls = cls / np.clip(norms, 1e-12, None)
        self.dim = cls.shape[1]
        return cls.tolist()


_EMBEDDER: Embedder | None = None


def get_embedder() -> Embedder:
    global _EMBEDDER
    if _EMBEDDER is None:
        _EMBEDDER = Embedder()
    return _EMBEDDER


class Handler(BaseHTTPRequestHandler):
    server_version = "memlite-embed/0.1"

    def _json(self, code: int, obj: dict) -> None:
        body = json.dumps(obj, ensure_ascii=False).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):  # 静默 access log，只留错误
        pass

    def do_GET(self):
        if self.path in ("/health", "/v1/health"):
            self._json(200, {"status": "ok", "model": MODEL_NAME})
        else:
            self._json(404, {"error": {"message": f"未知路径 {self.path}"}})

    def do_POST(self):
        if self.path not in ("/v1/embeddings", "/embeddings"):
            self._json(404, {"error": {"message": f"未知路径 {self.path}"}})
            return
        try:
            n = int(self.headers.get("Content-Length", "0"))
            payload = json.loads(self.rfile.read(n).decode("utf-8"))
            inp = payload.get("input", "")
            texts = [inp] if isinstance(inp, str) else list(inp)
            if not texts or not all(isinstance(t, str) and t for t in texts):
                raise ValueError("input 须为非空字符串或非空字符串数组")
            t0 = time.time()
            vectors = get_embedder().encode(texts)
            data = [{"object": "embedding", "index": i, "embedding": v}
                    for i, v in enumerate(vectors)]
            prompt_tokens = sum(len(t) for t in texts)
            self._json(200, {
                "object": "list",
                "model": payload.get("model") or MODEL_NAME,
                "data": data,
                "usage": {
                    "prompt_tokens": prompt_tokens,
                    "total_tokens": prompt_tokens,
                    "latency_ms": round((time.time() - t0) * 1000, 1),
                },
            })
        except json.JSONDecodeError:
            self._json(400, {"error": {"message": "body 不是合法 JSON"}})
        except ValueError as e:
            self._json(400, {"error": {"message": str(e)}})
        except Exception as e:  # noqa: BLE001 — server 边界，如实回 500
            self._json(500, {"error": {"message": f"{type(e).__name__}: {e}"}})


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="memlite-embed-server")
    p.add_argument("--host", default="127.0.0.1")
    p.add_argument("--port", type=int, default=18321)
    p.add_argument("--model-dir", default=None)
    args = p.parse_args(argv)

    if args.model_dir:
        global _EMBEDDER
        _EMBEDDER = Embedder(args.model_dir)
    else:
        get_embedder()  # 启动即加载，坏路径立刻炸不带病服务
    e = get_embedder()
    print(f"memlite embed server 就绪: http://{args.host}:{args.port}/v1/embeddings")
    print(f"模型: {e.dir}（dim 探测于首次请求，ONNX 量化版）")
    srv = ThreadingHTTPServer((args.host, args.port), Handler)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
