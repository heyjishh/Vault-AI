#!/usr/bin/env bash
# Juris Vault AI stack health watch.
# Checks SereneDB, the four sidecars, prod :3000, test :3001 and the
# Cloudflare tunnel agent. If anything is down, launches restore-stack.sh
# (detached, idempotent — it only starts what is missing), waits, and
# re-checks. Prints exactly one summary line:
#   HEALTHY
#   RESTORED <what was down>
#   STILL_DOWN <what is still down>
# Appends the same line, timestamped, to health-check.log beside this script.
set -u
cd "$(dirname "$0")"
LOG="$PWD/health-check.log"

port_up() { ss -tln 2>/dev/null | grep -q ":$1 "; }
http_ok() { [ "$(curl -s --noproxy '*' -m 6 -o /dev/null -w '%{http_code}' "http://127.0.0.1:$1/health" 2>/dev/null)" = "200" ]; }

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
  exit 0
else
  echo "$(date -Is) STILL_DOWN:$REMAINING (restore attempted for:$DOWN)" >> "$LOG"
  echo "STILL_DOWN:$REMAINING (restore attempted for:$DOWN)"
  exit 1
fi
