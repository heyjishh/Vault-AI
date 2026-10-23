/** API shapes — mirrors the legal-rag-server routes exactly. */

export interface UserView {
  id: string
  email: string
  role: string // 'user' | 'admin'
}

export interface TokenPair {
  access_token: string
  refresh_token: string
  token_type?: string
  expires_in?: number
}

export interface AuthResponse {
  user: UserView
  tokens: TokenPair
}

export type QueryMode = 'naive' | 'local' | 'global' | 'hybrid' | 'mix' | 'bypass'

export interface Citation {
  doc_id: string
  doc_title?: string | null
  chunk_id: string
  ordinal: number
  score: number
  snippet: string
  /** Pinpoint: the single sentence in the passage that supports the citation. */
  sentence?: string | null
  /** Staleness note when the cited document has been superseded. */
  stale?: string | null
  /** Origin URL for web citations (authority card link). */
  source_url?: string | null
  /** Publication / judgment date (YYYY-MM-DD) for web citations. */
  source_date?: string | null
}

/** Full text behind one citation, for the in-app source viewer modal. */
export interface SourceDocument {
  title?: string | null
  url?: string | null
  date?: string | null
  text: string
  /** Resolved corpus document id (corpus sources only). */
  document_id?: string | null
}

/** Citation-integrity receipt: markers mechanically checked against
 *  the source each one cites. */
export interface CitationAudit {
  checked: number
  verified: number
  stripped: number
}

export interface QueryAnswer {
  answer: string
  citations: Citation[]
  mode: QueryMode
  scope_note?: string | null
  /** True when served from a human-verified annotation. */
  verified?: boolean
  citation_audit?: CitationAudit | null
}

/** One hit from the corpus document search. */
export interface DocSearchHit {
  doc_id: string
  title: string
  matches: number
  title_match: boolean
  snippets: string[]
}

/** A custom sidebar tab an admin added to the user panel. */
export interface UiTab {
  id: string
  label: string
  kind: 'link' | 'page'
  url?: string
  content?: string
  visible: boolean
}

/** Public panel settings (admin-customized): branding, tabs, default mode. */
export interface PublicSettings {
  brand: { product_name: string; tagline: string }
  default_mode: string
  tabs: UiTab[]
}

/** A sealed document in the user's confidential vault (titles decrypted owner-side only while unlocked). */
export interface PrivateDocMeta {
  id: string
  title: string
  sha256: string
  chunks: number
  created_at: string
}

/** A matter workspace (mirrors crates/server/src/matters.rs). */
export interface Matter {
  id: string
  title: string
  client: string
  kind: string
  status: string // Active | On hold | Closed
  notes: string
  document_ids: string[]
  is_default: boolean
  /** Owning Space id; absent/null = no space boundary. */
  space_id?: string | null
  created_by: string
  created_at: string
  updated_at: string
}

export interface SpaceMember {
  user_id: string
  email: string
  role: string // owner | editor | viewer
}

export interface Space {
  id: string
  name: string
  owner_id: string
  members: SpaceMember[]
  created_at: string
  updated_at: string
}

export interface MatterSummary extends Matter {
  document_count: number
  activity_count: number
}

export interface MatterDetail extends Matter {
  documents: DocumentInfo[]
}

export interface TimelineItem {
  at: string
  kind: string
  title: string
  detail: string
  excerpt: string
  citations: number
  tokens: number
  user_email: string
}

export interface MatterMemory {
  document_count: number
  chunk_count: number
  entities: { name: string; kind: string; mentions: number }[]
  activity_count: number
  last_activity_at: string | null
}

/** Legal-validity metadata for a document (all fields optional). */
export interface DocMeta {
  jurisdiction?: string | null
  effective_from?: string | null
  effective_to?: string | null
  superseded_by?: string | null
}

export interface DocumentInfo {
  id: string
  title: string
  source_uri: string
  chars: number
  chunks: number
  chunk_ids: string[]
  ingested_at: string
  meta?: DocMeta
}

export interface IngestReport {
  documents: number
  chunks: number
  entities: number
  relations: number
  modal_processed: number
  doc_id?: string | null
  matter_id?: string | null
}

export interface CorpusStats {
  documents: number
  chunks: number
  entities: number
  relations: number
}

export interface Entity {
  name: string
  kind: string
  description: string
  chunk_ids: string[]
}

export interface Relation {
  from: string
  to: string
  description: string
  chunk_ids: string[]
}

export interface GraphView {
  entities: Entity[]
  relations: Relation[]
}

export interface EvidenceNote {
  claim: string
  source: string
}

export interface Hypothesis {
  question: string
  prediction: string
  status: string // supported | refuted | inconclusive
  evidence: EvidenceNote[]
}

export interface Conflict {
  topic: string
  position_a: string
  position_b: string
}

export interface ResearchReport {
  question: string
  conclusion: string
  hypotheses: Hypothesis[]
  conflicts: Conflict[]
  open_points: string[]
  citations: Citation[]
}

export interface WorkflowStepDef {
  id: string
  name: string
  mode?: string
  top_k?: number
  focus?: string | null
  use_web?: boolean
}

export interface WorkflowDef {
  id: string
  name: string
  description: string
  input_label: string
  steps: WorkflowStepDef[]
}

export interface StepResult {
  step_id: string
  name: string
  findings: string
  citations: Citation[]
}

export interface WorkflowRun {
  workflow_id: string
  workflow_name: string
  input: string
  steps: StepResult[]
  final_report: string
  citations: Citation[]
}

export interface AgentStepTrace {
  n: number
  action: string
  input: string
  reason: string
  findings: string
  citations: Citation[]
}

export interface AutonomousRun {
  goal: string
  steps: AgentStepTrace[]
  final_report: string
  citations: Citation[]
  stopped: string
  max_steps: number
}

export interface CacheStats {
  llm: { hits: number; misses: number; entries: number } | null
  embeddings: { hits: number; misses: number; entries: number } | null
}
