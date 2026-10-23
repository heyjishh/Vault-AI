import axios, { AxiosError, InternalAxiosRequestConfig } from 'axios'
import type {
  AuthResponse, CacheStats, Citation, CitationAudit, CorpusStats, DocMeta, DocSearchHit, DocumentInfo, GraphView, Space,
  IngestReport, Matter, MatterDetail, MatterMemory, MatterSummary, PrivateDocMeta, PublicSettings,
  QueryAnswer, QueryMode, ResearchReport, SourceDocument, TimelineItem, TokenPair,
  WorkflowDef, WorkflowRun, AutonomousRun,
} from './types'

const ACCESS_KEY = 'lexagent.access'
const REFRESH_KEY = 'lexagent.refresh'

export const tokens = {
  get access(): string | null { return localStorage.getItem(ACCESS_KEY) },
  get refresh(): string | null { return localStorage.getItem(REFRESH_KEY) },
  save(pair: TokenPair) {
    localStorage.setItem(ACCESS_KEY, pair.access_token)
    localStorage.setItem(REFRESH_KEY, pair.refresh_token)
  },
  clear() {
    localStorage.removeItem(ACCESS_KEY)
    localStorage.removeItem(REFRESH_KEY)
  },
}

export const api = axios.create({ baseURL: '/api/v1' })

api.interceptors.request.use((config: InternalAxiosRequestConfig) => {
  const t = tokens.access
  if (t) config.headers.Authorization = `Bearer ${t}`
  return config
})

let refreshing: Promise<string | null> | null = null

api.interceptors.response.use(
  (r) => r,
  async (error: AxiosError) => {
    const original = error.config as (InternalAxiosRequestConfig & { _retried?: boolean }) | undefined
    if (error.response?.status === 401 && original && !original._retried && tokens.refresh) {
      original._retried = true
      try {
        refreshing = refreshing ?? axios
          .post<{ } & TokenPair>('/api/v1/auth/refresh', { refresh_token: tokens.refresh })
          .then((r) => { tokens.save(r.data); return r.data.access_token })
          .finally(() => { refreshing = null })
        const access = await refreshing
        if (access) {
          original.headers.Authorization = `Bearer ${access}`
          return api(original)
        }
      } catch {
        tokens.clear()
        window.location.reload()
      }
    }
    return Promise.reject(error)
  },
)

function err(e: unknown): string {
  if (axios.isAxiosError(e)) {
    const d = e.response?.data as { error?: string; message?: string } | undefined
    return d?.error ?? d?.message ?? e.message
  }
  return String(e)
}

export const authApi = {
  async register(email: string, password: string): Promise<AuthResponse> {
    try { const r = await api.post<AuthResponse>('/auth/register', { email, password }); return r.data }
    catch (e) { throw new Error(err(e)) }
  },
  async login(email: string, password: string): Promise<AuthResponse> {
    try { const r = await api.post<AuthResponse>('/auth/login', { email, password }); return r.data }
    catch (e) { throw new Error(err(e)) }
  },
  async me(): Promise<import('./types').UserView> {
    const r = await api.get<import('./types').UserView>('/auth/me'); return r.data
  },
}

export const ragApi = {
  async query(q: string, mode: QueryMode, top_k: number, web: boolean, web_results = 3, matter_id?: string, priv?: boolean): Promise<QueryAnswer> {
    try {
      const r = await api.post<QueryAnswer>('/knowledge/query', { query: q, mode, top_k, web, web_results, matter_id, private: priv || undefined })
      return r.data
    } catch (e) { throw new Error(err(e)) }
  },
  async sourceDocument(body: { source_url?: string; chunk_id?: string; doc_id?: string }): Promise<SourceDocument> {
    try {
      const r = await api.post<SourceDocument>('/knowledge/source-document', body)
      return r.data
    } catch (e) { throw new Error(err(e)) }
  },
  async research(query: string, web: boolean): Promise<ResearchReport> {
    try { const r = await api.post<ResearchReport>('/knowledge/research', { query, web }); return r.data }
    catch (e) { throw new Error(err(e)) }
  },
  async stats(): Promise<CorpusStats> {
    const r = await api.get<CorpusStats>('/knowledge/stats'); return r.data
  },
  async graph(): Promise<GraphView> {
    const r = await api.get<GraphView>('/knowledge/graph'); return r.data
  },
  async cacheStats(): Promise<CacheStats> {
    const r = await api.get<CacheStats>('/knowledge/cache-stats'); return r.data
  },
  async runAutonomous(goal: string, matter_id?: string, max_steps?: number): Promise<AutonomousRun> {
    try {
      const r = await api.post<AutonomousRun>('/knowledge/agents/autonomous', { goal, matter_id: matter_id ?? null, max_steps: max_steps ?? null })
      return r.data
    } catch (e) { throw new Error(err(e)) }
  },
  async workflows(): Promise<WorkflowDef[]> {
    const r = await api.get<WorkflowDef[]>('/knowledge/workflows'); return r.data
  },
  async runWorkflow(workflow_id: string, input: string): Promise<WorkflowRun> {
    try {
      const r = await api.post<WorkflowRun>('/knowledge/workflows/run', { workflow_id, input })
      return r.data
    } catch (e) { throw new Error(err(e)) }
  },
}

