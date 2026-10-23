#!/usr/bin/env bash
# LexAgent — one-command deployment.
#
#   ./deploy.sh            build + start + wait until healthy
#   ./deploy.sh down       stop
#   ./deploy.sh logs       follow app logs
#   ./deploy.sh restart    restart the app container
#
# First run creates .env from .env.example and generates a random JWT
# secret. Fill in your model-provider keys in .env (Cloudflare Workers AI
# or any OpenAI-compatible endpoint) — see .env.example for a ready-made
# Cloudflare configuration.
set -euo pipefail
cd "$(dirname "$0")"

cmd="${1:-up}"
case "$cmd" in
  down)    docker compose down; exit 0 ;;
  logs)    docker compose logs -f app; exit 0 ;;
  restart) docker compose restart app; exit 0 ;;
  up)      ;;
  *) echo "usage: ./deploy.sh [up|down|logs|restart]"; exit 2 ;;
esac

command -v docker >/dev/null 2>&1 || {
  echo "✗ Docker is not installed. Install Docker Engine + the Compose v2 plugin first."
  exit 1
}
docker compose version >/dev/null 2>&1 || {
  echo "✗ Docker Compose v2 is required (the 'docker compose' subcommand)."
  exit 1
}

# --- .env bootstrap -----------------------------------------------------------
if [ ! -f .env ]; then
  cp .env.example .env
  echo "→ Created .env from .env.example"
fi
if grep -q "change-me-to-a-long-random-secret" .env; then
  SECRET="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
  sed -i.bak "s/change-me-to-a-long-random-secret/$SECRET/" .env && rm -f .env.bak
  echo "→ Generated a random JWT secret in .env"
fi
if grep -q "<your-account-id>" .env; then
  echo "⚠ .env still contains Cloudflare placeholders (<your-account-id> / <your-api-token>)."
  echo "  Edit .env and set your provider keys, then re-run ./deploy.sh."
  exit 1
fi

# --- build + start ------------------------------------------------------------
docker compose up -d --build

PORT="$(sed -n 's/^LEGAL_RAG_PORT=//p' .env | tail -1)"
PORT="${PORT:-3000}"

echo "→ Waiting for LexAgent to become healthy on port $PORT …"
healthy=""
for _ in $(seq 1 60); do
  if curl -fs "http://127.0.0.1:${PORT}/health" >/dev/null 2>&1; then
    healthy=1; break
  fi
  sleep 2
done

if [ -n "$healthy" ]; then
  echo ""
  echo "✓ LexAgent is up:"
  echo "    App    http://localhost:${PORT}/"
  echo "    Admin  http://localhost:${PORT}/admin"
  echo ""
  echo "  Register the first account in the app; addresses listed in"
  echo "  LEGAL_RAG_ADMIN_EMAILS (.env) are promoted to admin automatically."
else
  echo "✗ The container did not become healthy in time. Inspect with:"
  echo "    docker compose logs app"
  exit 1
fi
