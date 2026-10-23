#!/usr/bin/env python3
"""Local embeddings service for legal-rag (BAAI/bge-m3, 1024-dim).

OpenAI-compatible embeddings endpoint backed by a local sentence-
transformers model — deterministic, offline, zero Cloudflare neurons.
Exists so the Gemini test instance (:3001) can run fully independent of
the Cloudflare free-tier daily quota (embeddings were its last CF
dependency; the 429 wall killed Gemini Ask before Gemini ever saw the
question).

    POST /v1/embeddings {"model": "...", "input": "..." | ["...", ...]}
      → {"data": [{"embedding": [...], "index": 0}, ...], "usage": {...}}
    GET  /health → {"ok": true, "model": "..."}

Run:
    EMBED_PORT=8083 ~/workspace/ocr-bench/bench-env/bin/python \
        tools/embed/bge_m3_embeddings_sidecar.py

Point a server at it with:
    LEGAL_RAG_EMBEDDING_URL=http://127.0.0.1:8083/v1/embeddings
    LEGAL_RAG_EMBEDDING_MODEL=BAAI/bge-m3
    LEGAL_RAG_EMBEDDING_DIMS=1024
"""
import json
import os
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from sentence_transformers import SentenceTransformer

MODEL_NAME = os.environ.get("EMBED_MODEL", "BAAI/bge-m3")
HOST = os.environ.get("EMBED_HOST", "127.0.0.1")
PORT = int(os.environ.get("EMBED_PORT", "8083"))

print(f"loading embedding model {MODEL_NAME} ...", flush=True)
model = SentenceTransformer(MODEL_NAME, model_kwargs={"torch_dtype": "float16"})
DIM = model.get_sentence_embedding_dimension()
print(f"model ready: dims={DIM} on {HOST}:{PORT}", flush=True)
_lock = threading.Lock()


def embed(texts):
    with _lock:
        vecs = model.encode(
            texts,
            normalize_embeddings=True,
            batch_size=16,
            show_progress_bar=False,
        )
    return [[float(x) for x in v] for v in vecs]


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):  # quiet
        pass

    def _json(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path.startswith("/health"):
            self._json(200, {"ok": True, "model": MODEL_NAME, "dims": DIM})
        else:
            self._json(404, {"error": "not found"})

    def do_POST(self):
        if not self.path.startswith("/v1/embeddings"):
            self._json(404, {"error": "not found"})
            return
        try:
            n = int(self.headers.get("Content-Length", "0"))
            req = json.loads(self.rfile.read(n) or b"{}")
            inp = req.get("input", [])
            texts = [inp] if isinstance(inp, str) else [str(t) for t in inp]
            vecs = embed(texts)
            est = sum(max(1, len(t) // 4) for t in texts)
            self._json(200, {
                "object": "list",
                "data": [
                    {"object": "embedding", "index": i, "embedding": v}
                    for i, v in enumerate(vecs)
                ],
                "model": MODEL_NAME,
                "usage": {"prompt_tokens": est, "total_tokens": est},
            })
        except Exception as e:  # surface, never hang the caller
            self._json(500, {"error": {"message": f"embed failed: {e}"}})


if __name__ == "__main__":
    ThreadingHTTPServer((HOST, PORT), Handler).serve_forever()