export const vaultApi = {
  async search(q: string): Promise<DocSearchHit[]> {
    const r = await api.get<DocSearchHit[]>('/vault/search', { params: { q } }); return r.data
  },
  async list(): Promise<DocumentInfo[]> {
    const r = await api.get<DocumentInfo[]>('/vault/documents'); return r.data
  },
  async listInMatter(matter_id: string): Promise<DocumentInfo[]> {
    const r = await api.get<DocumentInfo[]>('/vault/documents', { params: { matter_id } }); return r.data
  },
  async uploadFile(title: string, file: File, matter_id?: string): Promise<IngestReport> {
    const buf = await file.arrayBuffer()
    let binary = ''
    const bytes = new Uint8Array(buf)
    for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i])
    const content_base64 = btoa(binary)
    try {
      const r = await api.post<IngestReport>('/vault/documents', { title, file_name: file.name, content_base64, matter_id })
      return r.data
    } catch (e) { throw new Error(err(e)) }
  },
  async uploadText(title: string, file_name: string, text: string, matter_id?: string): Promise<IngestReport> {
    try {
      const r = await api.post<IngestReport>('/vault/documents', { title, file_name, text, matter_id })
      return r.data
    } catch (e) { throw new Error(err(e)) }
  },
  async ingestS3(prefix?: string): Promise<IngestReport> {
    try {
      const r = await api.post<IngestReport>('/vault/ingest', null, { params: prefix ? { prefix } : {} })
      return r.data
    } catch (e) { throw new Error(err(e)) }
  },
  async remove(id: string): Promise<void> {
    try { await api.delete(`/vault/documents/${id}`) }
    catch (e) { throw new Error(err(e)) }
  },
  async updateMeta(id: string, meta: DocMeta): Promise<DocumentInfo> {
    try {
      const r = await api.patch<DocumentInfo>(`/vault/documents/${id}/metadata`, meta)
      return r.data
    } catch (e) { throw new Error(err(e)) }
  },
}

export interface Plan {
  id: string
  name: string
  price_inr: number
  monthly_tokens: number
  features: string[]
}

export interface BillingPayment {
  id: string
  order_id: string
  plan_id: string
  amount_inr: number
  provider: string
  status: string
  utr: string | null
  created_at: string
  completed_at: string | null
}

export interface MyBilling {
  plan: Plan
  subscription: { plan_id: string; status: string; period_start: string; period_end: string }
  tokens_used: number
  token_quota: number
  llm_prompt_tokens: number
  llm_completion_tokens: number
  embedding_tokens: number
  payments: BillingPayment[]
}

export const billingApi = {
  async plans(): Promise<Plan[]> {
    const r = await api.get<Plan[]>('/billing/plans'); return r.data
  },
  async me(): Promise<MyBilling> {
    const r = await api.get<MyBilling>('/billing/me'); return r.data
  },
  async checkout(plan_id: string): Promise<{ payment_id: string; mode: string; amount_inr: number; checkout_url: string | null }> {
    try {
      const r = await api.post('/billing/checkout', { plan_id }); return r.data
    } catch (e) { throw new Error(err(e)) }
  },
  async completeMock(payment_id: string): Promise<BillingPayment> {
    try {
      const r = await api.post<BillingPayment>(`/billing/payments/${payment_id}/complete-mock`); return r.data
    } catch (e) { throw new Error(err(e)) }
  },
  async paymentStatus(payment_id: string): Promise<BillingPayment> {
    const r = await api.get<BillingPayment>(`/billing/payments/${payment_id}`); return r.data
  },
}

export interface ActivityEvent {
  id: string
  user_id: string
  user_email: string
  kind: string
  title: string
  detail: string
  result_excerpt: string
  payload?: Record<string, unknown> | null
  citations: number
  tokens: number
  matter_id?: string | null
  created_at: string
}

