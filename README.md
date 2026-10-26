# Juris Vault AI

**Legal document intelligence for Indian law practice** — a retrieval-augmented
generation (RAG) system that answers questions, drafts documents, runs
structured research, and executes agentic legal workflows over a firm's own
document vault, with every answer grounded in cited, verifiable sources.

- **Owner:** Jishan — Juris Vault AI Team
- **Status:** v0.1.0 — feature-complete development system, live-verified end to end (21 Oct 2026)
- **Audience for this repo:** engineering, infrastructure, and AWS platform teams
- **Deep documentation:** [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) · [docs/ADR.md](docs/ADR.md) · [CHANGELOG.md](CHANGELOG.md) · [DEPLOY.md](DEPLOY.md) · [Architecture & ADR as PDF](docs/Juris-Vault-AI-Architecture-and-ADR.pdf)

---

## What it does

| Surface | Capability |
|---|---|
| **Ask** | Grounded Q&A over the vault (plus optional live web blending), streamed token-by-token, with sentence-level citation pinpoints, a citation-integrity audit, an in-app source viewer that highlights the exact cited sentence, and an "As of" control — ask as of a date and jurisdiction, applied as a hard legal-validity boundary. Every cited authority also carries a Citator treatment status with its passage as provenance (Overruled / Followed / Distinguished / Applied / Good law / Unknown — never asserted without a cited passage, never a silent drop), detected deterministically from judgments in the vault at ingest. Retrieval runs a Spice-grade fusion suite rewritten natively in Rust (RRF / weighted RRF / DBSF, an in-fusion statute term, rank-interleave leg merging, and a stale-while-revalidate retrieval cache) — no sidecar search runtime. |
| **Vault** | Document ingestion: PDF (text layer + Surya OCR for scans, incl. Hindi/Devanagari), pasted text, S3 import. Parent-child chunking, knowledge-graph extraction, per-document status, and legal-validity metadata (jurisdiction, in-force dates, superseded-by) per document. Password-protected PDFs accepted via an optional `file_password` (decrypted in memory; wrong password is a distinct error; encrypted S3 objects are skipped per-file, never batch-halting). |
| **Briefs** ("Living Brief") | Matter workspaces. Retrieval is hard-scoped to the brief's documents — a confidentiality boundary, not a filter hint. Briefs can be grouped into multi-tenant **Spaces** with owner/editor/viewer roles; non-members cannot even see that a space's briefs exist. |
| **Interact** | Per-document summary, chronology, key clauses, risks, and grounded chat. |
| **Draft** | Legal notice, agreement, IRAC memo, petition outline, advocate's letter — grounded in vault sources with citations. |
| **Research** | Hypothesis-driven research: framing → evidence gathering → adjudication → verdict-first report. |
| **Agents** | Fixed multi-step workflows (contract review, due diligence, IRAC memo) plus **Autonomous research**: a lead agent splits the goal across specialist researchers (parallel), a drafter, and an independent checker, then reconciles their work into a cited report with per-sub-agent receipts — or the legacy planner-executor loop via `orchestrated: false`. |
| **Private Vault** | Client-confidential documents encrypted at rest (Argon2id → AES-256-GCM); searchable only while unlocked, never written to the shared corpus or logs. Optional dedicated (local) endpoints for private embeddings and answers, so sealed content need never reach the main providers. Password-protected PDF uploads accepted via `file_password` (decrypted in memory, password zeroized and never logged). |
| **Command Center** (`/admin`) | User management, corpus and vault operations, retrieval/model settings, per-surface prompt editing, Verified Answers (human-curated replies), golden-ruleset evaluation, hit-testing, billing and insights. |
| **Billing** | Token-metered plans — Free 50k tokens/mo · Basic ₹2,999/mo (2M) · Premium ₹9,999/mo (10M) — with quota enforcement (HTTP 402) and UPI checkout. |

Product differentiators: published-grade Indic OCR (Surya-only), Hindi↔English
cross-lingual retrieval (bge-m3), Living Brief matter memory, provenance
receipts (citation-integrity audit), and primary-authority-first web research
(Supreme Court / Indian Kanoon / India Code sources are probed and ranked
ahead of news coverage).

## Architecture at a glance

