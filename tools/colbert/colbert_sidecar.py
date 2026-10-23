#!/usr/bin/env python3
"""Local ColBERT late-interaction rescoring service for legal-rag.

ColBERT-style MaxSim rescoring: instead of one vector per text, every
token gets an embedding, and a document scores

    score(q, d) = Σ_i max_j cos(q_i, d_j)     over query tokens i

Token-level matching is the point for legal text: a query token like
"86" or "res judicata" can match the exact token in the right Act's
chunk instead of being averaged away in a single-vector embedding.
Benchmark (hard same-section/wrong-Act distractors):
~/workspace/colbert-bench.

    POST /late-interaction
        {"query": "...", "documents": [{"id": "c1", "text": "..."}]}
      → {"scores": [{"id": "c1", "score": 21.37}, ...]}
        (same order as `documents`; score is a MaxSim sum, unbounded,
        comparable only within one request)
    GET  /health  → {"ok": true, "model": "..."}

Model: colbert-ir/colbertv2.0 — MIT license (checked on the HF model
card, Oct 2026). Jina's ColBERT models are CC BY-NC and must not be
used here. Encoding follows the reference ColBERT inference recipe:
[unused0] query marker + [MASK]-padding (query augmentation),
[unused1] doc marker, punctuation tokens filtered from documents,
128-dim linear projection, L2-normalized token embeddings.

Run:
    COLBERT_MODEL=colbert-ir/colbertv2.0 COLBERT_PORT=8084 \
        ~/workspace/ocr-bench/bench-env/bin/python \
        tools/colbert/colbert_sidecar.py

Python env: reuse ~/workspace/ocr-bench/bench-env (torch +
transformers). Not yet wired into the Rust server — the contract
above is the integration point (a future HttpLateInteraction
rescorer alongside HttpReranker, LEGAL_RAG_COLBERT_URL).
The model loads once at startup; GPU is used automatically when
present.
"""
import json
import os
import string
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODEL_NAME = os.environ.get("COLBERT_MODEL", "colbert-ir/colbertv2.0")
HOST = os.environ.get("COLBERT_HOST", "127.0.0.1")
PORT = int(os.environ.get("COLBERT_PORT", "8084"))
QUERY_MAXLEN = int(os.environ.get("COLBERT_QUERY_MAXLEN", "32"))
DOC_MAXLEN = int(os.environ.get("COLBERT_DOC_MAXLEN", "300"))

print(f"loading ColBERT model {MODEL_NAME} ...", flush=True)
import torch  # noqa: E402
from huggingface_hub import hf_hub_download  # noqa: E402
from safetensors.torch import load_file  # noqa: E402
from transformers import AutoModel, AutoTokenizer  # noqa: E402

DEVICE = "cuda" if torch.cuda.is_available() else "cpu"
TOKENIZER = AutoTokenizer.from_pretrained(MODEL_NAME)
BERT = AutoModel.from_pretrained(MODEL_NAME).to(DEVICE).eval()

# The checkpoint carries ColBERT's token projection as a top-level
# `linear` layer that BertModel does not load; pick it up explicitly.
_ckpt = load_file(hf_hub_download(MODEL_NAME, "model.safetensors"))
_in = BERT.config.hidden_size
_out = _ckpt["linear.weight"].shape[0]
LINEAR = torch.nn.Linear(_in, _out, bias="linear.bias" in _ckpt).to(DEVICE)
with torch.no_grad():
    LINEAR.weight.copy_(_ckpt["linear.weight"].to(DEVICE))
    if "linear.bias" in _ckpt:
        LINEAR.bias.copy_(_ckpt["linear.bias"].to(DEVICE))
LINEAR.eval()
del _ckpt

QUERY_MARKER = "[unused0]"  # ColBERT's [Q] token
DOC_MARKER = "[unused1]"  # ColBERT's [D] token
# Document tokens that are pure punctuation carry no matching signal.
PUNCT_IDS = {
    tid
    for tok, tid in TOKENIZER.get_vocab().items()
    if tok and all(ch in string.punctuation for ch in tok)
}
_lock = threading.Lock()
print(
    f"colbert service ready on {HOST}:{PORT} "
    f"(device={DEVICE}, dim={_out}, qmax={QUERY_MAXLEN}, dmax={DOC_MAXLEN})",
    flush=True,
)


def _project(input_ids, attention_mask):
    with torch.no_grad():
        out = BERT(input_ids=input_ids, attention_mask=attention_mask)[0]
        return torch.nn.functional.normalize(LINEAR(out), p=2, dim=2)


def encode_query(text: str) -> torch.Tensor:
    """(n_tokens, dim) normalized token embeddings, [MASK]-augmented."""
    enc = TOKENIZER(
        QUERY_MARKER + " " + text,
        add_special_tokens=True,
        truncation=True,
        max_length=QUERY_MAXLEN,
        return_tensors="pt",
    )
    ids = enc["input_ids"].to(DEVICE)
    attn = enc["attention_mask"].to(DEVICE)
    pad = QUERY_MAXLEN - ids.shape[1]
    if pad > 0:  # query augmentation: [MASK] tokens are real queries
        mask_id = TOKENIZER.mask_token_id
        ids = torch.cat([ids, torch.full((1, pad), mask_id, device=DEVICE)], dim=1)
        attn = torch.cat([attn, torch.ones((1, pad), dtype=attn.dtype, device=DEVICE)], dim=1)
    return _project(ids, attn)[0]


def encode_docs(texts: list) -> list:
    """One (n_tokens, dim) normalized embedding matrix per document,
    punctuation tokens removed."""
    enc = TOKENIZER(
        [DOC_MARKER + " " + t for t in texts],
        add_special_tokens=True,
        truncation=True,
        max_length=DOC_MAXLEN,
        padding=True,
        return_tensors="pt",
    )
    ids = enc["input_ids"].to(DEVICE)
    attn = enc["attention_mask"].to(DEVICE)
    emb = _project(ids, attn)
    docs = []
    for i in range(len(texts)):
        keep = [
            j
            for j in range(ids.shape[1])
            if attn[i, j] == 1 and int(ids[i, j]) not in PUNCT_IDS
        ]
        docs.append(emb[i, keep, :])
    return docs


def maxsim(q_emb: torch.Tensor, d_emb: torch.Tensor) -> float:
    """ColBERT late interaction: Σ_i max_j cos(q_i, d_j)."""
    if d_emb.shape[0] == 0:
        return 0.0
    sims = q_emb @ d_emb.T
    return float(sims.max(dim=1).values.sum().item())


def score_documents(query: str, documents: list) -> list:
    """documents: [{"id": ..., "text": ...}] → [float] in input order."""
    with _lock:
        q = encode_query(query)
        d_embs = encode_docs([str(d["text"]) for d in documents])
    return [maxsim(q, d) for d in d_embs]


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
        if self.path != "/late-interaction":
            self._send_json(404, {"error": "not found"})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            req = json.loads(self.rfile.read(length) or b"{}")
            query = req["query"]
            documents = req["documents"]
            scores = score_documents(query, documents)
            self._send_json(
                200,
                {
                    "scores": [
                        {"id": d["id"], "score": s}
                        for d, s in zip(documents, scores)
                    ]
                },
            )
        except Exception as e:  # noqa: BLE001 - report, never hang the caller
            self._send_json(500, {"error": f"{type(e).__name__}: {e}"})


if __name__ == "__main__":
    ThreadingHTTPServer((HOST, PORT), Handler).serve_forever()
