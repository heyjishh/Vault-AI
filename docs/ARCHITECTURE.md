# Juris Vault AI — System Architecture

| | |
|---|---|
| **Document** | System Architecture (engineering, infrastructure & AWS platform teams) |
| **Product** | Juris Vault AI — legal document intelligence for Indian law practice |
| **Owner** | **Jishan** — Juris Vault AI Team |
| **Version** | 1.4 — 23 October 2026 |
| **System version** | 0.1.0 (Cargo workspace version) |
| **Companion documents** | [ADR.md](ADR.md) (decision records ADR-0001…0018) · [README.md](../README.md) · [CHANGELOG.md](../CHANGELOG.md) · [DEPLOY.md](../DEPLOY.md) |
| **Status labels** | **LIVE-VERIFIED** = proven end-to-end in production-like runs · **BUILT** = implemented, verification partial · **TARGET** = design for the AWS move, not yet deployed · **ROADMAP** = agreed direction, not built |

> Read this first if you are joining the infra/AWS effort. Section 12 is the
> part that needs you: today's runtime is a development sandbox behind a
> relay; the AWS target topology is fully specified there and in ADR-0016.

---

## 1. What the system is

Juris Vault AI is a retrieval-augmented generation (RAG) platform for law
firms: users upload legal documents (statutes, judgments, contracts, scanned
court papers — English and Hindi), and the system answers questions, drafts
documents, runs structured research, and executes multi-step legal workflows
over that corpus. Every generated statement is grounded in retrieved sources,
cited at chunk level, and — for web-blended answers — passed through a
citation-integrity audit that strips unsupported citations before the user
sees them.

Design principles, in priority order:

1. **Provenance over fluency.** A citation the model cannot support is worse
   than no citation. The integrity audit, primary-source ranking, and source
   viewer all exist to serve this.
2. **Confidentiality is architectural, not cosmetic.** Brief-scoped retrieval
   and the encrypted Private Vault enforce boundaries in the retrieval layer,
   not in the UI.
3. **India-first.** Indic OCR (Surya), Hindi↔English cross-lingual retrieval,
   Indian primary authorities (Supreme Court, Indian Kanoon, India Code),
   INR pricing, data residency target `ap-south-1` (Mumbai).
4. **Measure, then decide.** Retrieval changes ship only with offline bench
   numbers; model changes ship only after head-to-heads (see §14).
5. **Honest surfaces.** Roadmap features are labelled roadmap in-product;
   verified vs unverified status is tracked in the changelog.

## 2. System context

```
                        ┌─────────────────────────┐
   Lawyers / firm staff │        Browser          │  Admins (firm + platform)
   ───────────────────► │  User SPA · Admin SPA   │ ◄───────────────────
                        └────────────┬────────────┘
                                     │ HTTPS, SSE streaming
                        ┌────────────▼────────────┐
                        │     Juris Vault AI      │
                        │  legal-rag-server (Rust)│
                        └──┬───┬───┬───┬───┬───┬──┘
             ┌─────────────┘   │   │   │   │   └─────────────┐
             ▼                 ▼   ▼   ▼   ▼                 ▼
   ┌──────────────┐ ┌────────────┐ ┌─────────┐ ┌──────────────┐ ┌────────────┐
   │ LLM provider │ │ Embeddings │ │ SereneDB│ │  UPI payment │ │ Public web │
   │ Cloudflare   │ │ Cloudflare │ │ (opt.)  │ │  YouPI-PG    │ │ DDG/Bing + │
   │ Workers AI   │ │ / local    │ │ :7890   │ │  (or mock)   │ │ publishers │
   │ llama-3.3-70b│ │ bge-m3     │ │         │ │              │ │ SCI/Kanoon │
   └──────────────┘ └────────────┘ └─────────┘ └──────────────┘ └────────────┘
             ┌──────────────────────────────────────────────┐
             │ AWS S3 (document lake import, ingest crate)  │
             └──────────────────────────────────────────────┘
```

External dependencies and their failure behaviour:

| Dependency | Used for | If it fails |
|---|---|---|
| LLM endpoint (OpenAI-compatible; today Cloudflare Workers AI) | Answer generation, graph extraction, research, workflows, interact/draft | Query path retries with backoff (4 attempts), then a friendly error; cached answers keep serving |
| Embeddings endpoint (today Cloudflare; local sidecar exists) | Ingest + query vectors | Same retry policy; cache hits unaffected |
| SereneDB (optional) | Persistent vector store | Falls back to in-memory index with a warning at boot |
| YouPI-PG (or mock) | UPI checkout | Checkout unavailable; metering/quota unaffected |
| Public web (DDG/Bing HTML + stealth browser fallback) | Web blending, research evidence | Web leg returns vault-only answers; per-source failures tolerated |
| AWS S3 | Bulk document import | Import endpoint errors; uploads via UI unaffected |

## 3. Container & component architecture

### 3.1 Runtime components

