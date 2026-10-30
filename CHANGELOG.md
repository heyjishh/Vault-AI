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
- **Universal upload intake — multi-file and folder pickers on
  every surface (30 Oct 2026)**: the user's directive was
  explicit — multi-file AND folder selection everywhere (through
  a Brief, direct, Vault, Private Vault), not only where earlier
  pieces happened to land them. Two gaps remained after
  Piece 18: the Vault page had a multi-file picker but folder
  intake only via drag-and-drop traversal (no folder picker),
  and the Confidential Vault's seal flow was still single-file.
  Now: the Vault page gains an "Upload a folder" picker
  (`webkitdirectory`) beside the dropzone, wired through the
  shared intake (the same caps of 20 files / 10 folders per
  batch, batch labelling, skipped-with-reason overflow) into its
  per-file staged queue — drop behavior unchanged. The
  Confidential Vault's seal section gains "Choose files…"
  (`multiple`) and "Choose folder…" (`webkitdirectory`) pickers
  (accept filter unchanged) feeding ONE sequential seal queue:
  per item, Uploading (the file is read into memory) → Sealing
  (the per-document seal POST; the shared progress list gains an
  honest 'sealing' stage for it) → Ready only on the server's
  confirmation ("N chunks sealed"), or Failed with the server's
  actual error plus Retry/Dismiss. Pasted text joins the same
  queue as a one-item job. Title rule: batch files are titled by
  their own names; a lone file picked while the Title field is
  filled uses that title (the field's long-standing role).
  Locked gating is exact: the queue can only start from the seal
  section, which renders only while the vault is unlocked, and
  if the vault is locked mid-queue each remaining item fails
  inline with the server's locked error — never a silent stop.
  A final sweep re-verified that the admin frontend has no file
  inputs at all, so no single-pick upload surface remains.
  Frontend-only — no backend change; the shared frontend dist
  means prod serves the new pickers immediately. Verified:
  workspace tests 238 passed / 0 failed, frontend build clean,
  the served bundle carries `webkitdirectory` for all four
  folder pickers (Interact, Brief, Vault, Confidential Vault)
  and `multiple` on all four file pickers, and on the test
  instance two files sealed back-to-back through the exact
  per-file seal call both landed in the Confidential Vault's
  document list (an ordinary vault ingest also returns 200).
- **Multiple-file selection on the Brief and Vault pages
  (30 Oct 2026)**: the Brief page's and the Vault page's file
  pickers only ever took a single file (`files?.[0]`, no
  `multiple`) — Interact's pickers were the only multi-file intake
  (user report, 30 Oct). Now: the Brief page's "Upload files"
  input takes many files at once and gains an "Upload folder"
  picker (`webkitdirectory`) beside it, every selected file
  enqueueing into the panel's staged queue with its own name as
  its title; the Vault page's picker takes many files, its drop
  takes every dropped file (traversing dropped folders instead of
  only the first file), and its single-upload state is replaced by
  the same per-file staged queue. The batch intake helpers (caps
  of 20 files / 10 folders per batch, overflow kept as
  skipped-with-reason rows, batch labelling, drop flattening) now
  live in the shared upload module, so all three surfaces apply
  one implementation — no divergent copies. A lone Vault file with
  the Title field filled keeps using that title; otherwise files
  are titled by their own names. Frontend-only — no backend
  change; the shared frontend dist means prod serves the fix
  immediately. Verified: workspace tests 238 passed / 0 failed,
  frontend build clean, the built bundle carries `multiple` on
  all three pickers and `webkitdirectory` on both folder pickers,
  and two files ingested back-to-back into a probe brief on the
  test instance both filed with confirmed reports.
- **Staged upload/indexing progress on every ingest surface
  (29 Oct 2026)**: the Brief page's Add-documents panel no longer
  sits on a bare "Indexing…" button label for the whole ingest
  (user report, 29 Oct). Piece 10's staged progress — Uploading
  with byte progress → Indexing → Ready/Failed, per-item bars — now
  lives in one shared component (`UploadProgressList`, extracted
  from Interact, which renders through it too) and drives: the
  Brief page's file-upload and pasted-text flows (queued items,
  an elapsed timer while pending, Ready only on the server's
  ingest confirmation, failures inline with the server's error
  plus Retry/Dismiss), the Vault page's pasted-text flow (same
  staged row), the Vault dropzone (already staged; gained the
  elapsed readout and a persistent inline failure row with
  Retry/Dismiss), the Vault S3/R2 ingest button (elapsed readout;
  a bulk server-side pull has no client stages to show), and the
  Confidential Vault's Seal flow (staged row, Ready = sealed and
  confirmed). Frontend-only over the existing ingest endpoints —
  no backend change. The frontend dist is shared between prod and
  test, so prod shows the new panel immediately; it needs no
  backend deploy. Verified: workspace tests 238 passed / 0 failed,
  frontend build clean, the built bundle carries the shared
  progress UI, and a live ingest into a test brief on the test
  instance returned the confirmed report the UI keys "Ready" on
  (a headless visual check was not feasible in the sandbox — the
  only Chrome present is the harness-managed browser, which
  refuses direct launches — so the Brief panel relies on parity
  with the same shared component Interact already uses live).
- **Interact: Ask-style UI + Vault / Private / Both source selector
  (29 Oct 2026)**: Interact's chat surface adopts the Ask experience —
  a message thread (question headings, live status line with elapsed
  time, inline errors with Retry), an Ask-style composer (Enter to
  send, Refine control kept from Piece 13, terms folding as the
  focus line), and citation cards. A prominent **source selector**
  governs every run, enforced server-side by a `source` field on the
  Interact endpoint: **Vault** (default — the active brief's
  documents, unchanged behaviour), **Private** (ONLY the user's
  Confidential Vault, via Ask's existing private retrieval path —
  owner-only chunks decrypted in memory per request, `[private:N]`
  citations; the brief's documents are never loaded), and **Both**
  (brief documents + private retrieval in one answer, citations
  labelled `[doc:N]` / `[private:N]`). Interact never uses web or
  system data in any mode. Document actions (summary, chronology,
  clauses, risks) operate on vault documents: in Private mode they
  are refused with a plain message (UI disables them with the same
  note); in Both mode they apply to the vault side only. A locked
  vault in Private/Both is the private path's explicit 400, surfaced
  in the UI as an unlock state linking to the Private Vault page —
  never a silent fallback to Vault. **Privacy invariants**: runs
  that read the Confidential Vault generate through the dedicated
  private LLM when configured, else an undecorated client handle —
  never the shared `CachedLlm` — so private prompts/completions
  never enter the shared completion cache; their activity records
  are redacted exactly like Ask's private path (no output, no
  restorable payload); private text never enters the regular
  corpus. Streaming parity is partial by plumbing: Interact's
  endpoint is blocking, so the thread shows Ask's status
  presentation without token streaming. Upload flows and the
  document picker are unchanged (Vault/Both modes).
- **Stack health watch (29 Oct 2026)**: `health-check.sh` checks
  SereneDB, the four sidecars, prod, test, and the Cloudflare tunnel
  agent; a cron runs it every 10 minutes and, if anything is down,
  restores it via the idempotent `restore-stack.sh` and reports —
  silent while everything is healthy. First night: caught and
  restored a full stack outage after a sandbox replacement.