```
┌──────────────────────────── Browser ────────────────────────────┐
│  User app (React 19 SPA)            Admin Command Center (SPA)  │
└──────────────┬───────────────────────────────┬──────────────────┘
               │  HTTPS / SSE                  │
┌──────────────▼───────────────────────────────▼──────────────────┐
│  legal-rag-server  (Rust · Axum · Tokio, single binary)         │
│  /api/v1/auth · /api/v1/vault · /api/v1/knowledge · /api/v1/    │
│  matters · /api/v1/billing · /api/v1/private-vault · /admin/*   │
├──────────────────────────────────────────────────────────────────┤
│  rag crate: ingest pipeline → parent-child chunks → hybrid      │
│  retrieval (vector + BM25 → RRF → reranker) → 6 LightRAG modes  │
│  → grounded generation → citation audit → caches                │
├──────────────┬───────────────┬────────────────┬─────────────────┤
│  LLM +       │  Vector store │  Sidecars      │  Persistence    │
│  embeddings  │  SereneDB     │  reranker :8081│  JSON stores +  │
│  (OpenAI-    │  :7890 (prod) │  OCR (Surya)   │  atomic         │
│  compatible) │  in-mem (dev) │  web search/   │  snapshots      │
│  Cloudflare  │               │  fetch         │  (LEGAL_RAG_    │
│  Workers AI  │               │                │  DATA_DIR)      │
└──────────────┴───────────────┴────────────────┴─────────────────┘
```

Full detail, data flows, and the AWS target topology: **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)**.
Why each choice was made: **[docs/ADR.md](docs/ADR.md)**.

## Repository layout

```
crates/
  common/   Shared config (env-driven) and error types
  auth/     Argon2 password hashing, JWT access/refresh, AuthUser/AdminUser
            extractors, Auth0-compatible OIDC client
  ingest/   S3 vault source (list/download) feeding the pipeline
  rag/      RAG-Anything-faithful pipeline: parsers, typed content blocks,
            context extractor, modal processors, parent-child chunking,
            knowledge graph, hybrid retrieval, six query modes, research
            mode, agentic workflows, caches, citation audit, usage metering
  server/   Axum API (65 routes), both SPAs' static hosting, settings,
            billing, matters, private vault, admin services
frontend/
  src/      User app (Vite + React 19 + TypeScript + Tailwind)
  admin/    Admin Command Center (separate Vite app, served at /admin)
tools/
  web/      websearch.py / webfetch.py — search + fetch sidecars (Python)
  rerank/   bge-reranker-v2-m3 scoring sidecar (HTTP, :8081)
  embed/    local bge-m3 embeddings sidecar (OpenAI-compatible, :8083, test)
  ocr/      Surya OCR assets
  tunnel/   development relay agent (sandbox demos only — not production)
Dockerfile / docker-compose.yml / deploy.sh / DEPLOY.md
docs/       ARCHITECTURE.md, ADR.md, this documentation set
```

## Quick start

### Local (development)

```bash
cargo build --release -p legal-rag-server
cp .env.example .env        # fill in provider keys + a JWT secret
./target/release/legal-rag-server
# App:  http://localhost:3000/        Admin: http://localhost:3000/admin
```

Register the first account in the app. Any email listed in
`LEGAL_RAG_ADMIN_EMAILS` is promoted to admin at register/login.

Mock mode (no external providers):

```bash
LEGAL_RAG_USE_MOCK_PROVIDERS=1 ./target/release/legal-rag-server
```

### Docker

```bash
./deploy.sh                 # or: docker compose up -d --build
docker compose --profile serenedb up -d   # optional SereneDB vector store
```

See [DEPLOY.md](DEPLOY.md). **Note:** the Docker image is authored and
YAML-validated but has not yet had a first build on a Docker host — that is
tracked as an open go-live task in the ADR (ADR-0015).

## Configuration

All configuration is environment-driven (`.env`, see `.env.example` for the
annotated template). The load-bearing variables:

