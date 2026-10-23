#!/bin/bash
# Isolated TEST instance on :3001 — copied data dir, Gemini bridge as LLM,
# LOCAL bge-m3 embeddings sidecar (:8083) — zero Cloudflare dependency
# (the CF free-tier 429 wall killed Gemini Ask at the embedding step).
# Production (:3000) is untouched.
cd "$(dirname "$0")"
export LEGAL_RAG_ADDR=127.0.0.1:3001
export LEGAL_RAG_DATA_DIR="$PWD/data-test"
export LEGAL_RAG_LLM_URL=http://127.0.0.1:8082/v1/chat/completions
export LEGAL_RAG_LLM_API_KEY=$(python3 -c "import json;print(json.load(open('/home/hatch/workspace/tools/gemini-web2api/config.json'))['api_keys'][0])")
export LEGAL_RAG_LLM_MODEL=gemini-3.6-flash
export LEGAL_RAG_EMBEDDING_URL=http://127.0.0.1:8083/v1/embeddings
export LEGAL_RAG_EMBEDDING_MODEL=BAAI/bge-m3
export LEGAL_RAG_EMBEDDING_DIMS=1024
exec ./target/release/legal-rag-server >> server-test.log 2>&1
