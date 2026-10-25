#!/usr/bin/env python3
"""Local cross-encoder rerank service for legal-rag (bge-reranker-v2-m3).

The default decision layer: the Rust server POSTs retrieved candidates
here and gets relevance scores back — deterministic, offline, and zero
billable LLM tokens per query (benchmark: ~/workspace/rerank-bench).

    POST /rerank  {"query": "...", "documents": ["...", ...]}
      → {"scores": [0.91, ...]}        (same order as `documents`)
    GET  /health  → {"ok": true, "model": "...", "token_cache": {...}}
    GET  /stats   → {"model": "...", "token_cache": {...}}

Run:
    RERANK_MODEL=BAAI/bge-reranker-v2-m3 RERANK_PORT=8081 \
        python3 tools/rerank/bge_reranker_sidecar.py

Requires sentence-transformers (pip install sentence-transformers).
Point the server at it with:
    LEGAL_RAG_RERANKER=local
    LEGAL_RAG_RERANK_URL=http://127.0.0.1:8081/rerank
The model loads once at startup; GPU is used automatically when present.

Token cache (txtai-style)
--------------------------
`CrossEncoder.predict` tokenizes every (query, document) pair on every
call, even when the same candidates recur across queries (the retrieval
pool re-scores overlapping documents constantly). This sidecar caches
the tokenized pair server-side so repeat pairs skip tokenization:

* Key: SHA-256 over ``query + "\\x00" + document`` (the full pair
  text). The tokenizer is deterministic for identical input text, and
  truncation is applied by the model's own ``preprocess`` at tokenize
  time, so the stored ids are exactly what a fresh call would produce
  — including for over-length documents, which are truncated before
  they are stored.
* Value: the pair's unpadded token tensors (``input_ids``,
  ``attention_mask``, plus any other tensor keys ``preprocess``
  returns), stored as plain lists. At request time the cached pairs
  are re-padded to the batch's max length (``input_ids`` with the
  tokenizer pad id, everything else with 0) and run through the
  underlying model forward + the model's activation function — the
  same computation ``predict`` performs, minus the tokenization.
  Score equivalence vs ``predict`` is verified by
  ``tools/rerank/test_token_cache.py`` (max abs diff ~1e-10 on the
  probe payload, tolerance 1e-4).
* Knob: ``RERANK_TOKEN_CACHE=0`` disables the cache entirely; the
  sidecar then falls back to the original ``CrossEncoder.predict``
  path. Any other value (including unset) enables it.
* Bound: ``RERANK_TOKEN_CACHE_MAX_ENTRIES`` (default 10000) caps the
  cache; the oldest entry is evicted first (FIFO). The cache is
  in-memory only and per-process — a sidecar restart clears it.
* Observability: ``GET /health`` (and ``GET /stats``) report
  ``token_cache`` with ``enabled``, ``hits``, ``misses``, ``entries``,
  and ``max_entries``. Hits/misses count pairs per request, so a
  repeat ``/rerank`` of N documents reports N hits.

The ``/rerank`` request/response contract is unchanged by the cache:
same JSON in, same ``{"scores": [...]}`` out, same document order.
"""
import hashlib
import json
import os
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODEL_NAME = os.environ.get("RERANK_MODEL", "BAAI/bge-reranker-v2-m3")
HOST = os.environ.get("RERANK_HOST", "127.0.0.1")
PORT = int(os.environ.get("RERANK_PORT", "8081"))

CACHE_ENABLED = os.environ.get("RERANK_TOKEN_CACHE", "1").strip().lower() not in (
    "0",
    "false",
    "no",
    "off",
)
try:
    CACHE_MAX_ENTRIES = max(1, int(os.environ.get("RERANK_TOKEN_CACHE_MAX_ENTRIES", "10000")))
except ValueError:
    CACHE_MAX_ENTRIES = 10000

print(f"loading rerank model {MODEL_NAME} ...", flush=True)
from sentence_transformers import CrossEncoder  # noqa: E402

import torch  # noqa: E402

MODEL = CrossEncoder(MODEL_NAME)
MODEL.eval()
print(
    f"rerank service ready on {HOST}:{PORT} "
    f"(token_cache={'on' if CACHE_ENABLED else 'off'})",
    flush=True,
)

# --- token cache -------------------------------------------------------
# key (sha256 hex) -> {tensor-key: [ints]} plus any scalar keys copied
# verbatim (e.g. preprocess's "modality": "text"). Insertion-ordered dict
# doubles as the FIFO eviction queue.
_cache: dict = {}
_cache_lock = threading.Lock()
_cache_hits = 0
_cache_misses = 0


def _cache_key(query: str, document: str) -> str:
    # SHA-256 over the full pair text, NUL-separated so that no
    # (query, document) split can collide with another split of the
    # same concatenation.
    return hashlib.sha256(f"{query}\x00{document}".encode("utf-8")).hexdigest()


