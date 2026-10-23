# Juris Vault AI — Architecture Decision Records (ADR)

| | |
|---|---|
| **Document** | Full ADR set for the infrastructure and AWS platform teams |
| **Product** | Juris Vault AI — legal document intelligence for Indian law practice |
| **Owner** | **Jishan** — Juris Vault AI Team |
| **Version** | 1.5 — 23 October 2026 |
| **Companion** | [ARCHITECTURE.md](ARCHITECTURE.md) · [README.md](../README.md) · [CHANGELOG.md](../CHANGELOG.md) |

## How to read this document

Each record follows the same shape: **Status · Context · Decision ·
Consequences · Alternatives considered**. Statuses:

- **Accepted** — decided and implemented (marked LIVE-VERIFIED where proven).
- **Accepted with open action** — decided; a named follow-up remains.
- **Proposed** — designed in full, awaiting the owner's (Jishan's) decision.
  Infra/AWS teams should build foundations that keep Proposed records cheap
  to adopt, but must not treat them as approved spend.

Records are immutable once accepted: a changed decision gets a **new** ADR
that supersedes the old one (the old record stays, marked Superseded).

## Index

| ADR | Title | Status |
|---|---|---|
| 0001 | Rust monorepo: five-crate workspace, single-binary server | Accepted |
| 0002 | RAG architecture: faithful RAG-Anything port + LightRAG query modes | Accepted |
| 0003 | Embeddings: bge-m3 fixed; improve via a decision layer, not a swap | Accepted |
| 0004 | Reranker: bge-reranker-v2-m3 local sidecar as the decision layer | Accepted |
| 0005 | Hybrid retrieval: vector + BM25 with RRF fusion | Accepted |
| 0006 | Vector store: in-memory default, SereneDB persistent backend | Accepted |
| 0007 | OCR: Surya-only pipeline | Accepted |
| 0008 | LLM provider: Cloudflare Workers AI today; provider abstracted | Accepted with open action |
| 0009 | Parent-child chunking for match/context separation | Accepted |
| 0010 | Persistence: atomic JSON stores now; PostgreSQL migration path | Accepted |
| 0011 | Auth: Argon2 + JWT, role-based admin, optional OIDC | Accepted |
| 0012 | Confidential Vault: Argon2id → AES-256-GCM, memory-only key | Accepted |
| 0013 | Billing: token metering; mock + YouPI-PG payments | Accepted with open action |
| 0014 | Web research: self-hosted sidecars, primary-source-first, citation audit | Accepted |
| 0015 | Deployment today: single host / Docker; sandbox relay is dev-only | Accepted with open action |
| 0016 | AWS target architecture: ECS Fargate + RDS + ap-south-1 | **Proposed** |
| 0017 | Data residency & compliance posture: India-first | Accepted |
| 0018 | Adopt functionality, never identity, from reference products | Accepted |

---

## ADR-0001 — Rust monorepo: five-crate workspace, single-binary server

**Status:** Accepted (LIVE-VERIFIED)

**Context.** The system needs a low-latency streaming API, heavy in-process
retrieval state, and simple deployment. The founding brief specified a Rust
implementation end to end (auth through serving).

**Decision.** One Cargo workspace (resolver 2, edition 2021) with five crates
— `common` (config/errors), `auth`, `ingest` (S3), `rag` (intelligence core),
`server` (Axum API + static SPA hosting) — compiled to one release binary,
`legal-rag-server`. The server serves both frontends from disk, so frontend
deploys never restart the API. Sidecars (rerank/OCR/web) are separate
processes by design (ADR-0004, 0007, 0014), not crates.

**Consequences.** (+) One artifact to ship, version, and roll back; shared
types across API and retrieval; predictable memory profile. (−) Rust build
times; a bug in any route module ships in the same binary (mitigated by the
70-test workspace suite and crate boundaries).

