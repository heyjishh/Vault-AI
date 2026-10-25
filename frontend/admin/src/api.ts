import axios, { AxiosError, InternalAxiosRequestConfig } from 'axios'

// Token keys intentionally match the user app so one login serves both interfaces.
const ACCESS_KEY = 'jurisvault.access'
const REFRESH_KEY = 'jurisvault.refresh'

export const tokens = {
  get access(): string | null { return localStorage.getItem(ACCESS_KEY) },
  get refresh(): string | null { return localStorage.getItem(REFRESH_KEY) },
  save(access: string, refresh: string) {
    localStorage.setItem(ACCESS_KEY, access)
    localStorage.setItem(REFRESH_KEY, refresh)
  },
  clear() {
    localStorage.removeItem(ACCESS_KEY)
    localStorage.removeItem(REFRESH_KEY)
  },
}

export interface Me { id: string; email: string; role: string }
export interface AdminUser { id: string; email: string; role: string; created_at: string }
export interface CorpusStats { documents: number; chunks: number; entities: number; relations: number }
export interface DocInfo {
  id: string; title: string; source_uri: string; chars: number; chunks: number; ingested_at: string
}
export interface CacheStats {
  llm: { hits: number; misses: number; entries: number } | null
  embeddings: { hits: number; misses: number; entries: number } | null
  answers: { hits: number; misses: number; entries: number }
}
export interface HitView {
  chunk_id: string; doc_id: string; doc_title: string | null
  score: number; snippet: string; is_child: boolean
}
export interface HitTestReport {
  question: string
  stages: { name: string; hits: HitView[] }[]
}
export interface WorkflowDef { id: string; name: string; description: string; steps: { id: string; name: string }[] }
export interface RetrievalStatus {
  vector_store: string
  fusion: string
  statute_boost: string
  bm25_engine: string
  bm25_enabled: boolean
  parent_child: boolean
  augmentation: boolean
  hyperedges: boolean
  hyde: boolean
  late_interaction: boolean
  retrieval_cache: boolean
  semantic_cache: boolean
  mcp_enabled: boolean
}
export interface McpInfo {
  enabled: boolean
  endpoint: string
  transport: string
  protocol_version: string
  tools: { name: string; description: string }[]
}
export interface BillingPayment {
  id: string; order_id: string; user_email: string; plan_id: string; amount_inr: number
  provider: string; status: string; utr: string | null; created_at: string; completed_at: string | null
}
export interface UserBillingRow {
  user_id: string; email: string; role: string; plan_id: string; plan_name: string
  subscription_status: string; period_end: string; tokens_used: number; token_quota: number; spent_inr: number
}
export interface BillingOverview {
  total_users: number; active_subscriptions: number; mrr_inr: number; collected_inr: number
  payments_count: number; users: UserBillingRow[]; recent_payments: BillingPayment[]
}
export interface BillingSettings {
  provider: string; youpi_base_url: string; youpi_merchant_id: string; webhook_secret_set: boolean
}
export interface ActivityEvent {
  id: string; user_id: string; user_email: string; kind: string; title: string; detail: string
  result_excerpt: string; citations: number; tokens: number; created_at: string
}
export interface DayPoint { date: string; events: number; tokens: number }
export interface UserInsight {
  user_id: string; email: string; role: string; plan_name: string; events: number; queries: number
  researches: number; tokens_activity: number; tokens_billed: number; token_quota: number
  spent_inr: number; last_active: string | null
}
export interface AdminInsights {
  total_users: number; total_events: number; total_queries: number; total_tokens_billed: number
  documents: number; chunks: number; mrr_inr: number; collected_inr: number; active_subscriptions: number
  series_14d: DayPoint[]; plan_distribution: { plan: string; users: number }[]
  per_user: UserInsight[]; top_queries: { question: string; count: number; last_asked: string }[]
  recent_activity: ActivityEvent[]
}
export interface GoldenRule {
  id: string; question: string; expected_terms: string[]; expected_sources: string[]
  notes: string; source: string; created_at: string
  last_result: {
    passed: boolean; missing_terms: string[]; missing_sources: string[]
    answer_excerpt: string; citations: number; ran_at: string
  } | null
}
export interface AnnotationRecord {
  id: string; question: string; answer: string
  citations: { doc_id: string; doc_title: string | null; chunk_id: string; ordinal: number; score: number; snippet: string }[]
  scope_doc_ids: string[] | null; corpus_fp: string; source: string
  created_by: string; created_at: string; enabled: boolean
}
export interface GoldenRunResult {
  ran_at: string; total: number; passed: number
  results: { rule_id: string; question: string; result: NonNullable<GoldenRule['last_result']> }[]
}