export interface MyDashboard {
  plan_name: string
  plan_id: string
  tokens_used: number
  token_quota: number
  period_end: string
  counts: { queries: number; researches: number; workflows: number; interacts: number; drafts: number; uploads: number }
  recent_activity: ActivityEvent[]
  documents: DocumentInfo[]
  corpus_documents: number
  corpus_chunks: number
}

export const dashboardApi = {
  async me(): Promise<MyDashboard> {
    const r = await api.get<MyDashboard>('/dashboard/me'); return r.data
  },
}

export interface InteractResponse {
  document_id: string
  document_title: string
  action: string
  output: string
  truncated: boolean
}

export interface DraftResponse {
  kind: string
  draft: string
  citations: Citation[]
}

export const createApi = {
  async interact(document_id: string, action: string, question?: string): Promise<InteractResponse> {
    try {
      const r = await api.post<InteractResponse>('/knowledge/interact', { document_id, action, question })
      return r.data
    } catch (e) { throw new Error(err(e)) }
  },
  async draft(kind: string, instructions: string, use_corpus: boolean, matter_id?: string): Promise<DraftResponse> {
    try {
      const r = await api.post<DraftResponse>('/knowledge/draft', { kind, instructions, use_corpus, matter_id })
      return r.data
    } catch (e) { throw new Error(err(e)) }
  },
}

export const publicSettingsApi = {
  async get(): Promise<PublicSettings> {
    const r = await api.get<PublicSettings>('/settings/public'); return r.data
  },
}

export type { PrivateDocMeta }

export const privateVaultApi = {
  async status(): Promise<{ has_password: boolean; unlocked: boolean; documents: number; routing?: { embeddings: string; llm: string } }> {
    const r = await api.get('/private-vault/status'); return r.data
  },
  async setPassword(password: string): Promise<void> {
    try { await api.post('/private-vault/password', { password }) }
    catch (e) { throw new Error(err(e)) }
  },
  async unlock(password: string): Promise<void> {
    try { await api.post('/private-vault/unlock', { password }) }
    catch (e) { throw new Error(err(e)) }
  },
  async lock(): Promise<void> {
    await api.post('/private-vault/lock', {})
  },
  async changePassword(current_password: string, new_password: string): Promise<void> {
    try { await api.post('/private-vault/password/change', { current_password, new_password }) }
    catch (e) { throw new Error(err(e)) }
  },
  async list(): Promise<PrivateDocMeta[]> {
    const r = await api.get<{ locked: boolean; documents: PrivateDocMeta[] }>('/private-vault/documents')
    return r.data.documents
  },
  async upload(body: { title?: string; file_name: string; text?: string; content_base64?: string }): Promise<PrivateDocMeta> {
    try {
      const r = await api.post<PrivateDocMeta>('/private-vault/documents', body); return r.data
    } catch (e) { throw new Error(err(e)) }
  },
  async remove(id: string): Promise<void> {
    try { await api.delete(`/private-vault/documents/${id}`) }
    catch (e) { throw new Error(err(e)) }
  },
}

export const mattersApi = {
  async list(): Promise<MatterSummary[]> {
    const r = await api.get<MatterSummary[]>('/matters'); return r.data
  },
  async create(title: string, client: string, kind: string, notes: string, space_id?: string): Promise<Matter> {
    try {
      const r = await api.post<Matter>('/matters', { title, client, kind, notes, space_id }); return r.data
    } catch (e) { throw new Error(err(e)) }
  },
  async get(id: string): Promise<MatterDetail> {
    const r = await api.get<MatterDetail>(`/matters/${id}`); return r.data
  },
  async update(id: string, patch: Partial<Pick<Matter, 'title' | 'client' | 'kind' | 'status' | 'notes'>> & { space_id?: string }): Promise<Matter> {
    try {
      const r = await api.patch<Matter>(`/matters/${id}`, patch); return r.data
    } catch (e) { throw new Error(err(e)) }
  },
  async remove(id: string): Promise<void> {
    try { await api.delete(`/matters/${id}`) }
    catch (e) { throw new Error(err(e)) }
  },
  async attach(id: string, doc_id: string): Promise<Matter> {
    try {
      const r = await api.post<Matter>(`/matters/${id}/documents`, { doc_id }); return r.data
    } catch (e) { throw new Error(err(e)) }
  },
  async detach(id: string, doc_id: string): Promise<void> {
    try { await api.delete(`/matters/${id}/documents/${doc_id}`) }
    catch (e) { throw new Error(err(e)) }
  },
  async timeline(id: string): Promise<TimelineItem[]> {
    const r = await api.get<TimelineItem[]>(`/matters/${id}/timeline`); return r.data
  },
  async memory(id: string): Promise<MatterMemory> {
    const r = await api.get<MatterMemory>(`/matters/${id}/memory`); return r.data
  },
}