- **Refine prompt in every chat surface (29 Oct 2026)**: Ask,
  Interact's document chat, Research, Draft, and the Agents inputs
  (workflow + autonomous) gain one shared **Refine** control — the
  hybrid the user asked for ("refine them what our system understand
  better, or make it vectorize, or hybrid mix"). One click on
  `POST /api/v1/knowledge/refine` returns a **visible rewrite** plus
  **retrieval terms**: the rewrite replaces the input text in place
  (still editable, never auto-sent) with **"Refined — Undo"**
  restoring the byte-identical original, and a subtle "Searching
  also for: …" line shows the terms the send will carry. The
  refinement contract is enforced in the prompt: intent and every
  fact preserved exactly (names, dates, amounts, sections verbatim),
  no new facts or legal conclusions, the user's language kept
  (Hindi/Hinglish stays, lightly cleaned). On send, the terms ride
  as `refine_terms` and fold into the **existing** HyDE/multi-query
  expansion pool as one pooled query — Ask (blocking + streaming),
  Research (every hypothesis's retrieval), Draft's grounding query,
  and the agent engines' steps/researcher legs. Interact has no
  retrieval leg (the selected documents are the evidence), so there
  the terms fold in server-side as an explicit focus line in the
  chat instruction. Refine failure (provider down, failure page,
  unparseable reply) is an honest error and leaves the original
  untouched and sendable — the endpoint keys off Piece 14's
  failure-page contract. Empty prompts and prompts over 4,000
  characters are clean 400s. Identical inputs are served by the
  shared LLM response cache (no second cache); the call is metered
  through the standard usage scope and deliberately not written to
  activity history (the send that follows is). **Verified live on
  :3001**: a messy Hinglish royalty question refined faithfully
  (language and intent kept; terms incl. a Hindi equivalent), the
  send answered from the document (Rs 310/tonne) with the pooled
  terms query visible in the answer's expansions — confirmed on
  fresh scoped and unscoped asks (a send whose question matches an
  already-cached answer is served that answer, as with every
  expansion); Interact refine + send verified the same way; bridge
  stopped → refine returns an honest 502 with no fabricated rewrite.
  Workspace tests 219 → 226 for this piece.
- **Ask: "Brief vault" mode (29 Oct 2026)**: Ask gains a toggle beside
  Web / Private. Off (default) is exactly today's behavior — Ask draws
  on the whole corpus, preferring the active brief and widening to the
  full vault with a disclosed scope note when the brief has no hits.
  On, the corpus leg is **hard-scoped to the active brief's filed
  documents, server-enforced, with no fallback**: a question the brief
  cannot answer gets a grounded refusal whose scope note states the
  restriction ("Brief vault: no documents in <Brief> matched —
  nothing outside this brief was searched"), and every answer in the
  mode carries the note naming the brief. Web stays an independent
  toggle (web citations remain allowed alongside brief-only corpus
  citations); Private is unchanged. The mode is part of the semantic
  answer-cache key, so hard- and soft-scoped answers to the same
  question over the same brief never cross-serve (regression-tested
  both orders); streaming carries the note via the existing Status
  scope mechanism, including on verified/cache replays. The Ask
  banner reconciles the standing mode with the one-off "Search full
  vault instead" escape: while Brief vault is on, the escape reads
  "Turn off Brief vault & search full vault" and visibly switches the
  toggle off (with a toast) as it widens — it never silently defeats
  the mode. New `brief_vault` flag on `POST /knowledge/query` and
  `/knowledge/query/stream`; with no matter named, the mode binds to
  the default brief (a brief is always active after the brief-only
  vault change). **Verified live on :3001** (see the verification
  record in this changelog's arc notes): cross-brief question ON →
  refusal with the explicit note; OFF → answered via the disclosed
  fallback; in-brief question ON → answered, brief citations only.
  Workspace tests 214 → 219 for this piece.
- **Upload + indexing progress UI (29 Oct 2026)**: uploads now show
  real stage progress instead of a spinner. Per file: **Uploading**
  with a determinate byte-progress bar (the transport's upload
  progress events; indeterminate pulse when the transport cannot
  measure), then **Indexing** once the bytes are sent, and **Ready**
  only when the server's ingest response confirms the document is
  indexed — a file is never shown ready on send alone. The batch
  header adds an aggregate bar (files done / total with a ready
  count). The Vault page's dropzone carries the same
  Uploading → Indexing stages and bar via the shared upload client
  (`vaultApi.uploadFile` gained an optional progress callback; no
  new dependencies, existing ivory/ink styling).
- **Interact: brief-wide working set by default (29 Oct 2026)**: with
  a brief active, Interact's working set is now **all documents
  filed in that brief** — an explicit "All documents in this brief
  (N)" state in the picker, on both the brief page's Interact tab
  and the standalone view in brief scope. The set re-derives when
  the brief or its documents change (including uploads filed into
  the brief); any manual checkbox change narrows it into a manual
  selection, and the row re-enters brief-wide mode. If a brief
  holds more than the 20-document cap, Interact covers the first 20
  and the picker says so plainly ("N documents are not covered") —
  no silent truncation. Cross-document questions run across the
  brief set; single-document and manual-selection behaviour is
  unchanged. This also firms up the brief as the vault boundary in
  Interact: brief scope shows only the brief's own filed documents
  (the standalone full-vault listing remains available only through
  the explicit This brief / Full vault toggle — removed again by the
  brief-only vault change later in this cycle, see Changed).
- **Interact + upload caps raised (29 Oct 2026)**: the Interact
  multi-document cap rises 5 → **20 documents at a time**
  (server-enforced `MAX_INTERACT_DOCS`, same clean 400 over the cap;
  UI copy derives from the same number). The analysis budget now
  scales with the selection instead of splitting a fixed pot: a
  document keeps its full legacy 24,000-char share until the total
  would pass a **120,000-char ceiling**, which then splits evenly
  (20 documents → 6,000 chars each); per-document truncation flags
  stay exact, and single-document runs are byte-identical to before.
  Upload batches gain intake caps — **20 files and 10 folders per
  batch** — applied in arrival order across file picks, folder picks
  and drag-and-drop: items beyond a cap stay in the batch list as
  "Skipped — batch limit of N files/folders reached", never silently
  dropped. Auto-select after upload rises to the 20 cap with it.
  Server cap/budget tests updated to the new numbers (workspace
  tests unchanged at 202). Live verification on :3001 recorded with
  the arc's final entry below.
- **Verification — caps / brief-wide / progress arc (29 Oct 2026)**:
  verified live on :3001 (release build of this arc; workspace
  tests **202 green**, count unchanged). Interact with 21 documents
  → clean 400 "interact supports at most 20 documents at once";
  a 20-document chat over the General brief's own first 20 filed
  documents → 200 in 58s, 20 per-document entries with exact
  truncation flags, output attributed per document. The General
  brief files 30 documents, so the brief-wide picker state covers
  20 and states 10 not covered. The served bundle contains the
  brief-wide row, the batch-cap skip reasons ("batch limit of 20
  files / 10 folders reached") and the Uploading/Indexing progress
  labels. Upload through the shared ingest route re-verified
  (txt → 200 with doc_id; probe document deleted afterwards).
  Batch caps and progress bars are client-side UI, verified from
  the served bundle and code path; the ingest route itself is
  unchanged. Note: a sandbox VM replacement during verification
  killed the whole stack; the standard restore brought both
  instances back, so :3000 also runs this build (infrastructure
  recovery, not a separate deploy decision).
- **Interact: upload files and folders in place (29 Oct 2026)**: the
  Interact page gains Upload files / Upload folder controls plus
  drag-and-drop of files and folders (dropped folders are traversed
  recursively). Everything goes through the existing vault ingest
  route — same accepted types (.pdf incl. scanned/password, .txt,
  .md, .doc, .docx) and limits, no second pipeline — so uploads are
  normal vault documents (owned by the uploader, Brief-fileable;
  in brief scope they file into the active brief). The queue runs
  sequentially with per-file states (queued / indexing / ready /
  failed with the server's reason — one bad file never fails the
  batch) under a batch label ("Folder: Smith-case (12 files)").
  Each successful upload is auto-selected into the Interact
  selection up to the 5-document cap; beyond the cap the file is
  labelled "uploaded, not auto-selected" rather than dropped.
  Live-verified on :3001 in the UI's per-file pattern (PDF fixture
  + txt ingested and immediately interacted over as a set; an
  unsupported .xyz rejected with a clean per-file 400; a
  folder-shaped pair ingested). Pipeline test added for mixed-batch
  failure isolation. Workspace tests 201 → 202.
- **Admin: permanent delete of a user's uploaded documents + history
  (29 Oct 2026)** — Command Center → Users → "Delete data": shows the
  pre-delete counts, requires typing the user's email to confirm,
  and returns a deletion receipt. Scope is deliberately narrow and
  is the feature's contract. DELETED: (1) documents the user
  uploaded — uploads now stamp `uploaded_by` (first uploader wins on
  content-deduped re-uploads; legacy unowned documents are never
  attributed) — removed through the whole retrieval stack (record,
  chunks, vectors in SereneDB + in-memory, graph nodes/edges,
  citator treatments) and unfiled from every Brief holding them
  (this also fixed the single-document delete route, which unfiled
  only the first brief); (2) the user's history (activity entries).
  PRESERVED, always: the account (login keeps working), feedback,
  Private Vault (byte-identical), payment + usage/metering records,
  system data — including the `admin_purge` activity record the
  purge itself writes — briefs as records, and all other users'
  data. Idempotent (second run removes nothing). Live-verified on
  :3001 with a second user (2 docs, one filed in their brief, an
  Ask, a feedback vote, a Private Vault doc): receipt 2 documents /
  10 chunks / 2 filings / 4 history entries; post-purge the docs are
  unretrievable and uncitable, history is empty, login works, and
  feedback / private_vault / users files are byte-identical.
  Workspace tests 198 → 201.
- **Interact: multi-document selection + brief scoping (29 Oct 2026)**:
  the Interact picker is now a multi-select (checkboxes, selected
  count shown, cap of 5 stated in the UI and enforced by the API —
  the 24k-char analysis budget is split across the selection, so a
  single document keeps the full historical budget and prompt
  format). The API accepts `document_ids` (legacy `document_id`
  still works); the analysis prompt presents each document as a
  labelled section with an attribution instruction, so
  cross-document questions answer per document, and the response
  carries a per-document `documents[]` (title, truncation, citator
  treatment resolved per document, never merged). Empty selection →
  400 "no documents selected"; over-cap → 400. Also fixed: the
  standalone Interact view ignored the active brief (AppShell never
  passed it) and always listed the whole vault — it now receives the
  "Working in" brief like Ask/Draft, defaults the picker to the
  brief's filed documents, and offers a This brief / Full vault
  toggle (switching keeps only selected documents present in the new
  list; the toggle itself was removed by the brief-only vault change
  later in this cycle — see Changed). Live-verified on :3001: 2-document chat across the citator
  briefs (200, both documents covered), legacy single-doc summary
  (200), brief-scoped list returns only the brief's document vs 31
  in the full vault. Workspace tests 190 → 198.
- **Citator V2 — appellate chains, ladders, governing authority**
  (27 Oct 2026, standalone crate + integration, kill switch defaults to
  V1). New crate `~/workspace/citator-v2/` (own git repo, zero deps,
  deterministic only): Reversed kind with appellate-reversal detection
  (appeal allowed / impugned judgment set aside / reversed / quashed,
  linked by appeal number, CNR, party match incl. appeal swap, or
  impugned date); ladder registry as data tables (civil, criminal,
  tax ITAT + AAR→AAAR tracks, company, competition, securities,
  consumer, service, environment); jurisdiction model (chains never
  cross known-different states; SC binds all; HC binds its own state
  only); conclusion policy — never drop, latest-in-chain governs the
  case, highest binding forum governs across cases, persuasive
  authorities never govern, AAR/AAAR applicant-only, SLP dismissal
  without reasons is not affirmance. Wired into the RAG as a path
  dependency behind `LEGAL_RAG_CITATOR_V2=1` (default OFF — V1 behavior
  untouched when off); when on, V2 feeds Ask citation statuses
  (incl. Reversed/Persuasive), rank-down ordering, the verdict's
  governing-authority line + system hint, and treatment notes, in
  blocking and streaming paths. Standalone suite 44 green; workspace
  152 green (was 146). Verified on test :3001 only (see below);
  production untouched. NO version bump, NO push.
  - **Live verification on :3001 (27 Oct, V2 on):** fixture chain
    District → Allahabad HC → SC, Ask "current status of Kumar v.
    State": district Reversed (evidence: the HC reversal sentence),
    HC GoodLaw (evidence: "We affirm Kumar v. State."), governing
    line naming the SC with the full 3-step chain ("latest in
    chain"), treatment note present, dead district ranked last,
    nothing dropped; fixtures deleted afterwards. Round 1 showed HC
    Unknown — V1's name-based self-guard drops same-caption
    affirmances at ingest — fixed by snippet treatment detection +
    nearest-below resolution (adapter only, V1 untouched); round 2
    confirms the fix. `citator.json` held no V1 Kumar edge, so the
    HC status came purely from the snippet path.
  - **Adapter limits fixed (27 Oct, FIX 1–4, workspace 173 green,
    standalone still 44):** (1) Stored forum/state/date/applicant at
    ingest — deterministic inference from title + judgment header,
    explicit upload fields win on merge, admin meta endpoint, and a
    V2 backfill endpoint filling empty legacy fields (idempotent, no
    re-embedding, no LLM); the adapter prefers stored metadata, title
    inference is last-resort only. (2) Full-text V2 detection at
    ingest into a separate `V2EdgeStore` (never V1's store — zero V1
    regression risk), persisted as `citator_v2.json`, with a self-only
    guard so same-caption affirmances land on the step below and can
    never self-affirm; snippet detection is now fallback only.
    (3) Per-request `state` + `applicant` on the Ask API, threaded
    into governing selection (unknown state codes → None, never a
    crash); the semantic answer cache key includes the context.
    (4) Live multi-step verification on :3001 (V2 on): civil chain
    District → Allahabad HC → SC fully green — district Reversed,
    HC GoodLaw (the round-1 same-caption Unknown gap is closed for
    good), SC governs as latest in chain, reversed step ranked last,
    nothing dropped. Tax chain AAR → AAAR → HC → SC green on
    verdict narrative, SC governing, and no-drop, but its
    intermediate steps stayed Unknown — engine recall gaps, not
    adapter bugs (see Deferred). Backfill reproduced across a VM
    restart: first run {meta_updated 8, v2_edges_added 8, v2_total
    12}, second run {0, 0, 12}; the 8 added edges all came from
    legacy corpus docs, none from the new fixtures (ingest already
    covered those). NO version bump, NO push.
  - **Phase 2 — citator on Research, Draft, Interact, autonomous
    (27 Oct, workspace 177 green, standalone still 44):**
    Research citations now carry V2 treatments (stored edges +
    stored metadata) with live-first rank-down and a new
    `ResearchReport.governing_line` field (absent when nothing
    governs); `ResearchRequest` accepts optional `state` +
    `applicant`. Draft grounds through a real citator context
    (scoped + unscoped). Autonomous runs thread the context into
    every researcher leg/retry/re-query and append the governing
    line from the researchers' treated evidence to the final report
    (both construction paths; never when inconclusive); the legacy
    planner-executor path stays on default context. Interact takes
    no state/applicant fields (it never calls `query()` — they
    would be dead) and instead reports the document's own V2
    treatment in `InteractResponse`. Shared normalization in
    `routes::citator_context`; kill switch still defaults OFF, V1
    behavior untouched when off. Two adapter bugs found and fixed
    with regression tests: stored JudgmentId edges carry real ids
    that could never match positional citations (Phase 1's live
    Chain A had succeeded via the snippet fallback, masking it) —
    `resolve_v2` now takes a real→positional map and remaps targets
    + evidence; and duplicate judgments of one document tied in
    `resolve_below` as false ambiguity (both copies Unknown) —
    `resolve_v2` now dedupes by identity to a canonical judgment
    and expands treatments/rank back to copies. Live on :3001 (V2
    on): Ask re-verified (SC governs, HC GoodLaw, district Reversed
    last, nothing dropped; V2-off Ask re-verified clean);
    research shows HC GoodLaw + governing line; draft (state UP)
    shows HC GoodLaw + district Reversed last; interact on the
    overruled district doc returns Reversed with the HC passage.
    NOT live-proven, stated plainly: research rank-down with dead
    law present (no research run retrieved the district doc —
    retrieval outcome); autonomous positive append (both attempts
    degraded to `partial` with zero citations as every researcher
    leg timed out at 120s under sustained bridge load — the
    no-invention half is proven live twice). NO version bump,
    NO push.
  - **Deferred with reasons:** HTTP `good_law_only` parameter (engine
    flag exists and is tested; threading it through the query entry
    points is a bigger API change than this integration warrants);
    web verification + public corpus (as V1); engine detection-recall
    follow-up — name extraction truncates multi-party company names
    and misses "In re:" forms, and reversal outcomes miss
    writ-petition constructions ("writ petition is allowed … ruling
    set aside"), which is why the live tax chain's HC/AAAR/AAR steps
    stayed Unknown (honestly Unknown — never asserted without a
    passage).
- **Citator — "is this case still good law?"** (V1, 26 Oct 2026).
  Every cited authority in an Ask answer now carries a treatment status
  with provenance — Overruled / Followed / Distinguished / Applied /
  Good law / Unknown — on the citation card, and an overruled or
  distinguished authority additionally gets a deterministic
  treatment-note line in the answer. A citation is NEVER dropped for
  its status. The governing rule is absolute: a status is never
  asserted without a cited passage; with no evidence the status is
  Unknown, never a guess and never an assumed good law.
  - **Detection** (`crates/rag/src/citator.rs`): deterministic pattern
    matching over judgment text at ingest — no LLM call, no extra
    embedding. Covers `A v. B` names near a treatment verb (12-word
    window), Indian `(supra)` short forms (`ADM Jabalpur (supra)`,
    `M.P. Sharma (supra)`), verb-following phrases (`which followed
    ADM Jabalpur`), and the operative sentence-initial formula
    (`ADM Jabalpur must be and is accordingly overruled`). Negation
    (`do not follow`, `declined to overrule`, noun-ish `the
    overruling`) and evaluative language without a verb (`are
    seriously flawed`) create no edge. A document cannot treat
    itself (self-edge guard; backfill prunes such edges) — found
    live on :3001, where a fixture judgment describing its own
    holding had first surfaced as an `applied` self-edge. Kill switch
    `LEGAL_RAG_CITATOR=0`.
  - **Backfill — BUILT, not deferred**: `POST
    /api/v1/admin/citator/backfill` re-scans stored parent chunks only
    (no re-embedding, no LLM calls), idempotent; the existing corpus
    is small (69 parent/legacy chunks on :3001), so backfill is a
    seconds-long text scan, verified live (see below).
  - **Resolution at Ask time**: only treatment edges whose source
    document is visible in the current scope count (brief docs when
    brief-scoped, full vault otherwise; a brief→vault fallback widens
    evidence exactly as it widens the answer). Name matching is
    normalized token-subset with alias expansion (`ADM` = Additional
    District Magistrate) and merged initials (`M P` = `M.P.`).
    Precedence: Overruled > Distinguished > Good law (affirmed) >
    Followed > Applied. Web and private citations are always Unknown.
    Cache hits re-resolve statuses against the current edge set.
  - **Surface**: status badges + evidence passage on Ask citation
    cards; treatment-note line in blocking and streaming answers
    (appended after the citation audit, so the quoted passage is
    never mistaken for a marker). Fixed workflows untouched.
  - **Verified**: 17 new unit/integration tests incl. synthetic
    fixtures for every verb and every guard, a brief-boundary test
    (out-of-scope edge does not leak; in-scope edge does show), and
    a real-pair eval on genuine Puttaswamy judgment text (see
    Architecture §5). Live-verified on :3001: synthetic uploads,
    backfill (2 corpus edges + 2 fixture edges, then 4→3 after the
    self-edge prune), brief-boundary Ask (cited-only brief:
    Unknown, no note; both-docs brief: Overruled + note; unscoped:
    Overruled + note), streaming Ask (Sources event + Done note),
    and golden (below). Full suite 129 → 146 green.
  - **Golden**: 15/20 before, **16/20 after** on :3001 — no
    regression (floor 14/20). The one flip (Kesavananda Bharati)
    is live re-generation wording variance, NOT a citator effect:
    the baseline run was cache-served in 79 ms, while the after-run
    regenerated live after the fixture uploads changed the corpus
    fingerprint; the passing answer contains no treatment note.
    Remaining fails are the same wording-only fails. Known honest limits: adjective `overruling`
    uses are excluded by the `the`-guard but other noun-ish uses can
    still misfire; multi-verb sentences attach by nearest verb within
    the window; dates are not yet used to order conflicting edges
    (precedence stands in). **Stretch deferred**: on-demand web
    verification for authorities with no corpus evidence — the web
    leg exists, but wiring it under the same provenance + scope rules
    needs its own verification pass and is follow-up, not V1.
- **True sub-agent orchestration, P1 + P2** (26 Oct 2026, LIVE-VERIFIED on
  :3001). `POST /api/v1/knowledge/agents/autonomous` now defaults to a
  lead → researchers → drafter → checker → reconciler pipeline
  (`crates/rag/src/subagents.rs`); `orchestrated: false` keeps the legacy
  planner-executor loop (live-verified: 72s, finished, correct verdict),
  and `stream: true` streams SSE status per sub-agent plus an extended
  Done event carrying every sub-agent's receipt (live-verified: Done
  carried all 6 receipts + audit 5 checked / 5 verified). Lead plans up
  to 3 branches; researchers run concurrently (tokio tasks) over the
  corpus + web legs with STRICT brief scoping (no full-vault fallback);
  drafter writes from findings + sources only; checker receives draft +
  cited sources ONLY (never the drafter's rationale) and runs the
  citation-integrity audit + a verdict-consistency check; on conflict
  the lead runs one narrowed re-query and FLAGS anything unresolved in
  the answer — it never invents a resolution. Defaults: max 3 parallel
  researchers, 25,000 tokens/sub-agent, 100,000 tokens/run (admin knob:
  `settings.orchestration_run_cap_tokens`, clamped 10k–1M), 120s per
  sub-agent (timeout aborts that specialist; run continues as
  `partial`). Metered through `usage.rs` child scopes — receipt token
  sums matched billing deltas exactly in every live run. Fixed
  workflows untouched. Agents page renders receipts + conflict flags
  (no P3 UI: no plan approval, per-run budget display, or run history).
  - **Measured, honestly**: 20-rule golden did NOT regress — **15/20**
    after (floor 14/20; all 5 fails wording-only, zero source misses).
    Unit tests 122 → **129** (7 new sub-agent tests).
  - **New multi-hop brief eval** (one Brief, 3 docs: IT Act s.79,
    2021 Rules r.3, Shreya Singhal; questions needing 2–3 sources):
    single-agent baseline 3/3 content hits (~5.3k tokens, 104–417s, 2/3
    ending `planner_error`). Orchestration: MH1 5,806 tokens / 530s
    (36h verdict hit, researcher-1 timed out twice, audit strip
    flagged + reconciler ran); MH2 refusal MISS (sole researcher timed
    out twice — drafter honestly refused rather than hallucinate);
    MH3 10,972 tokens / 571s (all 3 elements hit, checker flagged a
    Shreya claim + 16 audit strips, reconciler re-query timed out so
    the conflict stands flagged). SSE MH1: 36h hit, checker 5/5
    verified. Conflict probe (30-day vs 90-day Sharma clauses, one
    Brief): finished 106s, checker 8/8 verified, draft correctly hedged
    both periods + "which version governs not established" — conflict
    surfaced, nothing invented.
  - **Verdict: not a 3× failed trade, not a quality win either.**
    Orchestration costs ~1.1–2.1× single-agent tokens for the same
    content hits on this eval — because one Mix query already retrieves
    all three one-chunk docs, the single agent has nothing to beat
    here. What orchestration measurably adds is provenance (receipts),
    independent checking + flagging, and graceful degradation. Its
    liability on this stack is the anonymous Gemini bridge: one leg
    call takes ~80–90s, so three in parallel routinely blow the 120s
    deadline (web leg exonerated by probe: 79s with web vs 92s
    without) — the bounded sequential retry for timed-out researchers
    and lenient checker-JSON parsing were both added because live
    eval proved them necessary. Judge quality again on a fast
    provider before claiming a hit-rate gain; do not ship this as
    "better answers" on bridge evidence alone.
- **Password-protected PDF uploads (P1) + S3 encrypted-object skip (P2)**
  (26 Oct 2026). Both upload endpoints — corpus
  `POST /api/v1/vault/documents` and private-vault upload — accept an
  optional `file_password`. The file is decrypted in memory before
  extraction, so a scanned encrypted PDF flows through the unchanged
  render → Surya OCR path; the password is zeroized once decryption
  returns, never stored, never logged, and for private-vault uploads
  not even the fact one was supplied is logged. Wrong password →
  distinct HTTP 400 `wrong file password` (not the generic extraction
  failure); encrypted file without a password → HTTP 400 naming the
  protection; unprotected file + password → password ignored, normal
  path. Mechanism settled by measurement on real fixtures (plain +
  RC4 / AES-128 / AES-256 PDFs, one test-only password, committed under
  `crates/ingest/tests/fixtures/`): pure-Rust `lopdf` 0.34 covers RC4
  only (AES-128 "scheme not supported", AES-256 "unsupported key
  length"); `lopdf` 0.46 decrypts AES but loads only the `/Encrypt`
  object (1 object, 0 pages) from xref-stream files and re-saves an
  empty document, so it was rejected. The implementation therefore
  tries `lopdf` first and falls through to a `pypdf` helper
  (`tools/pdf_decrypt_sidecar.py`, password on stdin, never argv,
  exit 42 = wrong password, stderr discarded). S3 ingest pre-skips
  encrypted PDFs into `IngestReport.skipped_encrypted` and surfaces
  download + batch failures in `IngestReport.failed` instead of
  swallowing them — batch continues, per-file errors reported.
  **Live-verified on the :3001 test instance** (26 Oct): AES-256 and
  RC4 uploads with the correct password returned 200 with chunk text
  identical to the unprotected original (same entities/relations);
  wrong password 400 `wrong file password` on both endpoints;
  no-password 400 protected-file error; unprotected + password 200;
  private-vault AES-128 upload 200 with 1 chunk; all live docs deleted
  afterwards and the private vault re-locked. **Not live-verified by
  design:** the S3 skip path is unit-tested only — no S3/R2
  credentials exist in this environment. Out of v1 per spec:
  Office-encrypted documents, and per-file at-rest passwords (the
  Confidential Vault already owns at-rest encryption). Honest limit
  carried over: decrypted text still reaches the embedding/LLM
  provider exactly as today's vault limits state; and the `pypdf`
  helper (with `cryptography` for AES-256) must be present for AES
  files — RC4 works with no runtime dependency. **Durability fix same
  day:** a VM restart wiped the system-wide pip install and the AES
  path failed outright (rag suite caught it: 95/96 until fixed), so
  `pypdf`/`cryptography` are now vendored under
  `tools/pdf-decrypt-pylibs/` (gitignored, 22 MB), the helper
  bootstraps that path itself, and `restore-stack.sh` reinstalls the
  vendored libs when missing. Re-verified live after the restart
  (AES-256 corpus 200 with identical text, wrong-password 400,
  no-password 400, private AES-128 200) and the full rag suite is
  96/96.

### Changed
- **Brief-only vault — no full-vault view (29 Oct 2026, user
  decision)**: documents are only ever worked within a Brief. The
  "Working in" switcher loses its "Full vault (all briefs)" option
  and only chooses WHICH brief: an active brief always exists —
  with nothing chosen (or a stored pick that no longer exists) the
  active brief is the default brief, the same fallback uploads
  already used server-side. The Vault page now shows ONLY the
  active brief's documents (via the brief-scoped list endpoint);
  the standalone Interact view loses the Piece 5/9 This brief /
  Full vault toggle, so its document universe is exactly the
  active brief (Piece 9's brief-wide default is now the only
  scope). Uploads keep filing into the active brief; S3 bulk
  ingest still files into the default brief and now says so.
  No document can become unreachable: the boot migration's
  adoption runs through a counted `adopt_unfiled` (count logged),
  and deleting a brief or detaching a document from its only
  brief re-files the orphaned documents into the default brief
  immediately — the vault has no unfiled state. Deliberately NOT
  changed: Ask/Draft retrieval internals (incl. the zero-hit
  full-vault fallback and Ask's in-session "Search full vault
  instead" widening), Search, Graph, Dashboard, the in-brief
  "file an existing document" picker, and every Admin Command
  Center surface. **Verified live on :3001** (release build of
  this change; workspace tests 202 → **204 green**): fixture
  briefs A/B holding disjoint documents — the vault data path
  returned each brief's set only; a document orphaned by detach
  under the previous build sat in NO brief before the restart
  (38 total / 37 filed) and was adopted into the default brief
  at boot (38 / 38 after; General 30 → 31); detach on the new
  build re-homes instantly; the served bundle contains no
  "Full vault (all briefs)" option and no scope-toggle copy.
  Fixtures were removed after verification (corpus back to
  35 / 35). Note: the adopted count is logged via `tracing::info`
  beside the existing "matters ready" line — the test instance
  runs under the default log filter (RUST_LOG unset), so the
  adoption here is proven by the counts, not by the log line.
  Also fixed in passing: the matters store test helper now uses
  a unique temp file per test — parallel tests sharing one
  backing file could wipe each other's matters from disk, which
  surfaced as a flaky failure in the Piece 4 regression test.

### Fixed
- **Document search is now a term search, as its own UI promises
  (30 Oct 2026, Piece 24).** The Search documents endpoint treated
  the whole query as one literal phrase, so "incomes 86" returned
  no matches — in the very document headed "Other incomes / 86.
  Income-tax shall not be payable…" the words sit a punctuation
  mark apart, and number-plus-word searches failed generally.
  Queries now split into terms and a document matches when every
  term appears somewhere in its title or text, in any order;
  ranking (title first, then match count) and snippets are
  unchanged in shape. Verified live on the test instance:
  "incomes 86" → Section 86 IT Act, "royalty 310" → Kankar Mines
  Lease, single-term behaviour unchanged. 4 new unit tests
  (253 total, all green).
- **Ask streaming can no longer die at generation time: a
  non-streaming fallback guarantees answer text (30 Oct 2026,
  Piece 23).** Diagnosis (raw-socket probe against the test
  instance): statuses and sources streamed correctly, then the
  provider's streaming call returned "llm stream returned no
  content" and the stream closed — zero deltas, no Done —
  because the Gemini bridge answers a plain completion but
  cannot serve the OpenAI-style streaming call. The same
  failure family left prod Ask sitting on "Searching the vault
  and the web…" with no text ever generated. Now generation in
  the streaming path tries the stream first — real
  token-by-token streaming where the provider supports it —
  and if the stream errors or yields no text, it falls back to
  the provider's non-streaming completion with the same prompt
  and reveals the answer in word chunks (one shared chunker,
  also adopted by the cache-replay paths), converging on the
  same failure-page guard, citation audit, treatment note and
  Done as the streamed path. Only if both calls fail does the
  turn end in the client's (already visible) error state. Web
  mode also reports its own progress now — "Searching the
  web…", then "Reading N web sources…" — so the status line
  advances through the web phase instead of sitting on the
  opening label for the leg's whole budget. Three unit tests
  pin the fallback branches: stream error → chunked fallback;
  empty stream → fallback; healthy stream → fallback never
  called. Honest naming: on a provider that cannot stream, the
  reveal is a chunked replay of a completed answer, not live
  token generation — but it is never a dead stream.
- **Ask now streams through the public tunnel instead of sitting
  on "Connecting…" until the answer is complete (30 Oct 2026,
  Piece 22 + 22b — DEPLOYED and live-verified 30 Oct 2026;
  streaming is live on the public site).** The app itself always
  streamed (SSE Status → Sources → Deltas → Done), but both ends
  of the public relay buffered whole responses: the VM agent
  collected the entire app body before sending one WebSocket
  message, the Worker awaited that single message before
  returning anything, and the agent killed any app response
  taking over 50 seconds. Through the public URL the browser
  therefore received zero bytes until generation finished (or
  the 50s kill). Now the agent sends a head frame the moment the
  app returns headers, one frame per response chunk, and an end
  frame; the Worker resolves the response on the head frame and
  pipes chunks through a TransformStream; the 50s total deadline
  is replaced by a 300s idle timeout on both ends (long
  generations survive while bytes keep flowing). The Worker
  still accepts the previous agent's whole-body messages, so the
  rollout order is: deploy Worker, then restart the agent —
  never the reverse. Dress-rehearsal verified locally (wrangler
  dev + both agent builds against a timed SSE source): chunks
  arrived at the client every ~400ms as produced, not batched,
  and a legacy agent answered correctly through the new Worker.
  Deployed the same day — and the first live swap of the new
  agent exposed a production-only failure the rehearsal could
  not: responses came back 200 with empty bodies. Piece 22b
  root-caused it with frame-level live probes: the agent emitted
  head/chunk/end correctly and in order over the live proxy/TLS
  path, but body frames reaching the Durable Object in the same
  burst as the head frame were discarded — the Object resolves
  the client Response from the head, but the runtime has not yet
  wired the response stream through to the client, and chunk
  writes landing in that window are dropped (wrangler dev
  buffers such early writes, which is why the rehearsal passed);
  the end frame then closed an empty body. The agent now holds
  the first body frame for a 200ms settle window after the head
  and serializes body frames through a per-request promise chain
  (gates of 100ms and 400ms were both live-verified before the
  value was chosen); later frames stream with no added delay.
  Live verification of the deployed fix: /health returns its
  full JSON body, the 777,931-byte SPA bundle downloads
  byte-identical (SHA-256 match) with first byte at ~0.6s of a
  ~1.7s transfer, an unauthenticated stream POST returns its
  401 promptly, the tunnel socket stays up with no reconnect
  cycles, and health-check reports HEALTHY. The canonical
  public address is now
  https://legal-rag-relay.jurisvault.workers.dev (the account
  subdomain was renamed at the owner's request; the previous
  jishan-legal-rag address is retired).
- **SereneDB restore path can no longer empty the corpus
  (30 Oct 2026, Piece 21).** The second half of the Piece 20
  incident: executing that recovery exposed a worse bug in the
  restore path itself. The SereneDB snapshot dump
  (`SereneDbVectorStore::all_chunks`) used a text-only projection
  with **no embedding column**, so every snapshot written while
  SereneDB was the active store held embedding-less records — and
  the boot restore's upsert deleted every snapshot id from
  `legal_chunks` *first*, then failed its single insert
  (`VARCHAR '[]' can't be cast to FLOAT[1024]`) on those empty
  embeddings, turning a heal into a total corpus loss (prod was
  healed manually the same night; until this fix deployed, any
  prod restart would have re-run the destructive restore). Fixes:
  (1) `all_chunks` is now a true backup projection — it selects
  the embedding column and decodes it (tokio-postgres exposes the
  returned `FLOAT[N]` value as an `f32` array; the bracketed text
  form the writer emits is parsed as a fallback), while the
  genuinely text-only projections (`search`, `get_by_ids`,
  `chunks_of_doc`) stay as they are — no caller of those consumes
  record embeddings; (2) upsert validates **before any DELETE**:
  the store now carries its configured dimensions, records whose
  embedding length differs (empty included) are skipped with a
  loud WARN naming count, ids and reasons, and a skipped id never
  enters a delete batch — an existing row for it is left
  untouched; (3) valid records upsert in batches of at most 25
  with each batch's delete + insert kept together, so a mid-way
  failure strands at most one small batch. Regression coverage:
  pure unit tests for the partition/batch plan (mixed valid,
  empty and wrong-dimension records; skipped ids never batched),
  plus a `SERENEDB_URL`-gated live integration test against a
  unique scratch table proving a seeded row targeted by an
  empty-embedding record survives byte-identical, a
  wrong-dimension record never lands, and 26 new valid records
  land across batches — verified against the local SereneDB.
  Full workspace suite: **246 passed / 0 failed**; the test
  instance restarted on this build passes Ask + Interact smoke
  with `/health` sane (42 documents, 0 hollow).
- **Corpus durability: restarts can no longer silently erase indexed
  documents, and chunk loss is now detected and recoverable
  (30 Oct 2026, Piece 20).** The data-loss class, plainly: chunks
  live in the vector store and are mirrored to `chunks.json` on every
  snapshot save, and the snapshot is upserted back into the store at
  boot. When the database died uncleanly (sandbox replacements kill
  it hard — 49 starts in its log) it came back missing committed
  chunks; the next snapshot save then dumped the *thinned* store over
  `chunks.json`, propagating the loss into the only backup instead of
  healing from it. On prod this hollowed out 4 of 5 registered
  documents (Section 86 IT Act, Gazette of India 1947, the Hindi Act
  scan, and a Section 1 document): their registry metadata survived,
  but their chunk text and embeddings were gone — Interact failed
  with "document has no extractable text" and Ask silently could not
  cite them. What now prevents and detects it: (1) the chunk snapshot
  is **monotonic** — every save unions the active chunk set with the
  snapshot on disk, the store's record winning per chunk id, and
  disk-only records survive only while their document is still in the
  registry, so deletions still shrink the corpus and deleted chunks
  never resurrect; (2) **hollow detection** — after boot restore and
  on every snapshot save, registered documents the store holds no
  chunks for are recomputed and logged as loud `HOLLOW DOCUMENT`
  warnings naming the document with expected vs found counts, and
  `/health` reports `documents_total` / `documents_hollow` (the
  health watch prints an alert-only `HOLLOW <n>` line — a restart
  cannot heal hollowness, so it never triggers one); (3) recovery is
  staged in **`recover-chunks.py`**, which imports registry-named
  chunk records from a donor snapshot (dry-run against a copy of prod
  data with `data-test/chunks.json` as donor recovers 10 of the 11
  lost chunks — Section 86 ×2, Gazette ×7, Hindi scan ×1, embeddings
  intact; the Section 1 document's chunk is in no surviving snapshot
  and must be re-ingested from source). Regression tests cover the
  merge union, the registry filter, boot-restore healing, and exact
  hollow flagging. The prod recovery itself is executed separately
  with the deploy restart; this entry records the mechanism.
- **Brief-vault and soft Asks no longer share one cached completion
  (29 Oct 2026, Piece 15, defect 2).** Piece 12 partitioned the
  semantic ANSWER cache by mode, but the completion cache below it
  (`CachedLlm`, the production wiring in `main.rs`) keys only on
  (system, prompt) — and the brief-vault mode never entered the
  prompt: it lived only in the scope note and the answer-cache key.
  A hard ask and a soft ask over the same brief retrieve the same
  context, so their prompts were byte-identical and the second mode
  was silently served the first mode's generated text in 0s, with
  its own mode's annotations stamped on top (observed live on :3001:
  a soft ask replayed the hard ask's cached completion, scope note
  None). Piece 12's unit test missed it because its service used a
  bare mock LLM — the completion cache was not in its path. Fix:
  brief-vault generations now carry a system-prompt addendum naming
  the brief and the boundary (also a real instruction to the model),
  so the modes' prompts — and therefore their completion-cache
  entries — are distinct in both directions. Regression coverage is
  at the handler's own composition: the query service wired exactly
  as `main.rs` builds it (`CachedLlm` over a scripted provider),
  driven through `query_opts`, asserting per-mode provider calls,
  per-mode answers in both orders, and scope notes surviving cache
  serves (the fallback disclosure for soft serves, the brief-vault
  note for hard serves); the cross-serve test fails if the addendum
  is removed.
- **Second Gemini failure page recognised and purged (29 Oct 2026,
  Piece 15, defect 1).** The bridge has more than one failure page:
  "I seem to be encountering an error. Can I try something else for
  you?" — Gemini's could-not-generate text — did not open with the
  Piece 14 prefix, so it was served as a genuine Ask answer (with the
  scoped brief's citation attached) and cached, then re-served in 0s.
  `is_failure_output` now covers both observed pages plus the same
  UI's other failure phrasings, each a full opening phrase (the
  trouble page is keyed on its connection-specific opening — a bare
  "I'm having trouble…" is deliberately NOT matched, because a
  genuine short answer can open that way; the ≤200-char whole-output
  cap remains the second guard, and long answers that quote a
  failure page inside an analysis stay safe). With the extended
  detector, the standing purge-on-read machinery in both caches
  evicts the stored poison on first touch, and the Ask generation
  exits (blocking + streaming) error on the variant instead of
  polishing it into an answer. **Verified live on :3001**: the exact
  incident input (Brief vault ON, the probe brief, the royalty
  question) regenerated an honest grounded result in place of the
  error text; a control unscoped Ask answered normally (no
  false-positive on real content).
- **Provider failure pages are never cached as answers (29 Oct 2026,
  Piece 14).** A web-backed LLM bridge returns its failure page as an
  ordinary HTTP 200 completion ("Sorry, something went wrong. Please
  try your request again."), so one transient bridge failure during
  an Interact chat was written into the shared LLM cache and then
  re-served in 0s forever for that exact (documents, action,
  question) input — a 200 carrying error text, indistinguishable from
  an answer. Fixed at three layers. (1) Provider boundary:
  `is_failure_output` (whole-output, short, known-prefix match) makes
  `OpenAiLlm` — blocking and streaming — return an upstream error for
  a failure page instead of a completion, so Interact / Draft /
  Research / Agents surface an honest error status and a retry
  reaches the provider. (2) `CachedLlm` never writes a failure page
  and purges one on read (entries stored by older builds self-heal);
  the semantic answer cache refuses failure answers at `store` and
  purges them at `lookup`. (3) Ask guards both generation exits
  (blocking + streaming) before the citation audit and the answer
  cache, so a failure can never be polished into — or stored as — a
  grounded-looking response on any client configuration. Successful
  answers still cache exactly as before. Regression tests: scripted
  fail-then-recover LLM at the cache layer (no write, purge-on-read,
  retry reaches the provider, success still cached, blocking +
  streaming), answer-cache refusal/purge, and query-level
  fail-then-succeed on both the blocking and streaming Ask paths.
- **Citator misattribution — a named reversal no longer lands on an
  unmatched sole candidate (27 Oct 2026, engine fix + tests).** A live
  orchestrated run showed "Section 86 IT Act" as **reversed** with Mehta
  evidence. Root cause was NOT the merge (ruled out: `incorporate_branch`
  clones treatments, it never resolves them) but the engine's
  `detect_reversals` signal 1 ("sole-candidate-impugned"): a bare
  "impugned" sentence linked the reversal to the only cited judgment
  below the source forum, even when the sentence NAMED a different case
  — researcher-2's answer cited only the HC judgment + the tax section,
  so the tax section inherited the Mehta reversal. Signal 1 now fires
  only when the sentence names no case at all; when it names a case, the
  impugned judgment is the named case and only the name-based fallback
  (CaseName, resolved on a real match) may link it. The unnamed-sentence
  cases keep working (existing appeal-number, CNR, party-swap tests
  green). Regression tests at engine level, adapter level (exact live
  shape: stored HC→district edge inert, district not cited, s.86 must
  stay Unknown), and merge level (overlapping positional ids never
  cross-wire; shared chunk dedupes to one treatment). Standalone suite
  41 green (was 40); workspace 181 green (was 177). V1 untouched when
  the switch is off. **Live-verified on :3001 after the fix (V2 on):**
  Ask "current status of Mehta v. Desai?" (mix, top_k 10) — governing
  line present, HC good_law, district reversed ranked LAST, and
  Section 86 IT Act **unknown** (was reversed pre-fix); fresh
  orchestrated run on the same goal — stopped finished, HC good_law,
  verbatim V2 governing line, verdict follows the chain (SC affirmed
  the HC set-aside), district correctly reversed, and no unrelated
  citation carrying a chain treatment (s.86 absent from that run's
  citations, so the direct s.86 live proof is the Ask run above).
  No push, no version bump. Residual known
  weakness, by design: a sentence naming NO case + a single candidate
  still links (existing tests endorse it for single-case answers); a
  multi-topic answer with a bare "impugned" sentence could still
  mislink — the mitigation is that real chains carry party-match or
  stored edges, which outrank signal 1.
- **Web search leg rebuilt + failure-isolated; Research mode hardened
  (29 Oct 2026).** Ask with Web on ("tell me in detail about
  agriculture income") met a bare 502 and, when it succeeded, answered
  silently from the vault alone. Diagnosis, all verified live from
  this sandbox: the search sidecar returned `[]` after 62s — DDG HTML
  anomaly-blocks the egress IP (HTTP 202; the stealth Firefox gets
  the same block page), Bing's SERP for this IP degenerated into
  location/dictionary junk (an electoral-bonds query returned US
  Electoral College entries), Brave 429s, Mojeek captchas, and Indian
  Kanoon is Cloudflare-walled (HTTP connection reset; Firefox stops
  at "Just a moment…"). The 502 itself never came from the web leg —
  `gather_web` always swallowed its errors; the bare 502 is
  `Error::Upstream` → HTTP 502 from a later stage (embeddings/LLM)
  after the dead leg had burned 60–150s, and a successful corpus-only
  answer carried no hint that web had failed (mechanism pinned in a
  `gather_web` doc comment). Fixes:
  - `websearch.py` rewritten: sources run concurrently under a 40s
    deadline — Indian Kanoon's own search and DDG HTML, both read
    through the r.jina.ai reader proxy (which fetches from its own
    infrastructure, bypassing per-site egress blocks), plus `site:`
    probes (sci.gov.in / indiacode / legislative / incometaxindia)
    through the same DDG path, plus one fast direct DDG/Brave attempt
    each. Bing and the Firefox search fallback dropped. Tier ranking
    kept; a per-engine diversity cap stops one engine's tier-1 sweep
    crowding official explainer pages out of the fetch window. Every
    source logs count or failure reason to stderr. Agriculture query:
    62s/`[]` → ~8s with Kanoon case law + India Code Acts +
    incometaxindia.gov.in official pages; electoral-bonds query
    returns the actual SCI judgment PDF. `webfetch.py`: reader proxy
    is now the universal second path (HTTP → proxy → Firefox) and
    leads for Kanoon.
  - `gather_web` returns an outcome (Delivered / NoResults / Failed)
    under strict budgets (search 60s cap; fetch waves 40s/35s/60s);
    every failure logs stage + reason. Ask (blocking + streaming) and
    Research surface a non-delivered leg as an explicit "web search
    unavailable / no usable sources — based only on your vault" note.
    The web leg can still never fail a request; `LEGAL_RAG_WEB_SEARCH=0`
    remains the kill switch.
  - Research (`research.rs`, the clearai-dsh epistemic loop): frame,
    per-hypothesis retrieval, and adjudication failures now degrade
    the report instead of erroring the whole request (the 2026-10-25
    BNS 502 shape). Verdicts are enforced against delivered evidence
    in code — claims may only cite delivered citation ids, a
    hypothesis with no delivered evidence is forced inconclusive,
    and with zero delivered evidence the conclusion is replaced by an
    insufficiency statement. Refuted hypotheses stay (test-pinned).
    Report gains the fourth answer part, `decisions` (adjudicator's
    plus one per conflict), a `web_note`, and a
    predictions-don't-discriminate open point. Workspace 190 green
    (was 181; +9 tests: web outcome taxonomy, corpus-only-with-note,
    frame/adjudicate degradation, evidence enforcement, refuted
    retention, web noting, discrimination flag). Production :3000
    untouched. Live-verified on :3001 (29 Oct): Ask web=true on the
    agriculture-income question → HTTP 200 with Indian Kanoon +
    incometaxindia.gov.in citations and a Section 10(1)-grounded
    answer; Ask with the kill switch on → HTTP 200 in 15s, corpus-only,
    scope_note "Live web search was unavailable (configuration error:
    web search not configured)"; Research web=true → HTTP 200,
    hypotheses adjudicated with decisions + open points and 6 web
    citations; Research with web broken → HTTP 200 corpus-only with
    web_note and inconclusive verdicts.
- **Brief status change "5xx" — investigated, no code defect found
  (29 Oct 2026)**: the full path was re-verified live on :3001 against
  the real 26-document General brief — PATCH through Active / On hold
  / Closed and the UI's refetches (detail, list, memory, timeline)
  all returned 200, and both instances serve the current frontend
  bundle whose StatusSelect sends exactly those values. The report
  coincided with the day's repeated sandbox restarts (stack down);
  with the backend unreachable, the serving layer returns the 502 the
  user saw — the same surface as the Ask 502. No production code
  change was made; a store-level regression test now locks all three
  transitions on a brief with a filed document, including persistence
  across reload.

## [1.8.0] — 2026-10-25

### Added
- **R2 / S3-compatible endpoint option for the S3 vault import**
  (25 Oct 2026). `S3Config` gains `endpoint_url`
  (`LEGAL_RAG_S3_ENDPOINT_URL`); `S3Source::connect_with_endpoint`
  builds the SDK client with that endpoint and path-style addressing
  forced (`<endpoint>/<bucket>/<key>`), which is what R2-style
  stores expect, instead of virtual-hosted AWS addressing. Region is
  still required for signing (R2 users set it to `auto`). Unset, the
  client is stock AWS, unchanged. **Not live-verified by design**:
  no R2/S3 credentials exist in this environment, so verification is
  a unit test (endpoint and AWS-default construction, accessors,
  hermetic dummy credentials, 0.05s, no network touched) — a live
  R2 round-trip still needs real credentials.
- **Verdict-first format for corpus Ask answers** (25 Oct 2026).
  The verdict-first shape that web-augmented answers already used
  (a `## Verdict` section with the direct answer in 2–4 sentences,
  then `## Analysis`, then `## Caveats` only when sources conflict or
  a source is dated) now applies to corpus (non-web) answers too, via
  a new `CORPUS_FORMAT_ADDENDUM` in `crates/rag/src/query.rs` that
  cites `[doc:N]` only — it never mentions `[web:N]`, so a corpus
  answer is never invited to invent a web citation. Web answers keep
  their existing addendum byte-for-byte; Bypass (own-knowledge)
  answers are untouched, and the citation-integrity audit is
  format-agnostic, so it keeps checking every marker either way.
  **Verified live 25 Oct** on the :3001 test instance: a fresh corpus
  question answered starting with `## Verdict` plus `## Analysis`,
  3 citations, audit 15 checked / 14 verified / 1 stripped, zero
  `[web:N]` markers; a web-augmented question also opened with
  `## Verdict` with the web leg running (web:0, web:1 in citations)
  and audit 11 checked / 10 verified / 1 stripped.
- **Thumbs up/down feedback loop** (25 Oct 2026). Ask answers now
  carry Helpful / Not helpful buttons; votes are stored server-side
  (`feedback.json`) with the question and an excerpt of the judged
  answer, one vote per user per answer (re-voting updates instead of
  duplicating). Votes are retrievable at the documented read endpoint
  `GET /api/v1/admin/feedback` (votes newest-first with up/down
  counts); no admin UI page was added for it. Invalid votes are
  rejected (400) and voting requires login (401 without a token).
  **Verified live 25 Oct** on the :3001 test instance: vote up, then
  re-vote down on the same answer (same vote id, counts flipped
  1/0 to 0/1), with the probe vote cleaned from test data after.
- **Golden-rule edit endpoint** (25 Oct 2026). Admins can now edit an
  existing golden rule in place via `PUT /api/v1/admin/golden/rules/{id}`
  (only the fields present in the body change; id, source, created_at,
  and run history are never touched), with a pencil button on each row
  of the admin Golden Ruleset page. Editing no longer means deleting
  the rule and re-adding it. Empty questions are rejected (400) and
  unknown ids return 404. **Verified live 25 Oct** on the :3001 test
  instance: a temp rule was added, partially edited (terms + notes
  changed, question preserved), confirmed via GET, then deleted,
  leaving the 20-rule set intact.
- **Reranker token cache** (25 Oct 2026; txtai-style). The
  cross-encoder sidecar (`tools/rerank/bge_reranker_sidecar.py`, :8081)
  now caches tokenized (query, document) pairs server-side, keyed by
  SHA-256 over the full pair text, so repeat pairs skip tokenization
  and go straight to the model forward. Kill switch
  `RERANK_TOKEN_CACHE=0` (default enabled; disabled mode falls back
  to the original `CrossEncoder.predict` path unchanged); cap
  `RERANK_TOKEN_CACHE_MAX_ENTRIES` (default 10000, oldest evicted
  first); cache stats (`enabled`, `hits`, `misses`, `entries`) are
  reported additively in `GET /health` and in the new `GET /stats`.
  The `/rerank` request/response contract is unchanged. **Verified
  live 25 Oct** with the repeatable probe
  `tools/rerank/test_token_cache.py` (no Rust dependency): a repeat
  identical 3-document call went from 3 misses to 3 hits, and sidecar
  scores matched a fresh `CrossEncoder.predict` computation to a max
  abs diff of 7.5e-09 (tolerance 1e-4); the disable knob was verified
  by restart (`enabled: false`, 0 hits on repeat calls) and the
  sidecar was left running cache-enabled.
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
  contract. Bench (`bm25_engine_bench`, 24 Oct, re-measured on an
  idle box after an early contended run inverted the result —
  correction): at 324 chunks native builds 6.1 ms / queries 0.123 ms
  vs Tantivy 13.2 ms / 0.052 ms; at 50,000 chunks native 828 ms /
  63.4 ms vs Tantivy 313 ms / 2.25 ms (Tantivy ≈28× faster per query;
  the native linear scan collapses at scale). Top-10 overlap is
  9.3–9.5/10 — rankings are near-equivalent (the residual gap is
  Tantivy's fixed k1=1.2 vs native 1.5). Native stays the default at
  today's corpus sizes (zero extra dependency, exact scoring
  parity); Tantivy is the measured scale path when corpora grow.
  (f) **MCP server**: `POST /mcp` (streamable-HTTP style, JSON-RPC
  2.0, protocol 2024-11-05) with tools `search_vault`,
  `list_documents`, `ask`; auth is the caller's normal bearer token
  (quota enforced on `ask`). Admin Command Center shows live
  retrieval-stack status (new `GET /api/v1/admin/retrieval-status`),
  the MCP panel with a copyable client config, and the new hit-test
  stage. Kill switch `LEGAL_RAG_MCP=0`.
  Verification status: **live-verified on the test instance**
  (24 Oct): 110 workspace tests + both frontend builds green;
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

### Fixed
- **Stack restore now self-heals the ColBERT sidecar** (24 Oct 2026):
  `restore-stack.sh` predated the Tier-2 sidecar, so `:8084` had to be
  started manually after every sandbox restart; it now launches with
  the other sidecars and joins the health-check loop. Verified: all
  seven ports up from a bare restore with no manual step.

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