| # | Component | Tech | Port | Role |
|---|---|---|---|---|
| C1 | `legal-rag-server` | Rust (Axum 0.8 / Tokio 1), single static-ish binary | **3000** | API (65 routes), serves both SPAs, all product logic |
| C2 | User SPA | React 19.2 · TS 6 · Vite 8 · Tailwind 3.4 | served by C1 at `/` | Ask, Vault, Briefs, Interact, Draft, Research, Agents, Private Vault, Plans & Billing, Dashboard |
| C3 | Admin SPA ("Command Center") | React 19.2 · separate Vite app | served by C1 at `/admin` | Users, corpus, settings/prompts, billing, insights, golden rules, Verified Answers, hit-test |
| C4 | Reranker sidecar | Python (sentence-transformers), bge-reranker-v2-m3 | **8081** | Cross-encoder scoring of the fused retrieval pool |
| C5 | OCR sidecar | Python, Surya 0.22 (+ llama.cpp backend) | subprocess | OCR for scanned PDF pages (text layer first; OCR only where needed) |
| C6 | Web sidecars | Python (`tools/web/websearch.py`, `webfetch.py`) + stealth Firefox fallback | subprocess | Search, primary-source probes, page/PDF fetching and extraction |
| C7 | SereneDB (optional) | `serenedb/serenedb` image | **7890** | Vector store over Postgres wire protocol (`FLOAT[N]`, `<=>`, IVF index) |
| C8 | Embeddings sidecar (test/local) | Python, bge-m3 fp16, OpenAI-compatible | **8083** | Local embeddings — removes provider dependency; test stack uses it by default |
| C9 | Gemini bridge (TEST ONLY) | gemini-web2api, anonymous mode | **8082** | Lets the isolated test instance use Gemini Flash as its LLM; **never** in the production path |
| C10 | Relay agent (DEV/DEMO ONLY) | Node (`tools/tunnel/vm-agent.js`) + Cloudflare Worker | — | Exposes the sandbox build at a public URL. **Not a production component** — replaced by ALB in the AWS target |

### 3.2 Component diagram

```
Browser ──HTTPS──► [C1 legal-rag-server :3000]
                     ├── static: C2 user SPA (/) · C3 admin SPA (/admin)
                     ├── /api/v1/* ──► server crate route modules
                     │        ├── auth (Argon2/JWT/OIDC) ── users.json
                     │        ├── matters (Briefs) ── matters.json
                     │        ├── billing ── billing.json, usage metering
                     │        ├── private_vault ── private_vault.json (ciphertext)
                     │        └── knowledge ──► [rag crate QueryService]
                     ├── rag crate
                     │        ├── ingest pipeline ──► parsers ──► [C5 Surya OCR]
                     │        ├── vector store ──► in-memory │ [C7 SereneDB]
                     │        ├── retrieval ──► [C4 reranker :8081]
                     │        ├── web leg ──► [C6 websearch/webfetch]
                     │        └── LLM/embeddings clients ──► provider APIs │ [C8]
                     └── persistence ──► DATA_DIR JSON stores + snapshots
```

## 4. Backend architecture (Rust workspace)

Cargo workspace, resolver 2, edition 2021, version 0.1.0. Five crates with a
strict dependency direction (`server → rag/ingest/auth → common`):

| Crate | Contents (key modules) |
|---|---|
| `common` | Env-driven `Config`, shared `Error` type |
| `auth` | Argon2 password hashing, JWT access/refresh issuance + `AuthUser` / `AdminUser` Axum extractors, Auth0-compatible OIDC client (login/callback) |
| `ingest` | S3 source: list/download raw object bytes for the pipeline (`LEGAL_RAG_S3_*`) |
| `rag` | The intelligence core — see §5. Modules: `config`, content blocks + `parsers`, `ContextExtractor`, modal processors, `pipeline` (incl. parent-child split), `graph` extraction, `bm25`, `query` (six modes, caches, rerank), `audit` (citation integrity), `web` (StealthWebSearch), `research`, `agent` (workflows), `cache`, `persist`, `usage` (metering), vector stores (in-memory, `SereneDbVectorStore`) |
| `server` | Axum wiring + route modules: auth, vault, knowledge, matters, billing, private vault, settings, dashboard, admin (users/settings/billing/insights/golden/annotations/hit-test), SPA static hosting, `settings.rs` (hot-applied admin config), `matters.rs`, `annotations.rs` |

Cross-cutting behaviour in the server:

- **Settings system** — admin-editable config persisted to `data/settings.json`:
  default query mode, per-surface system prompts (`ask`, `interact`, `draft`,
  `research_frame`, `research_adjudicate`, `agent`), LLM/embedding model
  names, brand, custom sidebar tabs. Prompt/mode/LLM changes apply **hot**
  (per request / `DynamicLlm` wrapper); the embedding model is boot-applied
  and requires restart + re-index (the UI warns).
- **Activity capture** — every user action recorded to `data/activity.json`
  (cap 5,000, FIFO) powering the Dashboard, history restore, golden-rule and
  annotation promotion. Private-vault content is redacted before logging.
- **Metering** — `usage.rs` records real provider token usage per call across
  query/stream/research/workflows/uploads; cache hits bill 0. Quota exhaustion
  returns HTTP 402.

## 5. Retrieval & generation pipeline

### 5.1 Ingest flow (LIVE-VERIFIED)

```
Upload (text / base64 file / S3 object)
  → parse: text layer (PDF) → typed content blocks (text/image/table/equation)
  → pages with insufficient text rendered (poppler) → Surya OCR (C5)
  → modal processors describe non-text blocks with extracted context
  → chunk: parent sections; each parent split into ~220-char children
     (sentence-aware incl. Devanagari danda) — children are the match units,
     parents are context units (LEGAL_RAG_PARENT_CHILD, LEGAL_RAG_CHILD_CHARS)
  → embed children (+ parents retained) with bge-m3, 1024-dim
  → LLM knowledge-graph extraction over parents (entities/relations)
  → index: vector store + BM25 corpus + document registry
  → DocStatus: pending → processing → ready / failed
```