export const spacesApi = {
  async list(): Promise<Space[]> {
    const r = await api.get<Space[]>('/spaces'); return r.data
  },
  async create(name: string): Promise<Space> {
    try {
      const r = await api.post<Space>('/spaces', { name }); return r.data
    } catch (e) { throw new Error(err(e)) }
  },
  async rename(id: string, name: string): Promise<Space> {
    try {
      const r = await api.patch<Space>(`/spaces/${id}`, { name }); return r.data
    } catch (e) { throw new Error(err(e)) }
  },
  async remove(id: string): Promise<void> {
    try { await api.delete(`/spaces/${id}`) }
    catch (e) { throw new Error(err(e)) }
  },
  async addMember(id: string, email: string, role: string): Promise<Space> {
    try {
      const r = await api.post<Space>(`/spaces/${id}/members`, { email, role }); return r.data
    } catch (e) { throw new Error(err(e)) }
  },
  async removeMember(id: string, user_id: string): Promise<Space> {
    try {
      const r = await api.delete<Space>(`/spaces/${id}/members/${user_id}`); return r.data
    } catch (e) { throw new Error(err(e)) }
  },
}

export type { Citation }

export interface StreamHandlers {
  onStatus?: (message: string) => void
  onSources?: (citations: Citation[], mode: QueryMode, verified: boolean) => void
  onDelta?: (text: string) => void
  onDone?: (answer: string, audit?: CitationAudit | null) => void
  onError?: (message: string) => void
}

/**
 * Streaming query over SSE: status → sources → delta… → done.
 * Uses fetch + ReadableStream (axios can't stream in the browser).
 */
export async function queryStream(
  payload: { query: string; mode: QueryMode; top_k: number; web: boolean; web_results?: number; matter_id?: string; private?: boolean; as_of?: string; jurisdiction?: string },
  handlers: StreamHandlers,
  signal?: AbortSignal,
): Promise<void> {
  async function attempt(isRetry: boolean): Promise<void> {
    const res = await fetch('/api/v1/knowledge/query/stream', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        ...(tokens.access ? { Authorization: `Bearer ${tokens.access}` } : {}),
      },
      body: JSON.stringify(payload),
      signal,
    })
    if (res.status === 401 && !isRetry && tokens.refresh) {
      const r = await axios.post('/api/v1/auth/refresh', { refresh_token: tokens.refresh })
      tokens.save(r.data)
      return attempt(true)
    }
    if (!res.ok || !res.body) {
      const text = await res.text().catch(() => '')
      throw new Error(text || `stream failed (${res.status})`)
    }
    const reader = res.body.getReader()
    const decoder = new TextDecoder()
    let buffer = ''
    let sawDone = false
    let sawError = false
    for (;;) {
      const { value, done } = await reader.read()
      if (done) break
      buffer += decoder.decode(value, { stream: true })
      let idx
      while ((idx = buffer.indexOf('\n\n')) >= 0) {
        const rawEvent = buffer.slice(0, idx)
        buffer = buffer.slice(idx + 2)
        let event = 'message'
        let data = ''
        for (const line of rawEvent.split('\n')) {
          if (line.startsWith('event:')) event = line.slice(6).trim()
          else if (line.startsWith('data:')) data += line.slice(5).trim()
        }
        if (!data) continue
        let parsed: Record<string, unknown>
        try { parsed = JSON.parse(data) } catch { continue }
        if (event === 'status') {
          const msg = String(parsed.message ?? '')
          if (msg.startsWith('error:')) { sawError = true; handlers.onError?.(msg.slice(6).trim()) }
          else handlers.onStatus?.(msg)
        } else if (event === 'sources') {
          handlers.onSources?.((parsed.citations as Citation[]) ?? [], (parsed.mode as QueryMode) ?? payload.mode, parsed.verified === true)
        } else if (event === 'delta') {
          handlers.onDelta?.(String(parsed.text ?? ''))
        } else if (event === 'done') {
          sawDone = true
          handlers.onDone?.(String(parsed.answer ?? ''), (parsed.audit as CitationAudit) ?? null)
        }
      }
    }
    // A stream that already delivered a real error must not throw for
    // the missing Done — the caller would retry the whole (slow) query
    // as a "transport failure" and double the user's wait.
    if (!sawDone && !sawError) throw new Error('stream ended unexpectedly')
  }
  return attempt(false)
}
