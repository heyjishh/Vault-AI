# Changelog — Juris Vault AI

All notable changes to Juris Vault AI are recorded here, newest first.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versioning follows the workspace version in `Cargo.toml`.

**Owner:** Jishan — Juris Vault AI Team
**Maintained by:** Juris Vault AI Team. Every shipped change lands here in the
same commit/session as the change itself — features, fixes, benchmark results,
and verification status included. Entries state plainly what is **verified
live**, what is **built but unverified**, and what is **roadmap**.

---

## [Unreleased]

### Added
- **Tier-2 retrieval arc — augmentation, hyperedges, HyDE, late
  interaction, Tantivy, MCP** (24 Oct 2026; ADR-0021). Directed by the
  owner as the complete remaining take-list from the RAG landscape
  evaluation, with frontend integration.
  (a) **Index-time chunk augmentation** (RAGFlow pattern): one batched
  LLM call per document at ingest generates keywords (incl. Hindi
  equivalents) + questions per chunk; embedded as auxiliary match
  units (`pc=aug`) that retrieval canonicalizes back to the source
  chunk — the keyword leg also indexes aux text under the source id.
  Kill switch `LEGAL_RAG_AUGMENT=0`. Vault gains an "Indexing" view
  per document showing the generated keywords/questions.
  (b) **Statute hyperedges** (Hyper-RAG pattern): one retrievable unit
  per (Act, section) per document binding its member chunks; a hit on
  the unit canonicalizes to the first member and pulls siblings into
  context. Kill switch `LEGAL_RAG_HYPEREDGES=0`.
  (c) **HyDE / multi-query expansion**: short (≤8 words) or
  citation-style questions trigger one LLM expansion call (hypothetical
  answer passage + up to 3 variants); each variant gets its own search
  pool, merged by rank interleave. Expansions ride the answer/stream
  (`expansions` field; Ask shows "also searched as" chips). Kill
  switch `LEGAL_RAG_HYDE=0`.
  (d) **ColBERT late interaction** (Spice ships it; txtai pattern): a
  MaxSim sidecar (`tools/colbert/colbert_sidecar.py`, colbertv2.0,
  MIT) behind `LEGAL_RAG_COLBERT_URL`; scores blend (normalized,
  `LEGAL_RAG_COLBERT_WEIGHT`, default 0.5) with the cross-encoder
  after reranking, failures degrade to the reranker order. New
  hit-test stage `late-interaction` when active. **Bench verdict
  (24 Oct, `~/workspace/colbert-bench/REPORT.md`): DEFAULT OFF on
  evidence.** On full-passage candidates every scorer saturates
  (MRR 1.0); on production-realistic short chunks MaxSim is the
  weakest standalone scorer (MRR 0.709 vs cosine 1.0, cross-encoder
  0.913) — it saturates on shared legal vocabulary and misses both
  section-collision queries it was meant to fix, and blending dilutes
  the stronger scorers at 50/50. The stage ships as an experimental
  opt-in (a low weight may still earn its place on a larger golden
  set; unproven either way).
  (e) **Tantivy BM25 engine** behind `LEGAL_RAG_BM25_ENGINE=tantivy`
  (native in-house engine stays the default), same build/search
  contract. Bench (`bm25_engine_bench`, 24 Oct): at 324 chunks native
  builds in 4.6 ms / queries 105 µs vs Tantivy 133 ms / 468 µs; at
  50,000 chunks native 329 ms / 4.3 ms vs Tantivy 4.6 s / 72.7 ms —
  native is faster at both scales on this box, and the engines' top-10
  overlap is only ≈0.54 (ranking is NOT equivalent; treat Tantivy as
  an alternative scorer, not a drop-in). Native stays default.
  (f) **MCP server**: `POST /mcp` (streamable-HTTP style, JSON-RPC
  2.0, protocol 2024-11-05) with tools `search_vault`,
  `list_documents`, `ask`; auth is the caller's normal bearer token
  (quota enforced on `ask`). Admin Command Center shows live
  retrieval-stack status (new `GET /api/v1/admin/retrieval-status`),
  the MCP panel with a copyable client config, and the new hit-test
  stage. Kill switch `LEGAL_RAG_MCP=0`.
  Verification status: **live-verified on the test instance**
  (24 Oct): 112 workspace tests + both frontend builds green;
  augmentation, hyperedges (after a cross-pairing bug found in
  verification was fixed via nearest-Act pairing), HyDE expansions,
  MCP round-trips, retrieval-status and the late-interaction
  hit-test stage all exercised end-to-end (receipts in ADR-0021).
  Production (:3000) runs the build with default gates (ColBERT off,
  per the bench verdict). S3 Vectors live round-trip remains gated
  on AWS credentials (unchanged from ADR-0020).