### 5.2 Query flow (LIVE-VERIFIED)

```
Question
  1. Verified Answers check — curated reply at cosine ≥0.95, same scope +
     corpus fingerprint → served with zero LLM calls (never for web/private)
  2. Semantic answer cache — cosine ≥0.97, scope-keyed → replayed
     (stream hits replay Sources → Deltas → Done); never web/private
  3. Retrieval pool (three legs in parallel, tokio::join!):
       vector leg (wide: top_k×4 over children) + BM25 leg + graph/mode leg
       (LightRAG modes: naive/local/global/hybrid/mix/bypass; admin-set
       default = mix; brief scope hard-filters chunks + graph context)
  4. RRF fusion (k=60) → pool capped at 12
  5. Reranker (C4 cross-encoder, 500-char excerpts, 12s timeout,
     decisive-margin skip gates; vector order is the fallback)
  6. Parent expansion — matched children replaced by parent sections,
     siblings collapsed; citations point at parents
  7. Optional web leg (if ?web=true): primary-source probes + tiered search
     → two-wave fetch (top-3 protected wave) → best-passage extraction
  8. Private leg (if private=true and vault unlocked): owner's chunks only,
     top-3 decrypted in memory, [private:N] citations
  9. Generation (streaming SSE: heartbeat Status every 1.6s → Sources →
     Deltas → Done; verdict-first format addendum when web is blended)
 10. Citation-integrity audit — every [doc|web|private:N] marker checked
     against the exact excerpt shown to the model; <50% key-term support
     or unknown source → marker stripped; receipt returned to the client
 11. Metering + activity capture (+ answer-cache store when ≥1 citation)

Citation pinpoints (23 Oct 2026): each citation also carries the exact
sentence in the passage that supports the claim — selected by distinct
query-term coverage over flowing (de-wrapped) sentence segmentation
(English + Hindi danda), and only when the candidate is a complete
sentence (terminal punctuation, 40–700 chars). Cards display the
pinpoint; the source viewer highlights that sentence. Two retrieval
knobs are env-gated for corpus-scale tuning: `LEGAL_RAG_DEFAULT_TOP_K`
(request default, 5) and `LEGAL_RAG_PER_DOC_CAP` (per-document chunk cap
in the final context, off). Golden-set measurement on the 16-document
eval corpus found neither knob rescues ranking-level misses — the
rerank pool (step 4–5) is the binding constraint there.

### 5.3 Research & workflows

- **Research** (`research.rs`): frame hypotheses from the question → gather
  evidence per hypothesis (vault ± web) → adjudicate (IN FAVOUR / AGAINST /
  OPEN with rationale) → verdict-first report with citations. Prompts are
  admin-editable; overrides must preserve the JSON verdict contract.
- **Workflows** (`agent.rs`): fixed step pipelines — `contract_review` (5),
  `due_diligence` (4, incl. web), `memo_draft` (4, IRAC). Each step retrieves,
  generates, and cites; a final report synthesizes.
- **Autonomous agent** (`orchestrator.rs`, 23 Oct 2026): a planner–executor
  loop over the same tools — given a goal, the model plans one step at a
  time (search the vault, brief-scoped and space-checked, or search the
  web), up to a step budget (1–10, default 6), then synthesizes a
  verdict-first report from its findings only, with deduplicated
  citations. `POST /api/v1/knowledge/agents/autonomous`, billed like a
  workflow run. Honest limit: the planner is the main LLM — there are no
  separate sub-agent models.

## 6. Frontend architecture

Two independent Vite SPAs served as static files by C1 (no server restart
needed for frontend deploys — the server reads `dist` from disk per request
path config `LEGAL_RAG_FRONTEND_DIR` / `LEGAL_RAG_ADMIN_DIR`).

- **Design system** — REA-derived from Harvey's production UI: ivory `#fafaf9`,
  ink `#0f0e0d`, warm-grey scale, hairline borders, `rounded-lex` radii.
  Spec: `frontend/DESIGN.md`.
- **User app** — view-state routing in `AppShell` (Dashboard, Ask, Vault,
  Briefs, Interact, Draft, Research, Agents, Private Vault, Plans & Billing,
  Graph). Brief selection ("Working in") scopes Ask/Draft/Vault app-wide via
  a shared active-brief context. History restore rehydrates full results
  (answers + citations + sources sidebar) from the activity store.
- **Source viewer** — shared component (`components/SourceModal.tsx`,
  portal-rendered) used by Ask, Draft, Research, Agents: opens the full source
  document (via `POST /api/v1/knowledge/source-document`) with the cited
  passage highlighted (whitespace-normalized search, index-mapped back to
  original offsets, best-overlap fallback), provenance header (domain/date),
  and footer actions (Open original / Open in Interact / Private Vault).
- **Clause** — the assistant persona (avatar with live states) and the
  floating glass pet companion (draggable, expressions, page-aware tips,
  persisted position/size in `localStorage`). Pure frontend; no API impact.