**Alternatives considered.** Node/TypeScript service (rejected: the brief
mandated Rust, and the retrieval core benefits from Rust's concurrency and
memory control); microservices per surface (rejected: operational weight far
beyond the team's current scale).

---

## ADR-0002 — RAG architecture: faithful RAG-Anything port + LightRAG modes

**Status:** Accepted (LIVE-VERIFIED)

**Context.** Rather than inventing a retrieval architecture, the build ports
two proven open-source designs: **RAG-Anything** (document pipeline: parse →
typed content blocks → modal processors → index) and **LightRAG** (six query
modes — `naive`, `local`, `global`, `hybrid`, `mix`, `bypass` — over a
knowledge graph + vector hybrid).

**Decision.** Port both faithfully in the `rag` crate: same config shape
(`RagAnythingConfig`), same pipeline stages, same mode semantics. Product-,
India-, and provenance-specific layers (parent-child chunking, ADR-0009;
citation audit, ADR-0014; Brief scoping) are added **on top of** the port,
never by distorting it.

**Consequences.** (+) Behaviour is comparable against upstream references;
upstream improvements remain portable. (+) Modes give the admin a real
retrieval dial (default exposed to users is `mix`; the picker itself is
hidden from end users — a product decision recorded in the changelog).
(−) Port fidelity constrains some local optimizations.

**Alternatives considered.** LangChain/LlamaIndex in Python (rejected with
ADR-0001); building only on a hosted RAG API (rejected: no control over
chunking, provenance, or Indic language behaviour).

---

## ADR-0003 — Embeddings: bge-m3 fixed; improve via a decision layer

**Status:** Accepted (LIVE-VERIFIED)

**Context.** Retrieval quality for Indian legal text is dominated by two
needs: Hindi↔English cross-lingual matching and precise ranking among
near-duplicate statutory chunks. The owner directed (20 Oct 2026): keep
**bge-m3** as the embedding model; add a "decision model" layer instead of
swapping embeddings.

**Decision.** Embeddings are `BAAI/bge-m3`, 1024-dim, cosine space, for all
ingest and query paths (provider-hosted today; identical model in the local
sidecar, ADR-0008/0016). Quality investment goes into the reranking decision
layer (ADR-0004). Measured cross-lingual quality: Hindi query ↔ English
document cosine 0.89 (hosted) / 0.919 (local sidecar).

**Consequences.** (+) Stable vector space — no corpus re-embedding on model
fashion changes; the local sidecar can replace hosted embeddings with zero
drift (verified: local vectors retrieve correctly against the hosted-embedded
corpus). (−) We forgo potential gains from newer embedding models until a
head-to-head justifies a full re-index.

**Alternatives considered.** Swapping to a newer multilingual embedding
model (deferred by owner direction; requires full re-embed + re-bench);
OpenAI embeddings (rejected: weaker Indic cross-lingual evidence, per-call
cost, data leaves India).

---

## ADR-0004 — Reranker: bge-reranker-v2-m3 local sidecar as decision layer

**Status:** Accepted (LIVE-VERIFIED)

**Context.** ADR-0003's decision layer needed a decider. Candidates were
benchmarked offline on a fixed legal query set (report retained).

**Decision.** `BAAI/bge-reranker-v2-m3` runs as a local HTTP sidecar (:8081),
scoring the fused pool (cap 12 candidates, 500-char excerpts, 12s timeout,
decisive-margin skip gates; vector order is the automatic fallback).
Benchmarks: **MRR 0.944** vs 0.815 unranked baseline; the LLM listwise judge
tied on quality at per-call cost, so it remains an option
(`LEGAL_RAG_RERANKER=llm`), not the default. Round-2 challengers (closed
21 Oct 2026): mxbai-rerank-base-v2 MRR 0.889 — does not dethrone;
gte-multilingual-reranker-base unbenchable (crashes in vendor modeling code).

**Consequences.** (+) Best measured quality at zero marginal cost; no quota
exposure. (−) A GPU-less host pays ~9.5s per 10-candidate pool (2-vCPU
sandbox) — the AWS sizing for this component is an open benchmark item in
ADR-0016. **License note:** Jina reranker weights were rejected (CC BY-NC —
commercial trap); bge-reranker-v2-m3 is MIT.

**Alternatives considered.** LLM-as-judge default (kept as option); Cohere
Rerank API (hosted cost + data egress; not benchmarked better).

---

## ADR-0005 — Hybrid retrieval: vector + BM25 with RRF fusion

**Status:** Accepted (LIVE-VERIFIED)

**Context.** Pure vector retrieval misses exact statutory terms (section
numbers, defined terms). A BM25 leg was added and fused with Reciprocal Rank
Fusion (k=60).

**Decision.** Retrieval pool = RRF fusion of the vector leg (wide, top_k×4)
and a BM25 leg (k1=1.5, b=0.75, non-negative IDF) over chunk texts, with the
graph/mode leg per ADR-0002; the three legs run concurrently
(`tokio::join!`). Keyword-only hits receive a rank-imputed cosine so all
downstream margin gates stay on one scale. Offline bench: fused **MRR 0.833**
vs 0.815 vector-only, recall@3 1.0, no query regressed. The BM25 index
rebuilds lazily on corpus-fingerprint change; `LEGAL_RAG_BM25=0` disables.

**Consequences.** (+) Exact-term recall (section numbers!) without losing
semantic recall. (−) One more in-memory index to keep fingerprint-fresh.

**Alternatives considered.** Elasticsearch/OpenSearch sidecar (rejected at
current scale: operational weight; revisit if corpus passes ~1M chunks).

---

## ADR-0006 — Vector store: in-memory default, SereneDB persistent backend

**Status:** Accepted (**LIVE-VERIFIED in production since 23 Oct 2026** —
SereneDB 26.10.3 runs the production corpus; the gated live test initially
failed on the real server's distance-column type and passes with a
`float8` cast on `<=>` results, and the snapshot restore/save path was
fixed to honor whichever store is active. SereneDB ships Docker-only, so
the sandbox runs the official image's statically-linked `serened`
natively. The remaining open question is narrowed to the AWS target:
SereneDB (self-hosted) vs RDS pgvector — see the decisions register.)

**Context.** Development needs zero-dependency startup; production needs
durable, indexed vectors. **SereneDB** (serenedb.com, Apache 2.0) speaks the
Postgres wire protocol and supports native vector columns, and was the
owner's pick over the in-memory default.

**Decision.** A `VectorStore` trait with two backends. Default: in-memory
brute-force cosine (corpus restored from JSON snapshots). With `SERENEDB_URL`
set: SereneDB — `FLOAT[N]` columns, `<=>` cosine operator, IVF index
(`USING inverted (embedding ivf (metric='cosine'))`), Postgres wire v3 on
:7890; unreachable at boot → automatic fallback to in-memory with a warning.

**Consequences.** (+) Same code path in dev and prod; swap is one env var.
(−) SereneDB is a young project (Berlin, 2025) — the AWS record (ADR-0016)
keeps RDS **pgvector** as the documented fallback if SereneDB fails its
production evaluation. (−) SereneDB ships Docker-only; no standalone binary.

**Alternatives considered.** pgvector on RDS (fallback, above); Pinecone/
Qdrant hosted (rejected for residency and cost at this stage); keeping
vectors in Postgres JSON (rejected: no index, no scale path).

---

## ADR-0007 — OCR: Surya-only pipeline

**Status:** Accepted (LIVE-VERIFIED)

**Context.** Indian legal corpora are full of scanned paper — court notices,
gazette prints, Hindi documents. OCR quality decides whether those documents
exist for retrieval at all.

**Decision.** The PDF pipeline is: text layer first (fast path) → rendered
pages (`pdftoppm`) → **Surya 0.22 as the sole OCR engine**, via a local
sidecar (llama.cpp inference backend). A head-to-head benchmark (Surya vs
Tesseract 5 on English + Hindi + ruled tables) showed Surya tying or winning
everywhere and winning decisively on Hindi and tables; per the owner's
direction Tesseract was removed entirely (20 Oct 2026), and the Docling
fallback was removed after it. Operational gotchas (silent empty output when
the llama.cpp backend fails to spawn; results-path nesting) are documented
in the team's runbook notes.

**Consequences.** (+) One OCR behaviour to tune, verify, and support; best
measured Indic quality. (−) Surya is slow on CPU (~20 min for a dense page
on 2 vCPU) — **production OCR requires a GPU host**; this is a hard input to
ADR-0016 sizing. (−) PaddleOCR was evaluated as a challenger and is
unbenchable in our environment (crashes inside Paddle's own executor) —
recorded as an infrastructure incompatibility, not a quality verdict.

**Alternatives considered.** Tesseract (removed, above); cloud OCR APIs
(Textract/Google Vision — per-page cost, data egress, weaker Devanagari
evidence in our tests).

---

## ADR-0008 — LLM provider: Cloudflare Workers AI today; provider abstracted

**Status:** Accepted with open action (LIVE-VERIFIED; **owner decision
pending** on the production provider)

**Context.** Generation, graph extraction, research adjudication, drafting,
and workflows all need an LLM. All LLM/embedding calls go through
OpenAI-compatible HTTP clients (`LEGAL_RAG_LLM_*`, `LEGAL_RAG_EMBEDDING_*`),
so the provider is configuration, not code.

**Decision.** Today: **Cloudflare Workers AI** — LLM
`@cf/meta/llama-3.3-70b-instruct-fp8-fast`, embeddings `@cf/baai/bge-m3`.
Model selection is admin-changeable and hot-applied (`DynamicLlm`); a
Gemini bridge (anonymous gemini-web2api → Gemini Flash) powers only the
isolated test instance. A local bge-m3 embeddings sidecar already removes
embeddings from the provider entirely where deployed.

**The open action (owner: Jishan).** The Cloudflare **free tier allows
10,000 neurons/day** (reset 00:00 UTC); exhaustion presents as embedding 429s
and LLM error 4006. Production must pick one: (a) Cloudflare Workers **Paid**
(~$5/mo + usage), (b) **AWS Bedrock** models in-region where available, or
(c) self-hosted models on the GPU node from ADR-0016. Heavy verification
testing burned a full day's free quota on 21 Oct 2026 — treat free-tier
quota as a demo resource only.

**Consequences.** (+) Provider portability is real (three providers already
exercised: Cloudflare, Gemini bridge, mock). (−) Until the decision lands,
production capacity planning has an unknown.

**Alternatives considered.** OpenAI direct (works via the same client;
weaker India-residency story); locking to any single provider in code
(rejected outright).

---

## ADR-0009 — Parent-child chunking for match/context separation

**Status:** Accepted (LIVE-VERIFIED)

**Context.** Long statutory sections chunk poorly: match on a whole section
and precision drowns; match on fragments and the model lacks context — the
classic failure that Dify's parent-child design addresses (adopted as an
idea, ADR-0018).

**Decision.** Parents = document sections; each parent is split into
~220-char sentence-aware children (Devanagari danda respected;
`LEGAL_RAG_CHILD_CHARS`, disable with `LEGAL_RAG_PARENT_CHILD=0`). Children
are the match units (embeddings, BM25); after reranking, matches expand to
their parents for generation; citations point at parents; sibling matches
collapse. Bench: diluted corpus MRR **0.833 (child) vs 0.639 (parent-only)**.
Live-verified on the test instance (buried clause retrieved via child,
answered from parent).

**Consequences.** (+) Precision and context at once. (−) ~4–5× record count
vs parent-only indexing; legacy documents gain children only on re-ingest.

**Alternatives considered.** Fixed-size single-level chunking (the bench
above is the rejection evidence); semantic chunking via LLM (cost/latency,
deferred).

---

## ADR-0010 — Persistence: atomic JSON stores now; PostgreSQL migration path

**Status:** Accepted (LIVE-VERIFIED for JSON; migration is TARGET)

**Context.** The system launched in a sandbox with no managed database.
State had to be durable, inspectable, and restorable across restarts.

**Decision.** All state is JSON files under `LEGAL_RAG_DATA_DIR`, written
atomically (temp + rename): users, documents/chunks/graph, matters, billing,
activity (≤5,000 entries), annotations, golden rules, settings, snapshot
corpus (every 5 min + on SIGTERM/SIGINT, restored on boot). The data layer is
kept behind repository-style modules so the AWS migration (ADR-0016) swaps
storage, not callers: **target = RDS PostgreSQL** (schemas map 1:1 from the
§7.1 inventory in ARCHITECTURE.md), vectors per ADR-0006.

**Consequences.** (+) Zero-dependency durability today; every store is
human-inspectable during incidents. (−) Single-writer assumption — horizontal
app scaling is blocked until the RDS migration lands; file stores cap
practical corpus/tenant growth. Both are accepted for launch scope and are
the first migration items in ADR-0016 §12.5.

**Alternatives considered.** SQLite (rejected: no better scaling story, worse
inspectability for JSON-shaped state); going straight to Postgres in the
sandbox (no managed instance available; would have slowed the build phase).

---

## ADR-0011 — Auth: Argon2 + JWT, role-based admin, optional OIDC

**Status:** Accepted (LIVE-VERIFIED)

**Context.** The reference product (Harvey) gates everything behind an
Auth0 login wall. The product needs the same posture with a self-hosted
default and an enterprise SSO path.

**Decision.** Email/password with **Argon2id** hashes; **JWT access +
refresh** pairs (TTLs via `LEGAL_RAG_JWT_*_TTL`, secret via
`LEGAL_RAG_JWT_SECRET`); authorization by `User.role` with an `AdminUser`
extractor (403 for non-admins) and `LEGAL_RAG_ADMIN_EMAILS` promotion at
login/register; an **Auth0-compatible OIDC** client (login/callback) ships
in the `auth` crate for tenants who bring their own IdP — enabling a real
Auth0 tenant is a go-live configuration task, not code.

**Consequences.** (+) No external dependency for the default path; SSO is a
config flip. (−) Password auth means we own credential hygiene (Argon2id,
rate-limiting at the edge in target topology).

**Alternatives considered.** OIDC-only (rejected: blocks small-firm
onboarding); session cookies (rejected: the API is consumed by two SPAs and
future integrations; JWT fits).

---

## ADR-0012 — Confidential Vault: Argon2id → AES-256-GCM, memory-only key

**Status:** Accepted (LIVE-VERIFIED)

**Context.** Firms will store genuinely confidential client documents. A
"private" flag in a shared corpus is not confidentiality.

**Decision.** Per-user vault password → **Argon2id** KDF → **AES-256-GCM**.
Document titles and chunk texts are stored **only as ciphertext**
(`private_vault.json`) with SHA-256 fingerprints; the key lives only in
server memory while unlocked; **there is no recovery by design**. Private
ingest never enters the shared corpus, snapshots, or knowledge graph; query
with `private: true` embeds the question, searches only the owner's chunks,
decrypts the top 3 in memory, and cites them as `[private:N]`; a locked
vault returns an explicit 400. Activity logs redact private content.
Verified: the owner retrieves the fact with citation; another user retrieves
nothing; on-disk grep finds zero plaintext.

**Honest limits (accepted, disclosed in-product).** While unlocked, decrypted
context is sent to the LLM to answer; embeddings are computed by the
embedding provider at upload; embedding vectors are stored beside the
ciphertext. Client-side encryption (zero-knowledge) is roadmap, not claimed.

**Consequences.** (+) Real at-rest confidentiality with a small, auditable
crypto surface. (−) Forgotten vault password = unrecoverable data (by
design; the UI must keep saying so).

**Alternatives considered.** Corpus-level row security (rejected: plaintext
at rest); per-document passwords (rejected: unusable UX, same crypto).

---

## ADR-0013 — Billing: token metering; mock + YouPI-PG payments

**Status:** Accepted with open action (LIVE-VERIFIED with mock + webhook
flow; **compliance decision pending before real money**)

**Context.** Pricing research placed the product in the ₹1k–9k/month research
band that incumbents leave open. Billing must meter what actually costs money
— provider tokens.

**Decision.** Token-metered plans on 30-day periods: **Free 50k tokens ·
Basic ₹2,999 / 2M · Premium ₹9,999 / 10M**, no seat minimums. Metering uses
real provider usage across query/stream/research/workflows/uploads; semantic-
cache and verified-answer hits bill 0; over-quota requests fail with HTTP
402 before generation. Checkout providers: `mock` (testing) and **YouPI-PG**
(self-hosted UPI gateway, 0% fees, HMAC-SHA256-verified webhook).

**Open action.** YouPI-PG is self-described experimental/research software.
Before real payments on AWS: compliance review + bank reconciliation
process, **or** swap to a mainstream gateway (Razorpay/Stripe) behind the
same checkout/webhook seam. Owner decision (Jishan), infra to implement.

**Consequences.** (+) Pricing maps to cost; the meter is provider-agnostic.
(−) UPI-only today (cards/netbanking arrive with the mainstream gateway).

**Alternatives considered.** Seat-based pricing (rejected: misaligned with
usage cost and the target band); Stripe-only (international-first; UPI is
the Indian default rail).

---

## ADR-0014 — Web research: self-hosted sidecars, primary-source-first, citation audit

**Status:** Accepted (LIVE-VERIFIED)

**Context.** Legal answers that cite news write-ups of a judgment instead of
the judgment are a credibility failure — observed directly in a competitor's
output (a captured answer cited an unrelated Karnataka case for the central
holding of the electoral-bonds judgment). Web blending therefore ships with
three guardrails or not at all.

**Decision.**
1. **Self-hosted search/fetch sidecars** (Python, `tools/web`): DDG-HTML +
   Bing-HTML over plain HTTP (~3s), merged, canonical-URL deduped; stealth
   Firefox fallback when result-starved (DDG anomaly-blocks datacenter IPs
   after bursts — observed HTTP 202s); fetch is HTTP-first with **pypdf
   extraction for judgment PDFs**, meta-tag dates, browser fallback.
2. **Primary-source-first**: legal-authority tiers (T1: sci.gov.in,
   indiankanoon.org, ecourts, gazettes, indiacode; T2: LiveLaw, Bar & Bench,
   quality press) + explicit `site:` probes for judgment/statute queries;
   tier order floats primary authorities to the top of the model's context.
   Two-wave fetching protects the top 3 with a dedicated 45s window.
3. **Citation-integrity audit** (`rag/src/audit.rs`): every
   `[doc|web|private:N]` marker in a generated answer is checked against the
   exact excerpt the model was shown; <50% key-term support or an unknown
   source → the marker is stripped; the client receives the receipt
   (production observation: 12 checked / 9 verified / 3 stripped). Verdict-
   first answer format is enforced when web sources are blended.
   Supporting UX: the **source viewer modal** opens the full source document
   with the cited passage highlighted (Ask, Draft, Research, Agents).

**Consequences.** (+) Web answers cite the actual judgment (verified: the
electoral-bonds question returns the SC judgment PDF from api.sci.gov.in as
source #1). (+) Unsupported citations never reach the user. (−) Web Ask is
slower (~36s) than vault Ask (~5s); some publishers (LiveLaw) defeat both
fetch paths — tolerated per-source.

**Alternatives considered.** Tavily/Serper APIs (rejected: cost, egress, and
no tier control); LLM-parametric "web knowledge" without fetching (rejected
outright — unverifiable).

---

## ADR-0015 — Deployment today: single host / Docker; sandbox relay is dev-only

**Status:** Accepted with open action (LIVE-VERIFIED for the sandbox runtime;
Docker path BUILT, first build pending)

**Context.** Development and demos run in a sandbox VM with no Docker and no
public ingress. A portable deployment artifact was still required for
handover.

**Decision.** Canonical runtime = the release binary + sidecars on one host
(or the Docker image). The sandbox demo is exposed via a Cloudflare Worker
relay + local agent (`tools/tunnel`) — **explicitly a development
convenience, never a production component** (it disappears in ADR-0016).
The Docker path: 4-stage `Dockerfile` (→ `python:3.12-slim`, poppler,
non-root, `/data` volume, healthcheck), `docker-compose.yml` (app + optional
`serenedb` profile), `deploy.sh`, `DEPLOY.md`.

**Open action.** First `docker build` on a real Docker host + smoke test
(pending since 20 Oct 2026 — the sandbox has no Docker). Infra team: treat
this as go-live task #1.

**Consequences.** (+) Handover is one command once Docker exists. (−) Until
the first build, the image is validated (YAML/bash) but unproven.

**Alternatives considered.** Kubernetes for the dev phase (rejected:
disproportionate); bare-metal-only docs (rejected: compose is the portable
unit the AWS migration also starts from).

---

## ADR-0016 — AWS target architecture (PROPOSED)

**Status:** **Proposed** — awaiting owner (Jishan) approval. Addressed to the
AWS team: this is the build spec to estimate and stage, not yet approved
spend.

**Context.** Production needs: an SLA, GPU capacity for OCR (ADR-0007),
managed state (ADR-0010), secrets management, India data residency
(ADR-0017), and an end to sandbox restarts and provider free-tier walls
(ADR-0008).

**Decision (proposed).** Region **ap-south-1 (Mumbai)**, two AZs.

- **Ingress**: Route 53 → CloudFront (SPA static; `/api` bypass) → ALB
  (HTTPS via ACM, health check `GET /health`).
- **Compute**: ECS Fargate services — `app` ×2 tasks (2 vCPU/4 GB),
  `rerank` ×1, `web` ×1, `embed` ×1 (local bge-m3, ADR-0003); **OCR on ECS/EC2
  GPU (g5) behind an SQS queue**, autoscale 0–2. GPU for rerank/embed is a
  benchmark decision at staging (CPU numbers in ARCHITECTURE §10).
- **Data**: RDS PostgreSQL 16 Multi-AZ (db.t4g.medium at launch) for app
  state per ADR-0010; **SereneDB on EC2 (private subnet)** for vectors per
  ADR-0006, with pgvector as the documented fallback; S3 (versioned, SSE-KMS)
  for documents and snapshots; the existing S3 ingest source is unchanged.
- **Config/secrets**: Secrets Manager → ECS task environment, **same variable
  names** as ARCHITECTURE Appendix A. No `.env` files in production.
- **Images/CI**: ECR + pipeline (cargo release build + both Vite builds → one
  image per ADR-0015); blue/green via ECS deployment circuit breaker.
- **Observability**: CloudWatch Logs per service, alarms on 5xx and provider
  429 rates, dashboards for query latency + audit strip-rate + quota
  headroom (or provider spend under the ADR-0008 decision).
- **Backup/DR (launch tier)**: RDS automated backups + S3 versioning;
  RPO 24h / RTO 4h; cross-region copy deferred until the first enterprise
  contract requires it.
- **Relay agent (C10) is deleted**, not migrated.

**Migration order** (from ARCHITECTURE §12.5): Docker first build → RDS
schemas from the JSON stores → staging in ap-south-1 with the local
embeddings sidecar (kills the free-tier wall) → GPU OCR benchmark →
provider decision (ADR-0008) → production cutover → YouPI decision
(ADR-0013) before real payments.

**Alternative under verification (added 21 Oct 2026, owner direction).** An
AWS Pricing Calculator estimate prepared and shared with the AWS team on
**19 Oct 2026** — EC2-centric (m7i-flex.4xlarge API host, 2× Spot
g4dn.xlarge GPU ingest workers, dedicated t3a.xlarge SereneDB host,
Single-AZ db.t4g.small RDS, 16 TB S3 corpus, ALB) at **$2,058.78/month
($24,705.36 / 12 months)** — $2,015.78/month as shared, plus AWS WAF
($43.00/month: Web ACL on the ALB, AWS Managed Core/Known Bad Inputs/IP
Reputation/SQL Database rule sets, Bot Control Common, rate-based rule,
20M requests/mo planning figure) added 23 Oct 2026 at the owner's
direction — all in Asia Pacific (Mumbai) — is recorded in
full in ARCHITECTURE §12.6 as a deployment alternative to this record's
Fargate topology. The AWS team is asked to verify it (egress and corpus-size
assumptions, Single-AZ vs Multi-AZ posture, Spot interruption handling,
unpriced gaps) **whether or not it is adopted**; adoption remains a separate
owner decision alongside this ADR. Estimate link:
https://calculator.aws/#/estimate?id=50a088f3e134c1dcb06525fc8e6b32c956e71512
Updated estimate with WAF (23 Oct 2026):
https://calculator.aws/#/estimate?id=85ab95370800b927cdd6e0264e90946f86481e2d
— its WAF line shows $24.00/mo because the calculator cannot model Bot
Control's subscription and per-request fees; §12.6 keeps the
published-pricing $43.00/mo as canonical (owner decision, 23 Oct 2026).

**Consequences (if accepted).** (+) Managed, restart-proof, region-resident
production with a cost profile dominated by one GPU node + Fargate tasks.
(−) Estimated launch-tier run cost is materially above zero (Fargate + RDS
Multi-AZ + GPU hours); OCR queue adds ingest latency by design (async).

**Alternatives considered.** EKS (rejected at launch scale: platform weight);
single EC2 host with compose (kept as the small-deployment tier, ADR-0015 —
not the production target); Lambda for the API (rejected: long-lived
retrieval state, SSE streaming, and sidecar locality fit containers better);
us-east-1 (rejected: residency, ADR-0017).

---

## ADR-0017 — Data residency & compliance posture: India-first

**Status:** Accepted

**Context.** The corpus is Indian legal material, much of it confidential
client data, sold to Indian firms. Residency and disclosure are product
features, not legal footnotes.

**Decision.** Production data at rest lives in **ap-south-1** (ADR-0016);
documents are encrypted in S3 (SSE-KMS) and the Private Vault is encrypted
client-grade regardless of platform (ADR-0012). **Disclosed exception**:
LLM/embedding provider calls may leave the region depending on the ADR-0008
outcome; the DPA/customer terms must name the provider and region story
honestly, and the local-embeddings sidecar (ADR-0003) exists precisely to
shrink that exception. Access is role-gated (ADR-0011); activity history
provides the audit trail; private content is redacted from logs.

**Consequences.** (+) A residency story we can state in one sentence to a
firm's IT committee. (−) Some best-in-class models are not available
in-region; the provider decision (ADR-0008) must weigh that openly.

**Alternatives considered.** Global-by-default deployment (rejected);
on-prem-only for enterprise (deferred — the Docker tier, ADR-0015, is the
seed of that offer).

---

## ADR-0018 — Adopt functionality, never identity, from reference products

**Status:** Accepted (standing product rule, owner-directed 20 Oct 2026)

**Context.** The product was researched against Harvey (design/IA) and
Lexlegis (authenticated feature set), and evaluated against Dify (platform).
The owner's rule ("Purple Cow"): clone information-architecture
completeness, never identity.

**Decision.** REA-derived features are implemented **only after the owner's
approval, with a reasoned recommendation**; naming, branding, and copy are
always Juris Vault AI's own (the assistant persona is **Clause**; the matter
workspace is a **Brief / Living Brief**; the competitor's assistant name is
never used). Where a platform's license blocks adoption (Dify's modified
Apache 2.0 forbids multi-tenant SaaS operation without a commercial
license), we mine ideas and build natively — parent-child chunking
(ADR-0009), annotated replies (Verified Answers), and the hit-testing panel
all came from that evaluation with zero code reuse.

