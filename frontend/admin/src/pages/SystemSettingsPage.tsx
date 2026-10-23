import { useState } from 'react'
import { useMutation, useQuery } from '@tanstack/react-query'
import { Crosshair } from 'lucide-react'
import { adminApi, type HitTestReport } from '../api'
import { ErrorBanner, PageHead, Spinner, StatCard } from '../ui'

function hitRate(s: { hits: number; misses: number } | null | undefined): string {
  if (!s) return '—'
  const total = s.hits + s.misses
  if (total === 0) return 'no traffic yet'
  return `${Math.round((s.hits / total) * 100)}% hit rate`
}

const STAGE_LABELS: Record<string, string> = {
  vector: 'Vector (bge-m3)',
  bm25: 'BM25 keywords',
  fused: 'RRF fused pool',
  reranked: 'Reranked (cross-encoder)',
  'late-interaction': 'Late interaction (ColBERT MaxSim)',
}

function OnOff({ on }: { on: boolean }) {
  return (
    <span className={`rounded-full px-2 py-px text-[11px] font-semibold ${on ? 'bg-[#e7f4e4] text-[#2f6b2f]' : 'bg-stone-100 text-stone-400'}`}>
      {on ? 'ON' : 'OFF'}
    </span>
  )
}

function StackCard() {
  const { data: s } = useQuery({ queryKey: ['retrieval-status'], queryFn: adminApi.retrievalStatus })
  const { data: mcp } = useQuery({ queryKey: ['mcp-info'], queryFn: adminApi.mcpInfo })
  const [copied, setCopied] = useState(false)
  if (!s) return null
  const features: [string, boolean, string][] = [
    ['Index-time augmentation', s.augmentation, 'Keywords + generated questions per chunk, indexed as auxiliary match units'],
    ['Statute hyperedges', s.hyperedges, '{Act, section} as one retrievable unit; hits expand to member chunks'],
    ['HyDE / multi-query', s.hyde, 'Short citation-style questions expand into a hypothetical passage + variants'],
    ['ColBERT late interaction', s.late_interaction, 'Token-level MaxSim rescoring blended after the cross-encoder (needs LEGAL_RAG_COLBERT_URL)'],
    ['Parent-child chunking', s.parent_child, 'Small child spans are the match units; parents are the context'],
    ['BM25 keyword leg', s.bm25_enabled, `Engine: ${s.bm25_engine}`],
    ['Retrieval cache (SWR)', s.retrieval_cache, 'Fresh 120s + stale-while-revalidate, corpus-fingerprint keyed'],
    ['Semantic answer cache', s.semantic_cache, 'Paraphrase-level hits at zero LLM cost'],
  ]
  const mcpConfig = JSON.stringify({
    mcpServers: {
      'juris-vault-ai': {
        url: '<your-deployment>/mcp',
        headers: { Authorization: 'Bearer <user-api-token>' },
      },
    },
  }, null, 2)
  return (
    <>
      <div className="grid md:grid-cols-3 gap-3">
        <StatCard label="Vector store" value={s.vector_store} sub={`Fusion: ${s.fusion} · statute boost ×${s.statute_boost}`} />
        <StatCard label="Keyword engine" value={s.bm25_engine} sub={s.bm25_enabled ? 'BM25 leg active in fusion' : 'BM25 leg disabled'} />
        <StatCard label="Late interaction" value={s.late_interaction ? 'ColBERT on' : 'off'} sub="MaxSim blend after reranking" />
      </div>
      <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex divide-y divide-stone-200 mt-3">
        {features.map(([k, on, v]) => (
          <div key={k} className="flex items-center gap-4 px-5 py-3">
            <span className="w-52 flex-shrink-0 text-[13px] font-medium">{k}</span>
            <OnOff on={on} />
            <span className="text-[13px] text-stone-600">{v}</span>
          </div>
        ))}
      </div>

      <h2 className="font-display text-xl tracking-tight mt-10 mb-3">MCP server</h2>
      <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex p-5">
        <div className="flex items-center gap-3 flex-wrap">
          <p className="text-[14px] font-medium">Model Context Protocol — external clients connect as a user, with that user's access and quota.</p>
          <OnOff on={mcp?.enabled ?? s.mcp_enabled} />
        </div>
        <p className="text-[13px] text-stone-500 mt-1.5">
          Endpoint <code className="bg-stone-100 rounded px-1.5 py-0.5 text-[12.5px]">POST {mcp?.endpoint ?? '/mcp'}</code> ·
          transport {mcp?.transport ?? 'streamable-http'} · protocol {mcp?.protocol_version ?? '2024-11-05'}
        </p>
        <div className="flex flex-wrap gap-1.5 mt-3">
          {(mcp?.tools ?? []).map((t) => (
            <span key={t.name} title={t.description} className="rounded-full border border-stone-200 bg-[#fcfbf9] px-2.5 py-1 text-[12px] font-medium text-stone-600">
              {t.name}
            </span>
          ))}
        </div>
        <div className="relative mt-4">
          <pre className="rounded-lex bg-ink text-ivory/90 text-[12px] leading-relaxed p-4 overflow-x-auto">{mcpConfig}</pre>
          <button
            onClick={() => { navigator.clipboard?.writeText(mcpConfig); setCopied(true); setTimeout(() => setCopied(false), 1500) }}
            className="absolute top-2.5 right-2.5 rounded-lex bg-white/10 hover:bg-white/20 text-ivory px-2.5 py-1 text-[12px] font-medium"
          >
            {copied ? 'Copied ✓' : 'Copy config'}
          </button>
        </div>
      </div>
    </>
  )
}

