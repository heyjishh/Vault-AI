# Deploying Juris Vault AI

One image, one command. The image contains everything: the Rust API, the
user app, the admin Command Center, the OCR sidecar (Surya; pages
rendered with poppler), and the stealth web-search sidecar.

## Quick start

```bash
./deploy.sh
```

That is the whole flow. On first run it:

1. Creates `.env` from `.env.example` and generates a random JWT secret.
2. Stops and tells you if the Cloudflare placeholders are still unfilled —
   edit `.env` (your account ID + API token; see the ready-made block in
   `.env.example`), then re-run.
3. Builds the image and starts the container.
4. Waits for health, then prints the URLs.

- App: `http://localhost:3000/`
- Admin Command Center: `http://localhost:3000/admin`
- Host port is `LEGAL_RAG_PORT` in `.env` (default 3000).

Register the first account in the app. Any email listed in
`LEGAL_RAG_ADMIN_EMAILS` (`.env`) is promoted to admin automatically and
gets the Command Center link.

## Without the script

```bash
cp .env.example .env   # fill in provider keys + JWT secret
docker compose up -d --build
```

## Day-to-day

```bash
./deploy.sh logs       # follow logs
./deploy.sh restart    # restart the app
./deploy.sh down       # stop (data volume is kept)
```

## What's inside the image

| Piece | Source |
|---|---|
| Rust server (`legal-rag-server`) | `crates/`, release build, `--locked` |
| User app | `frontend/` → served at `/` |
| Admin Command Center | `frontend/admin/` → served at `/admin` |
| OCR | Surya (`tools/ocr/surya_sidecar.py`), the only OCR engine; poppler `pdftoppm` renders pages. The sidecar's Python dep (surya-ocr) is operator-installed or run as a separate service |
| Web-search sidecar | `tools/web` uv venv (invisible-playwright); its patched Firefox downloads on first search |
| Reranker | Local cross-encoder service (`tools/rerank/bge_reranker_sidecar.py`, bge-reranker-v2-m3) via `LEGAL_RAG_RERANK_URL`; LLM listwise reranker optional (`LEGAL_RAG_RERANKER=llm`) |

## Persistence

Snapshots (chunks, knowledge graph, documents, users) live in the
`legal-rag-data` volume at `/data` (`LEGAL_RAG_DATA_DIR`). Back it up with
any volume tooling; the app restores from it on boot.

## Optional: SereneDB vector store

```bash
docker compose --profile serenedb up -d
# and in .env:
SERENEDB_URL=postgres://postgres@serenedb:7890/postgres
```

When `SERENEDB_URL` is set, vectors persist in SereneDB and JSON snapshots
are skipped. Verify SereneDB's data directory against its current Docker
docs before relying on the compose volume for production persistence.

## Notes

- The server binds `0.0.0.0:3000` inside the container (compose sets
  `LEGAL_RAG_ADDR`); outside Docker the default stays `127.0.0.1:3000`.
- `.env` is never baked into the image (`.dockerignore`); compose injects
  it at runtime.
- Vault S3/R2 ingest is configured purely through `.env`
  (`LEGAL_RAG_S3_*` + standard AWS credential chain).
- Building needs network access for crates.io, npm, PyPI, and uv — the
  runtime container needs outbound HTTPS for your model provider, S3,
  and (optionally) web search.