- **Spice.ai vector engines, taken natively** (23 Oct 2026; ADR-0020).
  Spice's "vector engine" is a lineup, not a library, so each engine
  was implemented behind the existing `VectorStore` trait and selected
  with `LEGAL_RAG_VECTOR_STORE` (`memory` | `serenedb` | `hnsw` |
  `s3vectors`; unset keeps the legacy SERENEDB_URL behavior, so
  production is unchanged). (a) **`hnsw`** — the DuckDB-VSS engine
  design (in-process HNSW, Spice's own parameter defaults M=16 /
  ef_construction=128 / ef_search=64) as a pure-Rust store with JSON
  persistence and unlink-on-delete; exact brute-force on small
  graphs. (b) **`s3vectors`** — Spice's external engine, AWS S3
  Vectors: a hand-rolled SigV4 JSON client (no AWS SDK dependency),
  `doc_id` as filterable metadata (the pushdown pattern), bucket +
  index auto-created on connect, available in ap-south-1 (the
  ADR-0016 target region). **Benched** (20,000 × 1024-dim clustered
  vectors, 100 queries, `examples/vector_bench.rs`): brute force
  131 ms mean / recall 1.000; SereneDB IVF 67 ms / 1.000; HNSW
  ef_search=64 → 3.1 ms / 0.899, ef_search=128 → 6.4 ms / 0.981 —
  HNSW is 10–40× faster per query at a recall price, with a slow
  first build (186 s at 20k in this implementation). Production
  default stays SereneDB (exact at current corpus scale); HNSW is
  the embedded/Docker-tier engine, S3 Vectors the AWS-target engine.
  **Verification**: HNSW live-tested (unit + bench + persistence
  round-trip); S3 Vectors is docs-verified + unit-tested (SigV4
  matches AWS's published signing vector) with the live round-trip
  gated behind real AWS credentials (`S3VECTORS_LIVE_TEST=1`) — the
  sandbox holds none, stated plainly rather than implied.

### Added
- **Retrieval fusion suite + leg-merge fix + retrieval cache** (23 Oct
  2026, **verified live**; Spice.ai-grade search patterns rewritten
  natively in Rust — no sidecar runtime). (a) *Fusion suite*: the
  vector↔BM25 fusion is now selectable — plain RRF, weighted RRF
  (per-leg weights, tunable k, identifier boost), or DBSF distribution
  normalization — plus two in-fusion terms: a **statute term** (when a
  query names an Act, that Act's chunks are lifted inside fusion before
  the cut; aliases match abbreviated titles like "Section 86 IT Act")
  and an optional **recency-decay** term. (b) *Leg-merge fix*: Mix
  mode's vector/local/global legs merged by raw score, but the legs'
  scales are incommensurable (cosines ~0.4–0.6 vs the graph legs' flat
  0.85), so graph-neighborhood chunks flooded the rerank pool and
  truncated the vector leg's best hits — the Section 86 question's
  rank-1 answer never reached the context and the model refused. Legs
  now merge by **rank interleave**. (c) *Retrieval cache* (Spice's
  results-caching pattern): fresh window + stale-while-revalidate,
  keyed by question + mode + scope + corpus fingerprint. (d) Autocut
  score-gap cutoff after reranking and lost-in-the-middle prompt
  ordering (both env-gated). (e) Admin per-stage golden metrics
  (`POST /admin/golden/stages`). **Measured** on the 20-rule golden set
  (test instance): stage-level retrieval is saturated (recall 1.000 at
  k=5/k=10 in every configuration — fusion variants are stage-neutral);
  end-to-end, the leg-merge fix moved the golden score **12/20 →
  15/20** (control run without it, same day: 11/20), flipping Section
  86, Evidence Act s.101, CPC s.11 (res judicata) and the Puttaswamy
  three-fold test to PASS; all remaining failures are answer-wording
  only, zero source-attribution misses. Production defaults: RRF +
  statute term 1.35; weighted RRF measured stage-neutral-to-harmful
  (its identifier boost feeds the Act-blind BM25 leg) and ships opt-in.

### Changed
- **Rename completed + limitations corrected** (23 Oct 2026, **verified
  live**) — every remaining legacy pre-rename product string removed
  repo-wide:
  login/admin wordmarks, error-boundary button, the server's frontend
  fallback page, browser storage keys (`jurisvault.access` /
  `jurisvault.refresh` / `jurisvault.history` — existing sessions sign in
  once more), npm package names, Dockerfile/compose/deploy.sh/DEPLOY.md
  wording, and the stale scaffold `frontend/README.md` rewritten. Docs
  v1.4: README's limitations list and ARCHITECTURE §15 now state the
  single standing limitation — the Cloudflare Workers AI free-tier cap of
  10,000 neurons/day — and §15 no longer lists shipped features
  (sentence-level citations, jurisdiction scoping, Spaces, autonomous
  agents) as limitations.

### Added
- **Sentence-level citations** (23 Oct 2026, **verified live**) — citations now
  carry a pinpoint sentence: the exact sentence in the cited passage that
  supports the claim, chosen by distinct query-term coverage over flowing
  (de-wrapped) sentence segmentation, English and Hindi (danda) aware; a
  candidate must be a complete sentence (terminal punctuation, 40–700 chars)
  or no pinpoint is emitted. Citation cards display the pinpoint; the source
  viewer highlights that exact sentence in the full document. Unit tests:
  English, Hindi, hard-wrapped statute lines, no-match. Live check on
  production returned complete-sentence pinpoints from the corpus.
- **Jurisdiction & time awareness** (23 Oct 2026, **verified live**) — documents
  carry legal-validity metadata (jurisdiction, effective-from/to,
  superseded-by), settable at upload or via
  `PATCH /api/v1/vault/documents/{id}/metadata`. Ask gained an "As of"
  control (date + jurisdiction): matching filters are a **hard boundary**
  (no full-vault fallback, unlike brief scoping); documents not yet in force
  or out of jurisdiction are excluded, and superseded sources are flagged
  with an amber "Superseded" chip on their citations. Verified live:
  `as_of` before a document's effective date excludes it, a foreign
  jurisdiction excludes marked documents, superseded flags round-trip.
- **Multi-tenant Spaces** (23 Oct 2026, **verified live, 17/17 checks**) —
  Spaces group briefs and members with owner/editor/viewer roles
  (`crates/server/src/spaces.rs`, persisted to `data/spaces.json`). Briefs
  optionally belong to a space; access is enforced across brief lists,
  detail, scoped queries, vault document lists and the source viewer —
  non-members get 404 (existence is confidential), viewers read but cannot
  upload or edit (403). Admins bypass membership. Frontend: Spaces panel on
  the Briefs page (create, invite by email, roles, remove, delete), space
  filter chips, space picker on brief creation. Verification ran three real
  accounts on production; fixtures were removed afterwards.
- **Autonomous agent orchestration** (23 Oct 2026, **verified live**) — a
  planner–executor loop (`crates/rag/src/orchestrator.rs`): given a goal,
  the model plans one step at a time — search the vault (brief-scoped,
  space-checked) or search the web — up to a step budget (1–10, default 6),
  then synthesizes a verdict-first, cited report from its findings only.
  `POST /api/v1/knowledge/agents/autonomous`, billed like a workflow run;
  the Agents page gained an "Autonomous research" card with goal, brief
  scope, step budget and a step trace. Live run: 3 planned steps, 9
  citations, finished within budget. Honest limit: the planner is the same
  single LLM (no separate sub-agent models); quality inherits the main
  provider.
- **Confidential Vault local routing** (23 Oct 2026, **verified live** on the
  test instance) — `LEGAL_RAG_PRIVATE_EMBEDDING_URL` / `LEGAL_RAG_PRIVATE_LLM_URL`
  (with key/model/dims companions) route vault uploads, private question
  embeddings and private answer generation to dedicated — typically local —
  endpoints, so confidential text need never reach the main providers.
  Private clients are never wrapped in the shared semantic cache. The vault
  status API and page now state where private traffic goes ("local
  endpoint" vs "main provider"). Verified both directions: with local
  endpoints configured, a sealed document round-tripped (upload → private
  question → cited answer); with a deliberately invalid private-LLM model,
  private queries failed at the private endpoint while normal queries
  still succeeded. Production currently runs with routing unset (provider),
  and the vault page says so plainly. Caveat: switching the private
  embedder to a different model/dimension requires re-uploading sealed
  documents.
- **SereneDB is now the production vector store** (23 Oct 2026, **verified
  live**) — SereneDB 26.10.3 (official image, statically-linked `serened`
  run natively; no Docker in this sandbox) on :7890, production launched
  with `SERENEDB_URL`. The gated store test initially failed against the
  real server (distance column decode) — fixed by casting `<=>` results to
  `float8`; the full upsert/search/get/remove contract then passed.
  **Bug found and fixed by the switch**: snapshot restore/save only knew
  the in-memory store, so an externally-backed store booted empty;
  `restore_snapshots` now upserts into the active store and the periodic
  saver dumps the active store. Verified at the table level: the shared
  `legal_chunks` table holds exactly the production corpus (3 documents,
  10 chunks) and production answers cite it. Open decision ADR-0006
  (SereneDB vs pgvector for the AWS target) now has live evidence.
- **Web leg hardening** (23 Oct 2026, measured; report
  `~/workspace/web-leg/REPORT.md`) — search gained cross-process DDG
  pacing with jitter, retry/backoff on 202/429/503, and Brave as a third
  always-on backend (Mojeek and Startpage rejected empirically). Fetch
  gained a LiveLaw chain (canonical → AMP → Firefox → reader proxy);
  a test article went from FAIL/26.6s to OK, 5,895 chars, 2.7s. Measured
  under active DDG throttling: 8/8 searches returned 10 results with zero
  Firefox rescues (baseline: 2), worst-case latency 39.6s → 23.6s.
  Residuals, honestly: DDG still blocks under bursts (tolerated, not
  prevented); Indian Kanoon serves a Cloudflare challenge page; India
  Code bitstream URLs stall from this network.
- **Evaluation corpus expansion** (23 Oct 2026, test instance only; report
  `~/workspace/eval-expansion/REPORT.md`) — corpus grew 4 → 16 documents
  (324 chunks) with 12 public-domain Indian legal texts (Constitution
  articles, Contract/IPC/Evidence/CPC excerpts, Kesavananda, Puttaswamy,
  Maneka Gandhi); golden ruleset grew 1 → 20 rules. **Measured result:
  10/20** — and the baseline Section 86 rule regressed under dilution
  (missed at top_k=5 in the larger corpus). Classification of the 10
  failures: 5 were substantively correct answers failed by brittle
  exact-phrase expected terms, 4 were genuine retrieval misses (including
  a CPC s.11 vs Contract Act s.11 section-number collision), 2 answered
  correctly from an overlapping document. Hindi cross-lingual probes:
  1/5 hit at this corpus size (all misses refused honestly in Hindi).
  Follow-ups identified, not yet done: evaluator term matching
  (case/morphology-tolerant), dilution mitigation (higher default top_k
  or per-document caps).

### Changed
- **Documentation set v1.3** (23 Oct 2026) — README, `docs/ARCHITECTURE.md`
  and `docs/ADR.md` synced to the day's landed work: the README's
  limitations list no longer names sentence citations, jurisdiction/time
  awareness, multi-tenant Spaces or autonomous orchestration (all live);
  ARCHITECTURE records Spaces enforcement, legal-validity scoping,
  private-vault routing, SereneDB as the production store, and the
  autonomous agent; ADR-0006 is marked LIVE-VERIFIED in production with
  its open question narrowed to the AWS-target store choice. PDF
  regenerated (v1.3).
- **Golden evaluator + retrieval-depth experiments** (23 Oct 2026, measured
  on the test instance; production defaults unchanged) — the golden
  evaluator's term check is now substance-based instead of exact-substring:
  punctuation-normalized, order-insensitive token containment (stopwords
  dropped). Morphology is deliberately NOT bridged ("arose" still does not
  match "arising"). Two env-gated retrieval knobs were added for the
  dilution investigation: `LEGAL_RAG_DEFAULT_TOP_K` (request default when
  `top_k` is unspecified, default 5) and `LEGAL_RAG_PER_DOC_CAP` (max
  chunks per document in the answer context, default off). Three full
  20-rule runs on the expanded corpus, all with zero infrastructure
  failures: baseline evaluator/top_k 5 = **10/20**; evaluator fix alone =
  **12/20** (Contract s.73 + s.74 flipped); top_k 9 = **11/20**;
  top_k 9 + per-doc cap 3 = **11/20**. Conclusion, measured: neither
  pool depth nor per-doc caps rescue the persistent misses (Section 86
  source attribution, Kesavananda, Puttaswamy, the CPC s.11 vs Contract
  Act s.11 collision) — the target documents lose earlier, in fusion /
  the rerank pool, not at the context cutoff. ±1 rule between runs is
  live-model answer variance (Contract s.74 and Article 14 each flipped
  once on wording alone). The test instance was returned to the
  best-measured configuration (defaults); the knobs remain available.
  Next lever, identified but not yet built: rerank-pool depth and
  fusion weighting. Run data: `~/workspace/eval-expansion/golden-run-*.json`.

### Added
- **Source viewer modal on all pages** (21 Oct 2026) — the Ask page's full-source
  modal (complete document text, cited passage highlighted, source provenance)
  was extracted to a shared component (`frontend/src/components/SourceModal.tsx`)
  and rolled out to Draft, Research, and Agents: shared citation cards open it,
  markdown answers linkify `[doc:N]`/`[web:N]` markers into clickable chips, and
  Research evidence labels resolve to sources. Browser-verified PASS on all
  four surfaces (Draft card, Research card, Agents card + report chip, Ask chip
  regression).
- **Documentation set v1.0** (21 Oct 2026) — `docs/ARCHITECTURE.md` (full system
  architecture + AWS target topology), `docs/ADR.md` (ADR-0001…0018),
  rewritten `README.md`, and this changelog. Owner attribution: Jishan /
  Juris Vault AI Team.

### Added (documentation)
- **AWS WAF added to the shared estimate** (23 Oct 2026, owner direction) —
  the §12.6 estimate priced no WAF; line 14 now adds a Web ACL on the ALB
  (AWS Managed Core, Known Bad Inputs, Amazon IP Reputation, SQL Database
  rule sets, Bot Control Common, rate-based rule for the auth endpoints,
  20M requests/mo planning figure) at **$43.00/mo**, taking the estimate to
  **$2,058.78/mo · $24,705.36/12 mo**. Recorded in `docs/ARCHITECTURE.md`
  §12.6 and ADR-0016. Docs version bumped to 1.2; PDF regenerated. An
  updated shareable calculator estimate with the WAF line was also produced
  (link in §12.6); its WAF line shows $24.00/mo because the calculator
  cannot model Bot Control's subscription/request charges — the doc set
  keeps $43.00 as canonical (owner decision, 23 Oct 2026).
- **Shared AWS estimate recorded as a deployment alternative** (21 Oct 2026) —
  the AWS Pricing Calculator estimate shared with the AWS team on 19 Oct 2026
  ("My Estimate", Asia Pacific (Mumbai), **$2,015.78/mo · $24,189.36/12 mo**,
  EC2-centric: m7i-flex.4xlarge API · 2× Spot g4dn.xlarge GPU ingest ·
  t3a.xlarge SereneDB · Single-AZ db.t4g.small · 16 TB S3 corpus) is now
  documented in full in `docs/ARCHITECTURE.md` §12.6 (all 13 line items,
  verified to sum to the stated total) and referenced from ADR-0016 as an
  alternative under AWS-team verification — recorded whether or not it is
  adopted. Docs version bumped to 1.1; PDF regenerated.

### Fixed
- **Briefs listing page crash** (22 Oct 2026, JISHAN's report) — the Briefs
  page died instantly with "Cannot access '_' before initialization":
  `MattersPage` computed its search filter from `briefQuery` before that
  state was declared (use-before-declaration, introduced with the Oct 21
  search + pagination change). Backend and brief detail view were unaffected.
  Fixed by moving the declaration above the filter; audited the other
  paginated pages (Vault, Dashboard, Plans — all correctly ordered).
  Browser-verified PASS after rebuild: listing loads, search filters,
  General opens.
- **Chunk-id resolution in the source-document endpoint** (21 Oct 2026) —
  `Citation.chunk_id` is a vector-record UUID, not a document id; the endpoint
  now resolves chunk → record → document, which also repaired the older
  "Open in Interact" handoff that had silently passed the UUID.

---

## [0.1.0] — 2026-10-21

The first assembled, live-verified system. Built 19–21 Oct 2026: Rust backend
(RAG-Anything architecture port), React user app + admin Command Center,
billing, briefs, confidential vault, web research, and the full retrieval
stack. 70 workspace tests green; complete product sweep verified live.

### Added — product surfaces
- **Ask** with real SSE streaming (Status → Sources → Deltas → Done), live
  markdown, Stop, elapsed timer, citation cards before text, blocking fallback.
- **Source viewer modal** (Ask first) — full source document in-app with the
  cited passage highlighted (whitespace-normalized passage location with
  best-overlap fallback), served by `POST /api/v1/knowledge/source-document`
  with a per-URL fetch cache (SCI judgment PDF: 10.5s cold / 0.0s cached).
- **Clickable, legitimate web sources** — primary-source probes
  (`site:sci.gov.in`, `site:indiankanoon.org`, `site:indiacode.nic.in`,
  `site:legislative.gov.in`), authority tiers, two-wave fetching. Verified:
  electoral-bonds question returns the actual SC judgment PDF as source #1.
- **Briefs / Living Brief** — matter workspaces with hard-scoped retrieval
  (verified: no cross-brief leakage; grounded refusal on empty scope).
- **Interact** (summary/chronology/clauses/risks/chat per document) and
  **Draft** (notice, agreement, IRAC memo, petition, letter).
- **Research mode** — hypotheses → evidence → adjudication → verdict-first
  report, with per-hypothesis evidence and IN FAVOUR/AGAINST/OPEN verdicts.
- **Agents** — contract review (5 steps), due diligence (4), IRAC memo (4).
- **Confidential Vault** — Argon2id → AES-256-GCM encrypted documents,
  ciphertext-only at rest, in-memory key while unlocked, `[private:N]`
  citations, activity-log redaction. Verified: owner reads, other users get
  nothing, on-disk grep finds zero plaintext.
- **Clause, the assistant persona** — named AI presence with a living avatar,
  plus **Clause the glass pet**: a draggable floating companion (SVG
  expressions, blink loop, sleepy idle, page-aware greetings, sidebar toggle,
  persisted position, minimal size slider 60–184px). Faceless crystal body —
  the expression overlay is the only face (double-face fix, 21 Oct).
- **Admin Command Center** (`/admin`) — user management, corpus/vault,
  retrieval & models, billing, insights (14-day charts, plan mix, top
  questions), golden ruleset evaluation, hit-testing debugger, Verified
  Answers management.
- **Admin customization** — per-surface prompt editors (Ask, Interact, Draft,
  Research frame/adjudicate, Agents) prefilled with the real built-in prompts,
  default query mode, model selection, brand, custom sidebar tabs. Verified
  live (Hindi prompt override → Devanagari answer; bad model → 502 → revert).
- **Billing & subscriptions** — Free 50k · Basic ₹2,999/2M · Premium ₹9,999/10M
  tokens per 30-day period; real provider-usage metering; quota → HTTP 402;
  mock payments + YouPI-PG UPI (HMAC-SHA256 webhook). Verified end to end.
  *Caveat: YouPI-PG is self-described experimental software — compliance
  review required before real money (ADR-0013).*
- **Pagination + date filters** across Dashboard history, Vault, Briefs, and
  payment history. **History restore** everywhere (Ask threads with sources
  sidebar, drafts, research reports, workflow runs, interact sessions).
- **Docker deployment** — 4-stage Dockerfile, compose (+ optional SereneDB
  profile), `deploy.sh`, `DEPLOY.md`. *Authored and validated; first build on
  a Docker host still pending.*

### Added — retrieval & intelligence
- **Parent-child chunking** — ~220-char children matched, parent sections
  answered from; children in all retrieval stages, citations point at parents.
  Bench: dilution MRR 0.833 (child) vs 0.639 (parent-only).
- **BM25 hybrid leg + RRF fusion** — fused MRR 0.833 vs 0.815 vector-only,
  recall@3 1.0, no query regressed.
- **Parallel retrieval** — the three mode legs run concurrently (`tokio::join!`).
- **Semantic answer cache** — cosine ≥0.97, scope-keyed, corpus-fingerprint
  invalidation; verified live (repeat question answered in 0.45s).
- **Verified Answers (annotations)** — human-curated replies served before the
  cache at cosine ≥0.95 under identical scope/fingerprint rails; verified with
  paraphrase match and zero LLM calls.
- **Citation-integrity audit** — every cited marker checked against the exact
  excerpt shown to the model; unsupported markers stripped, receipt exposed
  (prod: 12 checked / 9 verified / 3 stripped).
- **Verdict-first web answers** — Verdict / Analysis / Caveats structure when
  web sources are blended; web Ask latency 159s → ~36s (concurrent fetch
  window, best-passage extraction).
- **Web leg rebuild** — DDG+Bing HTML search over plain HTTP (~3s), stealth
  Firefox fallback, pypdf judgment extraction, legal-authority tiering.
- **Gemini test bridge + isolated test instance (:3001)** — anonymous
  gemini-web2api (no user cookie; advised against and accepted), copied data
  dir, golden run 1/1 PASS; local bge-m3 embeddings sidecar removed the last
  Cloudflare dependency from the test stack.

### Changed
- **Reranker decision layer**: bge-reranker-v2-m3 local sidecar is the default
  (MRR 0.944 vs 0.815 no-rerank; LLM listwise judge kept optional). Qwen3 is
  the upgrade path; Jina weights rejected (CC BY-NC). Round-2 benches closed:
  mxbai (0.889) does not dethrone it; gte-multilingual and PaddleOCR entries
  unbenchable due to vendor-code crashes — recorded, not retried.
- **OCR is Surya-only** — Tesseract removed after the head-to-head (Surya tied
  or won everywhere, decisive on Hindi + tables); Docling fallback then also
  removed at the owner's direction.
- **Embeddings stay bge-m3** (1024-dim); improvement comes from the decision
  layer, not an embedding swap.
- **Frontend rebuilt** from the basic scaffold to the Harvey-derived design
  system (ivory `#fafaf9`, ink `#0f0e0d`, warm greys) as two Vite apps;
  "Assistant" renamed **Ask**; corpus counts removed from the Dashboard in
  favour of a plan/usage card.
- **Server runs the release binary** (debug→release swap completed 21 Oct).
- **Mode picker removed** from Ask (admin-set default `mix`); users never see
  the six LightRAG modes.

### Fixed
- **Cloudflare quota failure mode diagnosed** — free tier 10,000 neurons/day
  (reset 05:30 IST) surfaces as embedding 429s / LLM code 4006 and ~28s of
  heartbeats; streams now emit 1.6s heartbeat statuses (never a frozen line),
  429s surface a friendly retry message, and rerank discipline caps pool=12
  with 500-char excerpts and a 12s timeout.
- **Pet double-face** — baked-in face + SVG overlay rendered two faces; body
  regenerated faceless, cache-busted asset.
- **Sidebar source handoffs** — web cards now open the real document; private
  cards route to the Private Vault; doc cards resolve the true document id.

### Evaluated, not adopted
- **Dify** — license forbids multi-tenant SaaS operation without a commercial
  license; ideas mined instead (parent-child chunking, annotated replies,
  hit-testing — all three built natively). Zero code reuse.
- **Lexlegis MIRA IA** — functionality adopted, identity never (Purple Cow);
  their captured answer format showed polluted citations (an unrelated
  Karnataka case cited for the central holding) — the gap our citation audit
  closes.

---

## [0.0.1] — 2026-10-19/20 (pre-release build history)

- REA of harvey.ai (design system + IA) and lexlegis.ai (authenticated app);
  rea repo (morluto/rea v6.7.0) set up as the REA toolchain.
- Rust workspace created (common/auth/ingest/rag/server); RAG-Anything port
  with LightRAG's six query modes; SereneDB vector backend (docs-verified,
  live test gated on Docker availability).
- Auth (Argon2 + JWT + OIDC client), S3 ingest, first end-to-end query path,
  Hindi scanned-PDF OCR verification.

---

*Changelog discipline: entries are added when work ships, with verification
status stated. Roadmap items live in docs/ADR.md (Proposed records) — not
here. Nothing is recorded as done before it is verified.*
