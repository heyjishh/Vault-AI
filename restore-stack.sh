#!/bin/bash
# Full stack restore after VM replacement (idempotent-ish: skip if listening)
up() { ss -tln 2>/dev/null | grep -q ":$1 "; }
# 1. SereneDB
if ! up 7890; then
  cp ~/workspace/serenedb-rootfs/etc/serenedb/serened.conf /tmp/serened.conf
  sed -i 's|^--server_directory=.*|--server_directory=/home/hatch/workspace/serenedb-data|' /tmp/serened.conf
  (setsid nohup ~/workspace/serenedb-rootfs/usr/bin/serened --flagfile=/tmp/serened.conf >> ~/workspace/serenedb-data/serened.log 2>&1 < /dev/null &)
fi
# 2. Bridge on :8082 (needs --config or it takes 8081!)
if ! up 8082; then
  (cd ~/workspace/tools/gemini-web2api && setsid nohup ~/workspace/ocr-bench/bench-env/bin/python gemini_web2api.py --config config.json >> bridge.log 2>&1 < /dev/null &)
fi
# 3. Reranker :8081 + embeddings :8083
if ! up 8081; then
  (cd ~/workspace/legal-rag && setsid nohup env NO_PROXY=localhost,127.0.0.1 no_proxy=localhost,127.0.0.1 HF_HUB_OFFLINE=1 RERANK_MODEL=BAAI/bge-reranker-v2-m3 RERANK_PORT=8081 ~/workspace/ocr-bench/bench-env/bin/python tools/rerank/bge_reranker_sidecar.py >> tools/rerank/sidecar.log 2>&1 < /dev/null &)
fi
if ! up 8083; then
  (cd ~/workspace/legal-rag && setsid nohup env NO_PROXY=localhost,127.0.0.1 no_proxy=localhost,127.0.0.1 HF_HUB_OFFLINE=1 EMBED_PORT=8083 ~/workspace/ocr-bench/bench-env/bin/python tools/embed/bge_m3_embeddings_sidecar.py >> tools/embed/sidecar.log 2>&1 < /dev/null &)
fi
# 3b. ColBERT late-interaction :8084 (Tier-2 opt-in; missing from the original restore)
if ! up 8084; then
  (cd ~/workspace/legal-rag && setsid nohup env NO_PROXY=localhost,127.0.0.1 no_proxy=localhost,127.0.0.1 HF_HUB_OFFLINE=1 COLBERT_MODEL=colbert-ir/colbertv2.0 COLBERT_PORT=8084 ~/workspace/ocr-bench/bench-env/bin/python tools/colbert/colbert_sidecar.py >> tools/colbert/sidecar.log 2>&1 < /dev/null &)
fi
sleep 40
# 4. Prod :3000 + tunnel + test :3001
if ! up 3000; then
  (cd ~/workspace/legal-rag && setsid nohup env SERENEDB_URL=postgres://postgres@127.0.0.1:7890/postgres ./target/release/legal-rag-server >> server.log 2>&1 < /dev/null &)
fi
pgrep -f '[v]m-agent.js' >/dev/null || (cd ~/workspace/legal-rag && setsid nohup env WORKER_URL=https://legal-rag-relay.jishan-legal-rag.workers.dev node tools/tunnel/vm-agent.js >> vm-agent.log 2>&1 < /dev/null &)
if ! up 3001; then
  (cd ~/workspace/legal-rag && setsid nohup ./run-test-instance.sh > /dev/null 2>&1 &)
fi
sleep 8
for p in 7890 8081 8082 8083 8084 3000 3001; do up $p && echo "$p UP" || echo "$p DOWN"; done