**Consequences.** (+) A defensible product identity and clean IP posture.
(−) Every adopted idea costs a native build — accepted as the price of the
moat (Indic OCR, cross-lingual retrieval, Living Brief memory, provenance
receipts).

**Alternatives considered.** White-labelling or forking Dify (license-
blocked); pixel-cloning competitor UX (identity risk, rejected by rule).

---

## ADR-0019 — Fusion suite, rank-interleave leg merging, retrieval cache

**Status:** Accepted (23 Oct 2026; measured same day)

**Context.** A landscape evaluation of Spice.ai, Haystack, LlamaIndex,
txtai, RAGFlow, Hyper-RAG and Verba converged on rank-fusion maturity as
the transferable pattern (four of seven ship weighted/normalized fusion
over plain RRF). Per ADR-0018 the patterns were rewritten natively in
Rust — no sidecar runtime. The A/B programme then produced a more
valuable finding than the tuning itself: per-stage retrieval on the
20-rule golden corpus is saturated (recall 1.000 at k=5 and k=10 in
every configuration), while end-to-end failures traced to **Mix mode
merging its vector/local/global legs by raw score across
incommensurable scales** (cosines ~0.4–0.6 vs the graph legs' flat
0.85), which flooded the rerank pool with graph-neighborhood chunks and
truncated the vector leg's best hits before generation.