function HitTestCard() {
  const [question, setQuestion] = useState('')
  const [report, setReport] = useState<HitTestReport | null>(null)
  const run = useMutation({
    mutationFn: () => adminApi.hitTest(question.trim(), 8),
    onSuccess: setReport,
  })
  return (
    <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex p-5">
      <h3 className="font-display text-lg tracking-tight flex items-center gap-2">
        <Crosshair size={17} /> Hit testing
      </h3>
      <p className="text-[13px] text-stone-500 mt-1">
        Run one question through every retrieval stage and compare the rankings — no answer is generated.
        Scores: cosine for vector, BM25 weight for keywords, the vector scale after fusion.
      </p>
      <div className="flex gap-2 mt-3">
        <input
          value={question}
          onChange={(e) => setQuestion(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && question.trim() && run.mutate()}
          placeholder="e.g. What does Section 86 of the Income Tax Act deal with?"
          className="flex-1 rounded-lex border border-stone-300 px-3 py-2 text-[14px] focus:outline-none focus:border-ink"
        />
        <button
          onClick={() => run.mutate()}
          disabled={!question.trim() || run.isPending}
          className="rounded-lex bg-ink text-ivory px-4 py-2 text-[14px] font-medium disabled:opacity-50"
        >
          {run.isPending ? 'Testing…' : 'Run'}
        </button>
      </div>
      {run.isError && <div className="mt-3"><ErrorBanner message={(run.error as Error).message} /></div>}
      {report && (
        <div className="grid md:grid-cols-2 xl:grid-cols-4 gap-3 mt-4">
          {report.stages.map((stage) => (
            <div key={stage.name} className="rounded-lex border border-stone-200 overflow-hidden">
              <p className="bg-stone-50 px-3 py-2 text-[12px] font-semibold uppercase tracking-wide text-stone-500">
                {STAGE_LABELS[stage.name] ?? stage.name}
              </p>
              {stage.hits.length === 0 && (
                <p className="px-3 py-2 text-[13px] text-stone-400">no hits</p>
              )}
              <ol>
                {stage.hits.map((h, i) => (
                  <li key={h.chunk_id} className="px-3 py-2 border-t border-stone-100">
                    <p className="text-[12px] text-stone-500">
                      <span className="font-semibold text-ink">#{i + 1}</span>{' '}
                      {h.score.toFixed(stage.name === 'bm25' ? 1 : 3)} · {h.doc_title ?? h.doc_id}
                      {h.is_child && <span className="ml-1 rounded bg-stone-100 px-1 py-px text-[10.5px]">child</span>}
                    </p>
                    <p className="text-[12.5px] text-stone-600 leading-snug mt-0.5 line-clamp-3">{h.snippet}</p>
                  </li>
                ))}
              </ol>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

export default function SystemSettingsPage() {
  const { data: cache } = useQuery({ queryKey: ['cache'], queryFn: adminApi.cache, refetchInterval: 15_000 })
  const { data: workflows, isLoading } = useQuery({ queryKey: ['workflows'], queryFn: adminApi.workflows })

  return (
    <div className="max-w-5xl mx-auto px-6 py-10">
      <PageHead title="Retrieval & Models" sub="How answers are produced in this workspace — providers, retrieval modes, reranking and caches." />

      <h2 className="font-display text-xl tracking-tight mt-8 mb-3">Semantic cache</h2>
      <div className="grid md:grid-cols-3 gap-3">
        <StatCard label="LLM cache" value={cache?.llm ? `${cache.llm.entries} entries` : 'off'}
          sub={cache?.llm ? `${cache.llm.hits} hits · ${cache.llm.misses} misses · ${hitRate(cache.llm)}` : undefined} />
        <StatCard label="Embeddings cache" value={cache?.embeddings ? `${cache.embeddings.entries} entries` : 'off'}
          sub={cache?.embeddings ? `${cache.embeddings.hits} hits · ${cache.embeddings.misses} misses · ${hitRate(cache.embeddings)}` : undefined} />
        <StatCard label="Answer cache" value={cache?.answers ? `${cache.answers.entries} entries` : '—'}
          sub={cache?.answers ? `${cache.answers.hits} paraphrase hits · ${cache.answers.misses} misses · ${hitRate(cache.answers)}` : undefined} />
      </div>

      <h2 className="font-display text-xl tracking-tight mt-10 mb-3">Retrieval stack — live status</h2>
      <StackCard />

      <h2 className="font-display text-xl tracking-tight mt-10 mb-3">Retrieval debugger</h2>
      <HitTestCard />

      <h2 className="font-display text-xl tracking-tight mt-10 mb-3">Pipeline</h2>
      <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex divide-y divide-stone-200">
        {[
          ['LLM provider', 'Cloudflare Workers AI — llama-3.3-70b-instruct (OpenAI-compatible endpoint)'],
          ['Embeddings', 'Cloudflare Workers AI — bge-m3 (1024-dim, multilingual)'],
          ['Vector store', 'In-memory by default; SereneDB when SERENEDB_URL is set (PG wire, FLOAT[N] + IVF cosine)'],
          ['Query modes', 'Naive · Local · Global · Hybrid · Mix (default) · Bypass — LightRAG'],
          ['Reranking', 'Local cross-encoder decision layer (bge-reranker-v2-m3 service, zero billable tokens); LLM listwise rerank optional as deep judge'],
          ['Multimodal', 'Image / table / equation processors describe content in context, indexed as entities'],
          ['PDF pipeline', 'Text layer first; sparse pages rendered and OCR’d with Surya'],
          ['Web search', 'Stealth-browser sidecar; live results blended as [web:N] citations'],
          ['Persistence', 'Atomic JSON snapshots every 5 min + on graceful shutdown'],
        ].map(([k, v]) => (
          <div key={k} className="flex gap-6 px-5 py-3.5">
            <span className="w-36 flex-shrink-0 text-[13px] font-medium">{k}</span>
            <span className="text-[13px] text-stone-600">{v}</span>
          </div>
        ))}
      </div>

      <h2 className="font-display text-xl tracking-tight mt-10 mb-3">Agent workflows</h2>
      {isLoading ? (
        <Spinner />
      ) : (
        <div className="grid md:grid-cols-3 gap-3">
          {(workflows ?? []).map((w) => (
            <div key={w.id} className="rounded-lex-lg border border-stone-200 bg-white shadow-lex p-5">
              <p className="font-display text-lg tracking-tight">{w.name}</p>
              <p className="text-[13px] text-stone-500 mt-1.5 leading-relaxed">{w.description}</p>
              <p className="text-[12px] text-stone-400 mt-3">{w.steps.length} steps: {w.steps.map((s) => s.name).join(' → ')}</p>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
