#!/usr/bin/env bash
# Juris Vault AI stack health watch.
# Checks SereneDB, the four sidecars, prod :3000, test :3001 and the
# Cloudflare tunnel agent. If anything is down, launches restore-stack.sh
# (detached, idempotent — it only starts what is missing), waits, and
# re-checks. Prints exactly one summary line:
#   HEALTHY
#   RESTORED <what was down>
#   STILL_DOWN <what is still down>
# Plus, when prod reports hollow documents (registered documents whose
# chunks the vector store lost), one additional alert-only line:
#   HOLLOW <n>
# Hollowness is NEVER a restore trigger — a restart cannot heal it.
# Appends the same lines, timestamped, to health-check.log beside this script.
set -u
cd "$(dirname "$0")"
LOG="$PWD/health-check.log"

port_up() { ss -tln 2>/dev/null | grep -q ":$1 "; }
http_ok() { [ "$(curl -s --noproxy '*' -m 6 -o /dev/null -w '%{http_code}' "http://127.0.0.1:$1/health" 2>/dev/null)" = "200" ]; }

# documents_hollow from prod's /health JSON (0 when unreachable/unparsable).
hollow_count() {
  curl -s --noproxy '*' -m 6 "http://127.0.0.1:3000/health" 2>/dev/null \
    | python3 -c 'import sys, json; print(json.load(sys.stdin).get("documents_hollow", 0))' 2>/dev/null \
    || echo 0
}

# Alert-only: prints + logs "HOLLOW <n>" when prod has hollow documents.
emit_hollow() {
  local n
  n="$(hollow_count)"
  if [ "${n:-0}" -gt 0 ] 2>/dev/null; then
    echo "$(date -Is) HOLLOW $n" >> "$LOG"
    echo "HOLLOW $n"
  fi
}

check_all() {
  DOWN=""
  port_up 7890  || DOWN="$DOWN serenedb:7890"
  http_ok 8081  || DOWN="$DOWN reranker:8081"
  port_up 8082  || DOWN="$DOWN gemini-bridge:8082"
  http_ok 8083  || DOWN="$DOWN embeddings:8083"
  port_up 8084  || DOWN="$DOWN colbert:8084"
  http_ok 3000  || DOWN="$DOWN prod:3000"
  http_ok 3001  || DOWN="$DOWN test:3001"
  pgrep -f '[v]m-agent.js' >/dev/null || DOWN="$DOWN tunnel-agent"
  echo "$DOWN"
}

DOWN="$(check_all)"
if [ -z "$DOWN" ]; then
  echo "$(date -Is) HEALTHY" >> "$LOG"
  echo "HEALTHY"
  emit_hollow
  exit 0
fi

# Something is down: restore detached (never hold restore-stack in a
# foreground/background exec session — it must survive independently).
setsid nohup ./restore-stack.sh >> restore-stack.log 2>&1 < /dev/null &

# Sidecars take up to ~60s to load models; poll for up to 150s.
REMAINING="$DOWN"
for i in $(seq 1 15); do
  sleep 10
  REMAINING="$(check_all)"
  [ -z "$REMAINING" ] && break
done

if [ -z "$REMAINING" ]; then
  echo "$(date -Is) RESTORED was:$DOWN" >> "$LOG"
  echo "RESTORED was:$DOWN"
  emit_hollow
  exit 0
else
  echo "$(date -Is) STILL_DOWN:$REMAINING (restore attempted for:$DOWN)" >> "$LOG"
  echo "STILL_DOWN:$REMAINING (restore attempted for:$DOWN)"
  emit_hollow
  exit 1
fi