def _tokenize_pair(query: str, document: str) -> dict:
    """Tokenize one pair via the model's own preprocess, unpadded.

    A single-pair preprocess call pads only to that pair's own length,
    so slicing by the attention-mask sum recovers the exact unpadded
    sequence — no tokenizer arguments are re-implemented here.
    """
    feats = MODEL.preprocess([(query, document)])
    out = {}
    for key, value in feats.items():
        if isinstance(value, torch.Tensor):
            if key == "attention_mask":
                length = int(value[0].sum())
            else:
                # All per-token tensors share the padded length; the
                # attention mask is the authoritative unpadded length.
                length = int(feats["attention_mask"][0].sum())
            out[key] = value[0][:length].tolist()
        else:
            out[key] = value
    return out


def _pad_id() -> int:
    try:
        pad = MODEL.tokenizer.pad_token_id
    except Exception:  # noqa: BLE001 - fall back to XLM-R's pad id
        pad = None
    return pad if pad is not None else 1


def _score_cached_pairs(pairs_with_tokens):
    """Forward cached/uncached token lists through the model.

    Mirrors CrossEncoder.predict's batching: length-sorted batches of
    32, padded to the batch max, model forward, activation function,
    single-label squeeze — then restored to request order.
    """
    order = sorted(range(len(pairs_with_tokens)), key=lambda i: -len(pairs_with_tokens[i].get("input_ids", [])))
    sorted_pairs = [pairs_with_tokens[i] for i in order]
    pad = _pad_id()
    scores_sorted = []
    with torch.inference_mode():
        for start in range(0, len(sorted_pairs), 32):
            batch = sorted_pairs[start : start + 32]
            max_len = max(len(p.get("input_ids", [])) for p in batch)
            features = {}
            tensor_keys = set()
            for p in batch:
                tensor_keys.update(k for k, v in p.items() if isinstance(v, list))
            for key in tensor_keys:
                fill = pad if key == "input_ids" else 0
                features[key] = torch.tensor(
                    [p.get(key, [fill] * len(p.get("input_ids", []))) + [fill] * (max_len - len(p.get("input_ids", []))) for p in batch],
                    dtype=torch.long,
                )
            for p in batch:  # scalar (non-tensor) keys, e.g. modality
                for key, value in p.items():
                    if not isinstance(value, list):
                        features[key] = value
            out = MODEL(features)
            scores = out["scores"]
            if torch.is_floating_point(scores) and torch.finfo(scores.dtype).bits < 32:
                scores = scores.float()
            if MODEL.activation_fn is not None:
                scores = MODEL.activation_fn(scores)
            if MODEL.num_labels == 1 and scores.ndim > 1:
                scores = scores.squeeze(-1)
            scores_sorted.extend(float(s) for s in scores)
    scores = [0.0] * len(pairs_with_tokens)
    for sorted_pos, orig_idx in enumerate(order):
        scores[orig_idx] = scores_sorted[sorted_pos]
    return scores


def rerank(query: str, documents: list) -> list:
    """Score documents for a query, using the token cache when enabled."""
    if not documents:
        return []
    if not CACHE_ENABLED:
        scores = MODEL.predict([(query, str(d)) for d in documents], show_progress_bar=False)
        return [float(s) for s in scores]

    global _cache_hits, _cache_misses
    keys = [_cache_key(query, str(d)) for d in documents]
    with _cache_lock:
        tokens = [_cache.get(k) for k in keys]
    misses = [i for i, t in enumerate(tokens) if t is None]
    for i in misses:
        tokens[i] = _tokenize_pair(query, str(documents[i]))
    with _cache_lock:
        _cache_hits += len(documents) - len(misses)
        _cache_misses += len(misses)
        for i in misses:
            _cache[keys[i]] = tokens[i]
            while len(_cache) > CACHE_MAX_ENTRIES:
                _cache.pop(next(iter(_cache)))
    return _score_cached_pairs(tokens)


def cache_stats() -> dict:
    with _cache_lock:
        return {
            "enabled": CACHE_ENABLED,
            "hits": _cache_hits,
            "misses": _cache_misses,
            "entries": len(_cache),
            "max_entries": CACHE_MAX_ENTRIES,
        }


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
            self._send_json(200, {"ok": True, "model": MODEL_NAME, "token_cache": cache_stats()})
        elif self.path == "/stats":
            self._send_json(200, {"model": MODEL_NAME, "token_cache": cache_stats()})
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
            self._send_json(200, {"scores": rerank(query, documents)})
        except Exception as e:  # noqa: BLE001 - report, never hang the caller
            self._send_json(500, {"error": f"{type(e).__name__}: {e}"})


if __name__ == "__main__":
    ThreadingHTTPServer((HOST, PORT), Handler).serve_forever()
