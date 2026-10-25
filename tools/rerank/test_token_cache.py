#!/usr/bin/env python3
"""Verify the reranker sidecar token cache (tools/rerank/bge_reranker_sidecar.py).

Does NOT depend on the Rust workspace — pure Python + the sidecar's HTTP API.

Checks:
  (1) A repeat /rerank with an identical payload reports cache hits > 0
      on the second call (stats from GET /health).
  (2) The sidecar's scores equal a fresh CrossEncoder.predict computation
      on the same payload within 1e-4 (default; --no-equivalence skips the
      local model load for a fast cache-only check).
  (3) The /rerank request/response contract is unchanged: {"scores": [...]}
      in document order, one float per document.

Usage (sidecar must already be running on :8081):
    python3 tools/rerank/test_token_cache.py [--base http://127.0.0.1:8081]

Verify the disable knob by hand (requires a sidecar restart):
    RERANK_TOKEN_CACHE=0 ... python3 tools/rerank/bge_reranker_sidecar.py
    # then: curl /health -> token_cache.enabled == false, and repeat
    # /rerank calls report hits == 0 (the sidecar falls back to
    # CrossEncoder.predict with no caching).
"""
import argparse
import json
import os
import sys
import urllib.request

PAYLOAD = {
    "query": "Section 86 IT Act intermediary liability",
    "documents": [
        "Section 79 of the Information Technology Act provides safe harbour to intermediaries.",
        "Section 302 IPC deals with punishment for murder.",
        "The Evidence Act section 101 places burden of proof on who asserts a fact.",
    ],
}
TOLERANCE = 1e-4


def _call(base, method, path, payload=None):
    data = json.dumps(payload).encode() if payload is not None else None
    req = urllib.request.Request(
        base + path, data=data, method=method,
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=180) as resp:
        return json.loads(resp.read())


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default="http://127.0.0.1:8081")
    ap.add_argument("--no-equivalence", action="store_true",
                    help="skip the local CrossEncoder.predict comparison")
    args = ap.parse_args()

    health = _call(args.base, "GET", "/health")
    print("health:", health)
    cache = health.get("token_cache")
    assert cache is not None, "sidecar /health has no token_cache stats — is the new code running?"
    assert cache["enabled"], "token_cache.enabled is false — rerun without RERANK_TOKEN_CACHE=0"

    before = cache
    first = _call(args.base, "POST", "/rerank", PAYLOAD)
    mid = _call(args.base, "GET", "/health")["token_cache"]
    second = _call(args.base, "POST", "/rerank", PAYLOAD)
    after = _call(args.base, "GET", "/health")["token_cache"]
    print("first scores: ", first["scores"])
    print("second scores:", second["scores"])
    print("stats before:", before)
    print("stats mid:   ", mid)
    print("stats after: ", after)

    assert len(first["scores"]) == len(PAYLOAD["documents"]), "score count != document count"
    assert len(second["scores"]) == len(PAYLOAD["documents"])
    assert all(isinstance(s, float) for s in first["scores"] + second["scores"])
    assert first["scores"] == second["scores"], "repeat call scores differ (cache must not change scores)"
    assert mid["misses"] > before["misses"], "first call should record misses"
    assert after["hits"] > mid["hits"], "second (identical) call should record hits"
    assert after["hits"] - mid["hits"] >= len(PAYLOAD["documents"]), (
        f"expected >= {len(PAYLOAD['documents'])} hits on repeat, got {after['hits'] - mid['hits']}"
    )
    print(f"PASS cache: repeat call recorded {after['hits'] - mid['hits']} hits, "
          f"{mid['misses'] - before['misses']} misses on first call, {after['entries']} entries")

    if not args.no_equivalence:
        os.environ.setdefault("HF_HUB_OFFLINE", "1")
        from sentence_transformers import CrossEncoder
        model = CrossEncoder(health["model"])
        ref = model.predict(
            [(PAYLOAD["query"], d) for d in PAYLOAD["documents"]],
            show_progress_bar=False,
        )
        diffs = [abs(float(a) - float(b)) for a, b in zip(first["scores"], ref)]
        print("reference predict:", [float(x) for x in ref])
        print("max abs diff vs predict:", max(diffs))
        assert max(diffs) <= TOLERANCE, f"score diff {max(diffs)} exceeds {TOLERANCE}"
        print(f"PASS equivalence: max abs diff {max(diffs):.3e} <= {TOLERANCE}")

    print("ALL CHECKS PASSED")
    return 0


if __name__ == "__main__":
    sys.exit(main())