- **Admin app** — separate bundle + route guard (admin role required);
  eight Command Center sections sharing one container.

## 7. Data architecture

### 7.1 Stores (current — interim, see ADR-0010)

All state lives under `LEGAL_RAG_DATA_DIR` as JSON, written atomically
(temp-file + rename). Snapshotter persists the RAG corpus every 5 minutes
and on SIGTERM/SIGINT; state restores on boot.

| File | Contents | Sensitivity |
|---|---|---|
| `users.json` | Accounts, Argon2 hashes, roles (admin/user), plan | High |
| `documents.json` / `chunks.json` / `graph.json` | Corpus registry, chunk texts + vectors (in-memory mode), knowledge graph | Client data |
| `matters.json` | Briefs: documents, timeline, memory notes, optional `space_id`, document legal-validity metadata (jurisdiction, in-force range, superseded-by) | Client data |
| `spaces.json` | Multi-tenant Spaces: owner + members with roles (owner/editor/viewer) | Client data |
| `private_vault.json` | Private documents — **ciphertext only** + SHA-256 fingerprints | Highest (encrypted) |
| `billing.json` | Plans, subscriptions, payments (UTR refs), usage | Financial |
| `activity.json` | Action history (≤5,000 entries; private content redacted) | Medium |
| `annotations.json` | Verified Answers (curated Q/A + stored question vectors) | Medium |
| `golden.json` | Golden-ruleset evaluation cases | Low |
| `settings.json` | Admin settings + prompt overrides | Medium |

### 7.2 Vector data

- Embedding space: **bge-m3, 1024 dimensions**, cosine similarity. Cross-
  lingual by design (Hindi query ↔ English document cosine ≈ 0.89–0.92).
- In-memory store: brute-force cosine over all records; corpus fingerprint
  (hash of doc set) versions every cache/annotation scope. Development
  fallback.
- SereneDB store (**production since 23 Oct 2026**, SereneDB 26.10.3):
  `FLOAT[1024]` column, IVF index
  (`USING inverted (embedding ivf (metric='cosine'))`), `<=>` operator
  (results cast to `float8` — the wire type is not directly decodable),
  Postgres wire protocol v3 on :7890, no auth by default (bind privately).
  The snapshotter restores into and saves from whichever store is active,
  so an externally-backed deployment boots with its corpus.

## 8. Security architecture

