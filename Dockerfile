# syntax=docker/dockerfile:1
# ─────────────────────────────────────────────────────────────────────────────
# Juris Vault AI — all-in-one image: Rust API + user app + admin Command Center +
# OCR sidecar (Surya) + stealth web-search sidecar.
#
#   docker build -t juris-vault-ai .
#   docker run -d -p 3000:3000 --env-file .env -v legal-rag-data:/data juris-vault-ai
#
# Or simply: ./deploy.sh   (docker compose, one command)
# ─────────────────────────────────────────────────────────────────────────────

# ---------- Stage 1: frontends (user app + admin Command Center) ----------
FROM node:20-bookworm-slim AS web
WORKDIR /build/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY frontend/ ./
RUN npm run build
# Admin app has its own package.json (no separate lockfile).
WORKDIR /build/frontend/admin
RUN npm install --no-audit --no-fund && npm run build

# ---------- Stage 2: Rust server ----------
FROM rust:1-bookworm AS rust
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --locked \
 && cp target/release/legal-rag-server /usr/local/bin/legal-rag-server

# ---------- Stage 3: web-search sidecar environment (uv + venv) ----------
FROM python:3.12-slim AS sidecar
RUN pip install --no-cache-dir uv
WORKDIR /app/tools/web
COPY tools/web/ ./
# Creates /app/tools/web/.venv against /usr/local/bin/python3.12 — the same
# interpreter path the runtime stage (python:3.12-slim) provides.
RUN uv sync --locked

# ---------- Stage 4: runtime ----------
FROM python:3.12-slim AS runtime
# PDF page rendering + healthcheck tooling. OCR itself runs in the Surya
# sidecar script (tools/ocr/); its Python dependency (surya-ocr) is
# installed by the operator or run as a separate service; the server only
# needs the command line configured (RAG_ANYTHING_OCR_PRIMARY_CMD).
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl poppler-utils \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=rust /usr/local/bin/legal-rag-server ./legal-rag-server
COPY --from=web /build/frontend/dist ./frontend/dist
COPY --from=web /build/frontend/admin/dist ./frontend/admin/dist
COPY --from=sidecar /app/tools/web ./tools/web
COPY tools/ocr ./tools/ocr
COPY tools/rerank ./tools/rerank

RUN useradd --create-home --uid 10001 lexrag \
 && mkdir -p /data \
 && chown -R lexrag:lexrag /app /data
USER lexrag

# Container layout (all overridable via .env / compose environment).
ENV LEGAL_RAG_ADDR=0.0.0.0:3000 \
    LEGAL_RAG_DATA_DIR=/data \
    LEGAL_RAG_FRONTEND_DIR=/app/frontend/dist \
    LEGAL_RAG_ADMIN_DIR=/app/frontend/admin/dist \
    LEGAL_RAG_WEB_SIDECAR=/app/tools/web

EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD curl -fs http://127.0.0.1:3000/health || exit 1

ENTRYPOINT ["/app/legal-rag-server"]