| Variable | Purpose |
|---|---|
| `LEGAL_RAG_ADDR` | Bind address (default `127.0.0.1:3000`) |
| `LEGAL_RAG_DATA_DIR` | JSON stores + snapshots directory |
| `LEGAL_RAG_JWT_SECRET` | JWT signing secret (required in production) |
| `LEGAL_RAG_ADMIN_EMAILS` | Comma-separated emails promoted to admin |
| `LEGAL_RAG_LLM_URL` / `_API_KEY` / `_MODEL` | LLM endpoint (OpenAI-compatible) |
| `LEGAL_RAG_EMBEDDING_URL` / `_API_KEY` / `_MODEL` / `_DIMS` | Embeddings endpoint |
| `SERENEDB_URL` | SereneDB (Postgres wire, :7890); unset = in-memory vectors |
| `LEGAL_RAG_RERANKER` / `LEGAL_RAG_RERANK_URL` | `local` sidecar (default) or `llm` judge |
| `LEGAL_RAG_WEB_SIDECAR` | Path to `tools/web` (web blending on/off: `LEGAL_RAG_WEB_SEARCH`) |
| `LEGAL_RAG_OIDC_*` | Optional OIDC (Auth0-compatible) sign-in |
| `LEGAL_RAG_S3_BUCKET` / `_REGION` / `_PREFIX` | S3 vault import source |
| `LEGAL_RAG_S3_ENDPOINT_URL` | S3-compatible endpoint override (e.g. Cloudflare R2 `https://<account-id>.r2.cloudflarestorage.com`); enables path-style addressing; unset = stock AWS endpoints |
| `LEGAL_RAG_PAYMENT_PROVIDER` | `mock` (test) or `youpi` (UPI; see ADR-0013 caveat) |
| `LEGAL_RAG_SEMANTIC_THRESHOLD` / `LEGAL_RAG_ANNOTATION_THRESHOLD` | Cache / verified-answer match thresholds |
| `LEGAL_RAG_CHILD_CHARS` / `LEGAL_RAG_PARENT_CHILD` | Parent-child chunking controls |
| `LEGAL_RAG_RERANK_POOL` / `LEGAL_RAG_RERANK_TIMEOUT_MS` | Retrieval tuning |

The complete variable inventory (40+) is catalogued in
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#appendix-a--configuration-reference).

## API surface (summary)

Routes under `/api/v1`: `auth` (register/login/refresh/me/OIDC),
`vault` (documents, search, S3 ingest, document metadata), `knowledge`
(query, query/stream SSE, research, workflows, autonomous agent, interact,
draft, source-document, graph, stats, cache-stats), `matters` (briefs,
documents, timeline, memory), `spaces` (multi-tenant spaces and members),
`billing` (plans, checkout, payments, webhook), `private-vault` (status,
password, unlock/lock, documents), `settings/public`, `dashboard/me`, and
`admin/*` (users, settings, prompts, billing, insights, golden ruleset,
annotations, hit-test). Full inventory: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#appendix-b--api-inventory).

## Testing & verification status

- Rust workspace test suite: **110 tests green** at last full run (24 Oct 2026).
- Retrieval benchmarks (offline, reproducible): hybrid BM25+vector fused
  MRR **0.833** vs 0.815 vector-only; reranker decision layer MRR **0.944**;
  Hindi↔English embedding cosine **0.89–0.92**.
- OCR head-to-head: Surya 0.22 beat Tesseract 5 decisively on Hindi and ruled
  tables — the basis for the Surya-only decision (ADR-0007).
- Golden-ruleset evaluation on an expanded 16-document corpus (23 Oct 2026):
  **15/20** with the substance-based evaluator, up from 12/20 — the gain came
  from fixing how Mix mode merges its retrieval legs (rank interleave
  instead of raw-score merge across incommensurable scales) plus an
  in-fusion statute term; remaining misses are answer-wording only, with
  zero source-attribution misses — full classification in
  [CHANGELOG.md](CHANGELOG.md).
- End-to-end product sweeps (21 + 23 Oct 2026): every user and admin flow
  verified live, including a 17-check multi-tenant isolation suite for
  Spaces — details in [CHANGELOG.md](CHANGELOG.md).

## Known limitation

- **Provider quota only.** The production LLM/embedding provider is
  Cloudflare Workers AI on its free tier, which caps usage at **10,000
  neurons/day** (resets 00:00 UTC / 05:30 IST). Hitting the cap surfaces
  as HTTP 429 on embeddings and error 4006 on the LLM until the reset.
  The Workers Paid plan (~$5/mo + usage) removes the cap (ADR-0008,
  decision pending).

## Ownership

© 2026 **Jishan** and the **Juris Vault AI Team**. All rights reserved.
Documentation set version 1.8 — 25 October 2026. This release: verdict-first format for corpus Ask answers, reranker token cache, golden-rule edit endpoint, thumbs up/down feedback loop, R2/S3-compatible endpoint option for the vault import. Tier-2 retrieval arc (ADR-0021): index-time augmentation, statute hyperedges, HyDE/multi-query, ColBERT late interaction (opt-in), Tantivy BM25 option, MCP server.