export const api = axios.create({ baseURL: '/api/v1' })

api.interceptors.request.use((config: InternalAxiosRequestConfig) => {
  const t = tokens.access
  if (t) config.headers.Authorization = `Bearer ${t}`
  return config
})

api.interceptors.response.use(
  (r) => r,
  async (error: AxiosError) => {
    const original = error.config as (InternalAxiosRequestConfig & { _retried?: boolean }) | undefined
    if (error.response?.status === 401 && original && !original._retried && tokens.refresh) {
      original._retried = true
      try {
        const r = await axios.post('/api/v1/auth/refresh', { refresh_token: tokens.refresh })
        tokens.save(r.data.access_token, r.data.refresh_token ?? tokens.refresh)
        original.headers.Authorization = `Bearer ${r.data.access_token}`
        return api(original)
      } catch {
        tokens.clear()
        window.location.reload()
      }
    }
    return Promise.reject(error)
  },
)

export function errMsg(e: unknown): string {
  if (axios.isAxiosError(e)) {
    const d = e.response?.data as { error?: string } | undefined
    return d?.error ?? e.message
  }
  return String(e)
}

export const adminApi = {
  async login(email: string, password: string): Promise<Me> {
    const r = await api.post('/auth/login', { email, password })
    tokens.save(r.data.tokens.access_token, r.data.tokens.refresh_token)
    return r.data.user as Me
  },
  async me(): Promise<Me> {
    const r = await api.get<Me>('/auth/me')
    return r.data
  },
  async users(): Promise<AdminUser[]> {
    const r = await api.get<AdminUser[]>('/admin/users')
    return r.data
  },
  async setRole(id: string, role: string): Promise<AdminUser> {
    const r = await api.post<AdminUser>(`/admin/users/${id}/role`, { role })
    return r.data
  },
  async deleteUser(id: string): Promise<void> {
    await api.delete(`/admin/users/${id}`)
  },
  async stats(): Promise<CorpusStats> {
    const r = await api.get<CorpusStats>('/knowledge/stats')
    return r.data
  },
  async cache(): Promise<CacheStats> {
    const r = await api.get<CacheStats>('/knowledge/cache-stats')
    return r.data
  },
  async docs(): Promise<DocInfo[]> {
    const r = await api.get<DocInfo[]>('/vault/documents')
    return r.data
  },
  async deleteDoc(id: string): Promise<void> {
    await api.delete(`/vault/documents/${id}`)
  },
  async ingestS3(prefix?: string): Promise<{ documents: number; chunks: number; entities: number }> {
    const r = await api.post('/vault/ingest', null, { params: prefix ? { prefix } : {} })
    return r.data
  },
  async workflows(): Promise<WorkflowDef[]> {
    const r = await api.get<WorkflowDef[]>('/knowledge/workflows')
    return r.data
  },
  async billingOverview(): Promise<BillingOverview> {
    const r = await api.get<BillingOverview>('/admin/billing/overview')
    return r.data
  },
  async billingSettings(): Promise<BillingSettings> {
    const r = await api.get<BillingSettings>('/admin/billing/settings')
    return r.data
  },
  async updateBillingSettings(body: {
    provider: string; youpi_base_url: string; youpi_merchant_id: string; youpi_webhook_secret?: string
  }): Promise<BillingSettings> {
    try {
      const r = await api.put<BillingSettings>('/admin/billing/settings', body)
      return r.data
    } catch (e) { throw new Error(errMsg(e)) }
  },
  async insights(): Promise<AdminInsights> {
    const r = await api.get<AdminInsights>('/admin/insights')
    return r.data
  },
  async userActivity(userId: string): Promise<ActivityEvent[]> {
    const r = await api.get<ActivityEvent[]>(`/admin/insights/users/${userId}/activity`)
    return r.data
  },
  async goldenRules(): Promise<{ rules: GoldenRule[]; last_run: { ran_at: string; total: number; passed: number } | null }> {
    const r = await api.get('/admin/golden/rules')
    return r.data
  },
  async addGoldenRule(body: { question: string; expected_terms: string[]; expected_sources: string[]; notes: string }): Promise<GoldenRule> {
    try {
      const r = await api.post<GoldenRule>('/admin/golden/rules', body)
      return r.data
    } catch (e) { throw new Error(errMsg(e)) }
  },
  async goldenFromActivity(activity_id: string): Promise<GoldenRule> {
    try {
      const r = await api.post<GoldenRule>('/admin/golden/rules/from-activity', { activity_id })
      return r.data
    } catch (e) { throw new Error(errMsg(e)) }
  },
  async deleteGoldenRule(id: string): Promise<void> {
    await api.delete(`/admin/golden/rules/${id}`)
  },
  async updateGoldenRule(id: string, body: { question?: string; expected_terms?: string[]; expected_sources?: string[]; notes?: string }): Promise<GoldenRule> {
    try {
      const r = await api.put<GoldenRule>(`/admin/golden/rules/${id}`, body)
      return r.data
    } catch (e) { throw new Error(errMsg(e)) }
  },
  async hitTest(question: string, top_k = 8): Promise<HitTestReport> {
    try {
      const r = await api.post<HitTestReport>('/admin/hit-test', { question, top_k })
      return r.data
    } catch (e) { throw new Error(errMsg(e)) }
  },
  async retrievalStatus(): Promise<RetrievalStatus> {
    const r = await api.get<RetrievalStatus>('/admin/retrieval-status')
    return r.data
  },
  async mcpInfo(): Promise<McpInfo> {
    const r = await api.get<McpInfo>('/admin/mcp')
    return r.data
  },
  async annotations(): Promise<{ annotations: AnnotationRecord[]; current_corpus_fp: string }> {
    const r = await api.get('/admin/annotations')
    return r.data
  },
  async addAnnotation(body: { question: string; answer: string }): Promise<AnnotationRecord> {
    try {
      const r = await api.post<AnnotationRecord>('/admin/annotations', body)
      return r.data
    } catch (e) { throw new Error(errMsg(e)) }
  },
  async annotationFromActivity(activity_id: string, answer?: string): Promise<AnnotationRecord> {
    try {
      const r = await api.post<AnnotationRecord>('/admin/annotations/from-activity', { activity_id, answer })
      return r.data
    } catch (e) { throw new Error(errMsg(e)) }
  },
  async deleteAnnotation(id: string): Promise<void> {
    await api.delete(`/admin/annotations/${id}`)
  },
  async toggleAnnotation(id: string, enabled: boolean): Promise<void> {
    await api.post(`/admin/annotations/${id}/toggle`, { enabled })
  },
  async runGolden(): Promise<GoldenRunResult> {
    try {
      const r = await api.post<GoldenRunResult>('/admin/golden/run')
      return r.data
    } catch (e) { throw new Error(errMsg(e)) }
  },
  async settings(): Promise<{
    settings: PanelSettings
    defaults: Record<string, string>
    agent_workflows: { id: string; name: string; report_instruction: string }[]
    effective: { llm_model: string; embedding_model: string; default_mode: string; prompts_active: Record<string, boolean> }
  }> {
    const r = await api.get('/admin/settings')
    return r.data
  },
  async updateSettings(body: PanelSettings): Promise<{ ok: boolean; settings: PanelSettings }> {
    try {
      const r = await api.put('/admin/settings', body)
      return r.data
    } catch (e) { throw new Error(errMsg(e)) }
  },
}

export interface UiTab {
  id: string
  label: string
  kind: 'link' | 'page'
  url?: string
  content?: string
  visible: boolean
}

export interface PanelPrompts {
  ask?: string | null
  interact?: string | null
  draft?: string | null
  research_frame?: string | null
  research_adjudicate?: string | null
  agent?: string | null
}

export interface PanelSettings {
  default_mode: string
  prompts: PanelPrompts
  llm_model?: string | null
  embedding_model?: string | null
  brand: { product_name: string; tagline: string }
  tabs: UiTab[]
}
