#!/usr/bin/env python3
"""Local cross-encoder rerank service for legal-rag (bge-reranker-v2-m3).

The default decision layer: the Rust server POSTs retrieved candidates
here and gets relevance scores back — deterministic, offline, and zero
billable LLM tokens per query (benchmark: ~/workspace/rerank-bench).

    POST /rerank  {"query": "...", "documents": ["...", ...]}
      → {"scores": [0.91, ...]}        (same order as `documents`)
    GET  /health  → {"ok": true, "model": "..."}

Run:
    RERANK_MODEL=BAAI/bge-reranker-v2-m3 RERANK_PORT=8081 \
        python3 tools/rerank/bge_reranker_sidecar.py

Requires sentence-transformers (pip install sentence-transformers).
Point the server at it with:
    LEGAL_RAG_RERANKER=local
    LEGAL_RAG_RERANK_URL=http://127.0.0.1:8081/rerank
The model loads once at startup; GPU is used automatically when present.
"""
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODEL_NAME = os.environ.get("RERANK_MODEL", "BAAI/bge-reranker-v2-m3")
HOST = os.environ.get("RERANK_HOST", "127.0.0.1")
PORT = int(os.environ.get("RERANK_PORT", "8081"))

print(f"loading rerank model {MODEL_NAME} ...", flush=True)
from sentence_transformers import CrossEncoder  # noqa: E402

MODEL = CrossEncoder(MODEL_NAME)
print(f"rerank service ready on {HOST}:{PORT}", flush=True)


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):  # keep logs quiet
        pass

    def _send_json(self, code: int, payload: dict) -> None:
        body = json.dumps(payload).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/health":
            self._send_json(200, {"ok": True, "model": MODEL_NAME})
        else:
            self._send_json(404, {"error": "not found"})

    def do_POST(self):
        if self.path != "/rerank":
            self._send_json(404, {"error": "not found"})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            req = json.loads(self.rfile.read(length) or b"{}")
            query = req["query"]
            documents = req["documents"]
            pairs = [(query, str(d)) for d in documents]
            scores = MODEL.predict(pairs, show_progress_bar=False)
            self._send_json(200, {"scores": [float(s) for s in scores]})
        except Exception as e:  # noqa: BLE001 - report, never hang the caller
            self._send_json(500, {"error": f"{type(e).__name__}: {e}"})


if __name__ == "__main__":
    ThreadingHTTPServer((HOST, PORT), Handler).serve_forever()