| Layer | Mechanism (LIVE-VERIFIED unless noted) |
|---|---|
| Transport | HTTPS at the edge (relay today; ALB + ACM in target) |
| Authentication | Email/password (Argon2id hashes) → JWT access + refresh pair (`LEGAL_RAG_JWT_*_TTL`); optional OIDC (Auth0-compatible) |
| Authorization | `User.role` (admin/user); `LEGAL_RAG_ADMIN_EMAILS` promotes at login/register; `AdminUser` extractor → 403 for non-admins on all `/admin/*` |
| Brief confidentiality | Retrieval hard-filters to the brief's document set for chunks **and** graph context; zero in-scope hits → grounded refusal, no silent full-vault fallback (fallback only when scope matches nothing retrieved, and it is disclosed in-product as a scope note) |
| Spaces (multi-tenancy) | Briefs may belong to a Space with owner/editor/viewer membership. Enforcement is server-side on every surface: brief lists and detail, scoped queries, vault document lists, and the source viewer. Non-members get **404** (a space brief's existence is itself confidential); viewers read but cannot upload or edit (**403**); deleting a space un-files its briefs rather than deleting them. Admins bypass membership. Verified live with a 17-check isolation suite across three accounts (23 Oct 2026) |
| Legal-validity scoping | Documents carry jurisdiction / effective-from / effective-to / superseded-by metadata. Ask's "As of" (date + jurisdiction) applies metadata as a **strict** boundary — stricter than brief scoping: no full-vault fallback, and superseded sources are flagged on their citations |
| Private Vault | Per-user password → Argon2id KDF → AES-256-GCM; key exists only in server memory while unlocked; **no recovery by design**; titles + chunks stored only as ciphertext; uploads never enter the shared corpus, snapshots, or graph; activity logs redact private content. Optional dedicated endpoints (`LEGAL_RAG_PRIVATE_EMBEDDING_URL` / `LEGAL_RAG_PRIVATE_LLM_URL`) route vault uploads, private question embeddings and private answers to local providers; private clients are never wrapped in the shared semantic cache, and the vault status reports where private traffic goes. *Honest limits: without the dedicated endpoints, an unlocked vault's decrypted context is sent to the main LLM for answering and embeddings are computed by the main provider; vectors sit beside ciphertext; switching the private embedder's model/dimension requires re-uploading sealed documents.* |
| Secrets | Provider keys + JWT secret in `.env` (gitignored) / Secrets Manager in target; never in code, logs, or docs |
| Web fetching | Per-source failure isolation; fetch caps (10MB), timeouts; fetched content is data, never instructions |
| Payments | YouPI webhook verified with HMAC-SHA256; mock provider is test-only |

## 9. Billing architecture

- **Plans** (30-day periods, token-metered): Free 50,000 · Basic ₹2,999 /
  2,000,000 · Premium ₹9,999 / 10,000,000 tokens. No seat minimums.
- **Metering** counts real provider usage per operation; semantic-cache and
  verified-answer hits bill 0. Over-quota → HTTP 402 before generation.
- **Checkout**: `mock` (instant, test) or `youpi` — redirect to self-hosted
  YouPI-PG, poll for completion, HMAC-verified webhook upgrades the plan.
- **Admin**: MRR, collected totals, per-user spend, payment list with UTRs.
- **Risk note**: YouPI-PG is experimental software — see ADR-0013 before
  processing real payments on AWS.

## 10. Performance & measured capacity (21 Oct 2026)

| Metric | Value | Source |
|---|---|---|
| Healthy Ask latency (cached provider) | sources 2.6s · first word 3.6s · done 4.6s | live stream timing |
| Semantic-cache hit | 0.45s end-to-end | live |
| Web-blended Ask | ~36s (was 159s before concurrent fetch window) | live |
| Reranker quality | MRR 0.944 (vs 0.815 unranked) | offline bench, 12-q set |
| Hybrid retrieval | fused MRR 0.833, recall@3 1.0 | offline bench, core-9 |
| Parent-child (diluted corpus) | MRR 0.833 vs 0.639 parent-only | offline bench |
| OCR throughput | ~20 min/dense page on 2 vCPU — **GPU host required in production** | sandbox measurement |
| Rerank sidecar | ~9.5s per 10×500-char pool on 2 vCPU | sandbox measurement |
| Provider quota (free tier) | 10,000 neurons/day, resets 00:00 UTC (05:30 IST) | observed 429 / code 4006 |

Sandbox numbers are from a 2-vCPU dev box — treat as floor, not capacity
plan. AWS sizing guidance is in §12.4.

## 11. Reliability & failure modes

| Failure | Signature | System behaviour / recovery |
|---|---|---|
| LLM/embeddings quota exhausted | 429 (embeddings), code 4006 (LLM) after ~28s of retries | Friendly rate-limit error with Retry; cache + verified answers keep serving |
| SereneDB unreachable at boot | warning log | In-memory store; corpus rebuilt from snapshots |
| Reranker sidecar down | timeout (12s) | Vector order used; answers still grounded |
| Web source blocked/slow | per-source timeout (30s fetch, 45s wave) | Source skipped; DDG may anomaly-block datacenter IPs after bursts (HTTP 202) → stealth-browser fallback engages |
| Process kill / host restart | — | Snapshots restore corpus + stores on boot; sidecars are stateless and restart independently |
| Frontend deploy | — | Static files swapped on disk; no restart |

**Restore drill (dev sandbox)**: start C1 → relay agent → sidecars (rerank
~45s model load, embeddings ~85s) → test instance. In the AWS target this is
replaced by ECS service recovery + ALB health checks (ADR-0016).

## 12. Deployment architecture

### 12.1 Today — development/demo (NOT production)

Single sandbox VM: C1 on :3000 (+ test instance on :3001 with an isolated
data copy and the Gemini bridge), sidecars on :8081/:8083, exposed publicly
through a Cloudflare Worker relay (C10). Purpose: demos and verification.
It has no SLA, restarts with the sandbox, and must not hold real client data.

### 12.2 Single-host Docker (small deployments)

`Dockerfile` (4-stage → `python:3.12-slim` runtime with poppler, non-root,
`/data` volume, healthcheck) + `docker-compose.yml` (app service; optional
`serenedb` profile). One command via `./deploy.sh`. **Status: authored and
validated, first Docker-host build pending** (ADR-0015).

### 12.3 AWS target topology (TARGET — for the AWS team)

```
                         ┌────────────── Route 53 ──────────────┐
                         │  app.jurisvault.ai · admin (same host) │
                         └──────────────────┬───────────────────┘
                                     ┌──────▼──────┐
                                     │ CloudFront  │  (SPA static + /api bypass)
                                     └──────┬──────┘
                                     ┌──────▼──────┐
                                     │     ALB     │  HTTPS (ACM), health /health
                                     └──────┬──────┘
        ┌──────────────────────── VPC ap-south-1 (Mumbai) ───────────────────────┐
        │  Public subnets (2 AZ): ALB, NAT GW                                    │
        │  Private subnets (2 AZ):                                               │
        │   ┌──────────────────── ECS Fargate services ────────────────────┐     │
        │   │ app (C1) ×2 tasks        rerank (C4) ×1     ocr (C5) queue-fed│    │
        │   │ embed (C8) ×1 (GPU opt.) web (C6) ×1                           │    │
        │   └──────┬───────────────┬──────────────┬──────────────┬──────────┘    │
        │          ▼               ▼              ▼              ▼               │
        │   RDS PostgreSQL    SereneDB on EC2  S3 buckets    Secrets Manager     │
        │   (app state,       (vector store,  (documents,   (provider keys,     │
        │   Multi-AZ)         private subnet)  snapshots)    JWT secret)         │
        │  Observability: CloudWatch Logs + metrics + alarms · ECR images       │
        │  Backups: RDS automated + S3 versioning · DR: snapshot copy to        │
        │  ap-south-1 AZ pair; RPO 24h / RTO 4h at launch tier                  │
        └────────────────────────────────────────────────────────────────────────┘
```

### 12.4 AWS service mapping & sizing (launch tier)

| Component | AWS service | Launch sizing | Notes |
|---|---|---|---|
| App server C1 | ECS Fargate | 2 tasks · 2 vCPU / 4 GB | Stateless once §12.5 lands; scale on CPU/requests |
| Reranker C4 | ECS Fargate (CPU) or EC2 g5.xlarge (GPU) | 1 task · 4 vCPU / 8 GB | GPU removes the ~9.5s/pool CPU cost; benchmark before choosing |
| OCR C5 | ECS on EC2 GPU (g5) + SQS queue | 1 node, autoscale 0–2 | Surya is GPU-bound in production; queue decouples ingest latency |
| Embeddings C8 | ECS Fargate / GPU shared with C4 | 1 task | bge-m3 local = zero per-call provider cost + no quota wall |
| Web sidecar C6 | ECS Fargate | 1 task · 2 vCPU / 4 GB | Headless browser memory-bound |
| App state (target) | RDS PostgreSQL 16, Multi-AZ | db.t4g.medium | Replaces JSON stores (ADR-0010 migration) |
| Vectors | SereneDB on EC2 (private) **or** RDS pgvector | m6g.large | Decision in ADR-0006: SereneDB first (already integrated); pgvector is the fallback |
| Documents/snapshots | S3 (versioned, SSE-KMS) | — | Also keeps the S3 ingest source |
| Static SPAs | Served by C1 at launch; CloudFront+S3 later | — | Splitting is a config change, not a rewrite |
| Secrets | Secrets Manager | — | Injected as env vars (same names as Appendix A) |
| CI/CD | ECR + CodePipeline/GitHub Actions | — | `cargo` release build + two Vite builds → one image |
| Region | **ap-south-1 (Mumbai)** | — | Indian legal data residency; LLM provider calls leave region (disclose in DPA) |

### 12.5 Migration deltas for AWS (engineering checklist)

1. JSON stores → RDS PostgreSQL (schemas exist 1:1 from §7.1; keep the
   atomic-write repository layer as the seam). Vectors → SereneDB (§7.2).
2. Local `LEGAL_RAG_DATA_DIR` volume → EFS or S3-backed snapshot sync during
   transition; final state: no local state on tasks.
3. `.env` → Secrets Manager + ECS task env (same variable names).
4. Relay agent (C10) deleted; ALB health check on `GET /health`.
5. YouPI-PG: compliance review **or** replace with a mainstream PG (ADR-0013).
6. Cloudflare Paid **or** Bedrock/local models — provider decision (ADR-0008);
   the OpenAI-compatible client means this is config, not code.
7. First Docker build + load test on a Docker host (open since 20 Oct).

### 12.6 Alternative under verification — the shared AWS estimate (19 Oct 2026)

An AWS Pricing Calculator estimate was prepared and **already shared with the
AWS team on 19 Oct 2026**, before this document set existed. At the owner's
direction it is recorded here in full as a **deployment alternative** — the
AWS team is asked to verify it against §12.3/§12.4 and ADR-0016, **whether or
not it becomes the adopted deployment**. It is an EC2-centric design (single
API instance, Spot GPU ingest workers, dedicated SereneDB host), sized for a
16 TB corpus and a ~$2k/month budget, where §12.3 proposes ECS Fargate with
Multi-AZ state.

- **Estimate**: "My Estimate", AWS Pricing Calculator, dated 19 Oct 2026
  (pricing as of that date; read-only share)
- **Link**: https://calculator.aws/#/estimate?id=50a088f3e134c1dcb06525fc8e6b32c956e71512
- **Updated link (with WAF, 23 Oct 2026)**:
  https://calculator.aws/#/estimate?id=85ab95370800b927cdd6e0264e90946f86481e2d
  — in the calculator the WAF line shows $24.00/mo (total $2,039.78/mo),
  because its WAF form cannot model Bot Control's $10/mo subscription and
  per-request charges. This document keeps the published-pricing figure of
  $43.00/mo (total $2,058.78/mo) as canonical — owner decision, 23 Oct 2026.
- **Region**: Asia Pacific (Mumbai) for every line — consistent with ADR-0017
- **Totals**: upfront **$0.00** · monthly **$2,058.78** · 12 months
  **$24,705.36** (USD, taxes excluded per AWS's own acknowledgement).
  The 13 line items as shared sum exactly to $2,015.78 (verified
  21 Oct 2026); line 14 (AWS WAF, $43.00) was added 23 Oct 2026 at the
  owner's direction — the shared estimate had priced no WAF — bringing
  the monthly total to $2,058.78.

| # | Service | Configuration (as estimated) | $/mo |
|---|---|---|---|
| 1 | EC2 | API server — m7i-flex.4xlarge, On-Demand, 730 h, ×1 ("scaled for $2k/mo budget") | 588.38 |
| 2 | EC2 | GPU ingest worker — g4dn.xlarge, Spot (64% discount), ×1 | 152.16 |
| 3 | EC2 | Dedicated SereneDB — t3a.xlarge, On-Demand, 730 h, ×1 | 71.98 |
| 4 | EBS | gp3, 2,218 GB total (API root 80 GB + SereneDB root 50 GB + SereneDB data 2,048 GB + GPU roots 2×20 GB), 3,000 IOPS, 125 MBps, no snapshot storage | 202.28 |
| 5 | RDS for PostgreSQL | Metadata DB — db.t4g.small, **Single-AZ**, 40 GB gp3, On-Demand | 35.90 |
| 6 | S3 | Source corpus — 16,384 GB Standard, 2M PUT/COPY/POST/LIST, 10M GET/other | 423.60 |
| 7 | Elastic Load Balancing | 1 Application Load Balancer | 20.37 |
| 8 | S3 | SereneDB nightly pg_dump backups — 1,024 GB, 7-day retention | 25.61 |
| 9 | Secrets Manager | 1 secret, 10,000 API calls/mo | 0.45 |
| 10 | CloudWatch | 20 GB log ingestion, 5 standard alarms, 1 dashboard, OTEL events | 13.99 |
| 11 | Data Transfer | Internet egress 3,000 GB/mo | 327.90 |
| 12 | ECR | Container images, 10 GB stored | 1.00 |
| 13 | EC2 | GPU ingest worker #2 — g4dn.xlarge, Spot, ×1 (parallel ingest for 16 TB) | 152.16 |
| 14 | AWS WAF | Web ACL on the ALB (added 23 Oct 2026) — 1 Web ACL ($5.00); 6 charged rules ($6.00): AWS Managed Core, Known Bad Inputs, Amazon IP Reputation, SQL Database rule sets, Bot Control (Common) rule group, 1 rate-based rule (2,000 req/5 min/IP, protects the auth endpoints); Bot Control subscription ($10.00); requests at a 20M/mo planning figure: base inspection 20M × $0.60/M ($12.00) + Bot Control Common beyond the 10M free tier 10M × $1.00/M ($10.00) | 43.00 |
| | **Total** | | **2,058.78** |

Design notes and the verification questions this raises for the AWS team:

- **Topology contrast**: EC2 hosts with named roles vs ADR-0016's Fargate
  services. This estimate has **no HA** for the API (one instance) and a
  **Single-AZ** RDS; ADR-0016 proposes 2 app tasks and Multi-AZ. Verify
  which availability posture production actually requires.
- **The 16 TB corpus assumption drives $649.88/mo** (S3 corpus $423.60 +
  SereneDB data volume inside EBS) — confirm the real corpus size at launch;
  the current production corpus is three documents. Right-sizing this line
  is the single largest saving available.
- **Egress is the second-largest cost** ($327.90 for 3 TB/mo at "thousands
  of users/day"). Verify the traffic assumption; also check for double
  counting — the EC2 rows select 0 TB data transfer while this separate
  Data Transfer line carries the full 3 TB.
- **Spot GPU workers** (2× g4dn.xlarge at a 64% Spot discount) fit the
  queue-fed OCR/ingest design (ADR-0007) well, but Spot interruption
  handling must be designed (checkpointed ingest jobs); the estimate prices
  them as "consistent" workloads.
- **Gaps to price or rule out**: no CloudFront, no NAT Gateway (WAF was
  priced as line 14 on 23 Oct 2026 — its 20M requests/mo planning figure
  should be reconciled with the 3 TB egress assumption), EC2
  detailed monitoring disabled (CloudWatch is estimated separately), no
  backup cost for the API/SereneDB EBS volumes beyond the S3 pg_dump line,
  and no LLM/provider spend (ADR-0008 — correctly outside AWS, but it
  belongs in the same budget conversation).
- Line descriptions still carry the legacy pre-rename product name (the
  estimate predates the Juris Vault AI rename) — cosmetic only.

**Status**: recorded as an alternative under verification (owner direction,
21 Oct 2026). Adoption is a separate decision, taken with ADR-0016.

## 13. Observability

- **Today**: structured stdout logs per service; `/health` on C1 and each
  sidecar; admin Insights (usage, top questions, 14-day activity);
  `/api/v1/knowledge/cache-stats` (cache hit economics); activity store as an
  audit trail.
- **Target**: CloudWatch Logs (one log group per ECS service), metrics on
  query latency/audit strip-rate/quota headroom, alarms on 5xx + provider
  429s, X-Ray or OTEL traces across the query pipeline stages (§5.2 steps are
  natural span boundaries).

## 14. Testing & benchmarks

- **Unit/integration**: 70 Rust workspace tests (retrieval, audit incl. the
  polluted-citation case, cache scoping, vault crypto, workflows) — green.
- **Offline benches** (reproducible scripts + reports retained): reranker
  head-to-head (bge vs LLM judge vs mxbai vs gte), OCR head-to-head (Surya vs
  Tesseract vs Paddle — Paddle unbenchable, vendor crash, recorded), BM25
  fusion, parent-child dilution, cross-lingual cosine checks.
- **Golden ruleset**: admin-curated Q&A with expected terms + sources, scored
  against live answers (`POST /admin/golden/run`).
- **Hit-testing**: admin debugger shows vector/BM25/fused/reranked stages per
  query — the first tool when a user reports a bad answer.

## 15. Limitations & roadmap (summary)

**Limitation — provider quota only:** the production provider is
Cloudflare Workers AI's free tier, capped at 10,000 neurons/day (reset
00:00 UTC); the Paid plan (~$5/mo) or a provider move removes it
(ADR-0008, decision open). **Roadmap:** JSON persistence → managed
PostgreSQL on the AWS target (§12.5, ADR-0010/0016); first Docker image
build; retrieval pool-depth/fusion-weighting tuning (eval findings in
`~/workspace/eval-expansion`, measured 23 Oct 2026). Everything else
formerly listed here — sentence-level citations, jurisdiction/temporal
awareness, multi-tenant Spaces, autonomous agents — has shipped and is
described in §5/§8.

---

## Appendix A — Configuration reference

Server (C1) environment. Defaults in `crates/common` / `.env.example`.

**Core**: `LEGAL_RAG_ADDR` (bind), `LEGAL_RAG_PORT` (compose), `LEGAL_RAG_DATA_DIR`,
`LEGAL_RAG_FRONTEND_DIR`, `LEGAL_RAG_ADMIN_DIR`, `LEGAL_RAG_USE_MOCK_PROVIDERS`.
**Auth**: `LEGAL_RAG_JWT_SECRET`, `LEGAL_RAG_JWT_ACCESS_TTL`, `LEGAL_RAG_JWT_REFRESH_TTL`,
`LEGAL_RAG_ADMIN_EMAILS`, `LEGAL_RAG_OIDC_ISSUER`, `LEGAL_RAG_OIDC_CLIENT_ID`,
`LEGAL_RAG_OIDC_CLIENT_SECRET`, `LEGAL_RAG_OIDC_REDIRECT_URI`.
**Providers**: `LEGAL_RAG_LLM_URL`, `LEGAL_RAG_LLM_API_KEY`, `LEGAL_RAG_LLM_MODEL`,
`LEGAL_RAG_EMBEDDING_URL`, `LEGAL_RAG_EMBEDDING_API_KEY`, `LEGAL_RAG_EMBEDDING_MODEL`,
`LEGAL_RAG_EMBEDDING_DIMS`.
**Retrieval**: `LEGAL_RAG_RERANKER` (`local`|`llm`), `LEGAL_RAG_RERANK_URL`,
`LEGAL_RAG_RERANK` (on/off), `LEGAL_RAG_RERANK_CANDIDATES`, `LEGAL_RAG_RERANK_POOL`,
`LEGAL_RAG_RERANK_TIMEOUT_MS`, `LEGAL_RAG_CACHE_ENABLED`, `LEGAL_RAG_SEMANTIC_CACHE`,
`LEGAL_RAG_SEMANTIC_THRESHOLD`, `LEGAL_RAG_ANNOTATION_THRESHOLD`,
`LEGAL_RAG_PARENT_CHILD`, `LEGAL_RAG_CHILD_CHARS`, `LEGAL_RAG_CHUNK_SIZE`,
`LEGAL_RAG_CHUNK_OVERLAP`.
**Vector store**: `SERENEDB_URL`, `SERENEDB_TABLE`.
**Ingest/OCR**: `LEGAL_RAG_S3_BUCKET`, `LEGAL_RAG_S3_REGION`, `LEGAL_RAG_S3_PREFIX`,
`RAG_ANYTHING_MAX_CONCURRENT_FILES`, `RAG_ANYTHING_ENABLE_IMAGE_PROCESSING`,
`RAG_ANYTHING_ENABLE_TABLE_PROCESSING`, `RAG_ANYTHING_ENABLE_EQUATION_PROCESSING`,
`RAG_ANYTHING_OCR_ENABLED`, `RAG_ANYTHING_OCR_MIN_TEXT_CHARS`,
`RAG_ANYTHING_OCR_DPI`, `RAG_ANYTHING_OCR_PRIMARY_CMD`.
**Web**: `LEGAL_RAG_WEB_SEARCH`, `LEGAL_RAG_WEB_SIDECAR`.
**Billing**: `LEGAL_RAG_PAYMENT_PROVIDER` (`mock`|`youpi`), `LEGAL_RAG_YOUPI_URL`,
`LEGAL_RAG_YOUPI_MERCHANT_ID`, `LEGAL_RAG_YOUPI_WEBHOOK_SECRET`.

## Appendix B — API inventory (65 routes, `/api/v1`)

- **auth**: register · login · refresh · me · oidc/login · oidc/callback
- **vault**: documents (create/list/delete) · search · ingest (S3)
- **knowledge**: query · query/stream (SSE) · research · workflows ·
  workflows/run · interact · draft · source-document · graph · stats ·
  cache-stats · documents (interact corpus)
- **matters**: briefs CRUD · documents attach/detach · timeline · memory
- **billing**: plans · me · checkout · payments · payments/:id · webhook/youpi
- **private-vault**: status · password (set) · unlock · lock · documents
- **settings**: public · **dashboard**: me
- **admin**: users (list/get/patch) · settings (get/put) · billing/overview ·
  billing/settings · insights · insights/users/:id · golden/rules (+from-activity) ·
  golden/run · annotations (+from-activity, toggle) · hit-test

## Appendix C — Port & process inventory

| Port | Process | Env |
|---|---|---|
| 3000 | legal-rag-server (prod instance) | production |
| 3001 | legal-rag-server (test instance, `data-test`, Gemini bridge LLM) | test only |
| 7890 | SereneDB | optional |
| 8081 | reranker sidecar | all |
| 8082 | Gemini bridge | test only |
| 8083 | local bge-m3 embeddings sidecar | test/local |

---

*Document control: v1.2 — authored 21 Oct 2026 by the Juris Vault AI Team (v1.1 adds §12.6, the shared-estimate alternative; v1.2 adds the AWS WAF line to §12.6, 23 Oct 2026)
(owner: Jishan). Update this document in the same change as any architecture
change, and add the corresponding ADR (or amend the existing one) — see the
changelog discipline in [CHANGELOG.md](../CHANGELOG.md).*