**Decision.** (1) Legs merge by **rank interleave** (round-robin in
each leg's own order, dedup by chunk id). (2) The vector↔BM25 fusion is
a suite — RRF / weighted RRF / DBSF — with an **in-fusion statute
term** (a query naming an Act lifts that Act's chunks ×1.35 by default,
aliases matching abbreviated titles) and an optional recency-decay
term. (3) A **retrieval-tier cache** with stale-while-revalidate,
keyed by question + mode + scope + corpus fingerprint. (4) Production
defaults stay conservative: RRF + statute term; weighted RRF and the
post-fusion metadata boost are opt-in (measured stage-neutral to
harmful — the identifier boost feeds the Act-blind BM25 leg).

**Measured consequences.** Golden set, test instance, 23 Oct 2026:
12/20 (pre-suite best) → 11/20 (suite, pre-merge-fix control) →
**15/20** (merge fix in); all remaining failures are answer-wording
only, with zero source-attribution misses.

**Alternatives considered.** Adopting Spice.ai as a sidecar (rejected —
separate runtime, Alpha vector engines, ADR-0018); post-fusion
metadata boosting as the statute mechanism (rejected by measurement —
it cannot rescue a document the fusion cut already dropped).

---

## Open decisions register (for the owner)

| # | Decision | ADR | Needed from |
|---|---|---|---|
| 1 | Production LLM/embedding provider: Cloudflare Paid vs Bedrock vs self-hosted | 0008 | Jishan |
| 2 | Approve the AWS target architecture + launch-tier sizing | 0016 | Jishan |
| 3 | Payments for real money: YouPI-PG compliance review vs mainstream gateway | 0013 | Jishan |
| 4 | AWS-target vector store: SereneDB self-hosted vs RDS pgvector (production sandbox evaluation is DONE — SereneDB live since 23 Oct 2026; this is now only the AWS-target choice) | 0006/0016 | Jishan / AWS team |
| 5 | First Docker build + smoke test on a Docker host | 0015 | Infra team |
| 6 | Verify the shared 19 Oct 2026 AWS estimate ($2,058.78/mo incl. the WAF line added 23 Oct 2026, EC2-centric, 16 TB corpus) against ADR-0016 — see ARCHITECTURE §12.6 | 0016 | AWS team |

*Register discipline: when a decision lands, the corresponding ADR's status
changes in the same commit, the change is logged in
[CHANGELOG.md](../CHANGELOG.md), and this register is updated.*

---

*Document control: ADR set v1.2, authored 21 Oct 2026 by the Juris Vault AI
Team (owner: Jishan). New decisions get new records; accepted records are
never edited except to mark them Superseded.*
