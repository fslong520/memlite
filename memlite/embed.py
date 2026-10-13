"""嵌入层：配置了 API 就走语义检索，没配置就回退词面 BM25。

配置三来源（后者覆盖前者）：~/.memlite/config.json 的 `embedding` 段、
环境变量 MEMLITE_API_BASE / MEMLITE_API_KEY / MEMLITE_EMBED_MODEL。
endpoint 兼容 OpenAI /embeddings 契约（api_base、api_key、model 三件即可，
硅基流动、DeepSeek、OpenAI 及自建 vLLM/Ollama 均适用）。

安全：api_key 只存本机 config（家目录，不在数据仓内），禁写入记忆正文。
"""

from __future__ import annotations

import json
import os
import urllib.request

DEFAULT_TIMEOUT = 60


def _config_path() -> str:
    return os.environ.get("MEMLITE_CONFIG") or os.path.expanduser("~/.memlite/config.json")


def _config() -> dict:
    p = _config_path()
    if os.path.isfile(p):
        try:
            return json.load(open(p, encoding="utf-8"))
        except json.JSONDecodeError:
            pass
    return {}


def embedding_config() -> dict:
    """合并 config 与环境变量；api_base/api_key/model 齐全才算配置了嵌入。"""
    cfg = dict(_config().get("embedding") or {})
    if os.environ.get("MEMLITE_API_BASE"):
        cfg["api_base"] = os.environ["MEMLITE_API_BASE"]
    if os.environ.get("MEMLITE_API_KEY"):
        cfg["api_key"] = os.environ["MEMLITE_API_KEY"]
    if os.environ.get("MEMLITE_EMBED_MODEL"):
        cfg["model"] = os.environ["MEMLITE_EMBED_MODEL"]
    return cfg


class EmbeddingError(RuntimeError):
    pass


class EmbeddingClient:
    """OpenAI 兼容 /embeddings 客户端，纯标准库。"""

    def __init__(self, api_base: str, api_key: str, model: str,
                 timeout: int = DEFAULT_TIMEOUT):
        self.api_base = api_base.rstrip("/")
        self.api_key = api_key
        self.model = model
        self.timeout = timeout

    def embed(self, text: str) -> list[float]:
        if not text:
            raise EmbeddingError("空文本无法嵌入")
        body = json.dumps({"model": self.model, "input": text}).encode("utf-8")
        req = urllib.request.Request(
            f"{self.api_base}/embeddings",
            data=body,
            headers={
                "Content-Type": "application/json",
                "Authorization": f"Bearer {self.api_key}",
            },
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                data = json.loads(resp.read().decode("utf-8"))
        except urllib.error.HTTPError as e:
            raise EmbeddingError(f"嵌入 API HTTP {e.code}: {e.read()[:200]!r}") from e
        except (urllib.error.URLError, TimeoutError, OSError) as e:
            raise EmbeddingError(f"嵌入 API 不可达: {e}") from e
        try:
            return [float(x) for x in data["data"][0]["embedding"]]
        except (KeyError, IndexError, TypeError) as e:
            raise EmbeddingError(f"响应形状不合: {str(data)[:200]}") from e


def get_client() -> EmbeddingClient | None:
    """配置齐则返回客户端，否则 None（调用方回退 BM25）。"""
    cfg = embedding_config()
    if cfg.get("api_base") and cfg.get("api_key") and cfg.get("model"):
        return EmbeddingClient(
            api_base=cfg["api_base"],
            api_key=cfg["api_key"],
            model=cfg["model"],
            timeout=int(cfg.get("timeout", DEFAULT_TIMEOUT)),
        )
    return None


def embed_text(text: str) -> list[float] | None:
    """有客户端则嵌入，失败返回 None（不抛——调用方按词面兜底）。"""
    client = get_client()
    if client is None:
        return None
    try:
        return client.embed(text)
    except EmbeddingError:
        return None
