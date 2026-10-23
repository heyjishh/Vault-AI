import { useQuery } from '@tanstack/react-query'
import { adminApi } from '../api'
import { PageHead, Spinner, StatCard } from '../ui'

function hitRate(s: { hits: number; misses: number } | null | undefined): string {
  if (!s) return 'cache off'
  const total = s.hits + s.misses
  if (total === 0) return 'no traffic yet'
  return `${Math.round((s.hits / total) * 100)}% hit rate · ${s.hits}/${total}`
}

export default function OverviewPage() {
  const { data: stats } = useQuery({ queryKey: ['stats'], queryFn: adminApi.stats, refetchInterval: 15_000 })
  const { data: cache } = useQuery({ queryKey: ['cache'], queryFn: adminApi.cache, refetchInterval: 15_000 })
  const { data: users } = useQuery({ queryKey: ['users'], queryFn: adminApi.users })
  const { data: docs } = useQuery({ queryKey: ['docs'], queryFn: adminApi.docs })

  const admins = users?.filter((u) => u.role === 'admin').length ?? 0
  const recent = [...(docs ?? [])].sort((a, b) => b.ingested_at.localeCompare(a.ingested_at)).slice(0, 5)

  return (
    <div className="max-w-5xl mx-auto px-6 py-10">
      <PageHead title="Overview" sub="See, understand, act — workspace health at a glance (Harvey Command Center pattern)." />

      <div className="grid grid-cols-2 md:grid-cols-4 gap-3 mt-6">
        <StatCard label="Users" value={users?.length ?? '—'} sub={users ? `${admins} admin(s)` : undefined} />
        <StatCard label="Documents" value={stats?.documents ?? '—'} />
        <StatCard label="Chunks" value={stats?.chunks ?? '—'} sub="embedded passages" />
        <StatCard label="Graph" value={stats ? `${stats.entities} / ${stats.relations}` : '—'} sub="entities / relations" />
      </div>

      <div className="grid md:grid-cols-2 gap-3 mt-3">
        <StatCard label="LLM cache" value={cache?.llm ? `${cache.llm.entries} entries` : 'off'} sub={hitRate(cache?.llm)} />
        <StatCard label="Embeddings cache" value={cache?.embeddings ? `${cache.embeddings.entries} entries` : 'off'} sub={hitRate(cache?.embeddings)} />
      </div>

      <h2 className="font-display text-xl tracking-tight mt-10 mb-3">Recently ingested</h2>
      {!docs ? (
        <Spinner />
      ) : recent.length === 0 ? (
        <p className="text-[15px] text-stone-500">No documents yet. Ingest from Corpus &amp; Vault.</p>
      ) : (
        <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex overflow-hidden">
          {recent.map((d, i) => (
            <div key={d.id} className={`flex items-center justify-between px-5 py-3 ${i > 0 ? 'border-t border-stone-200' : ''}`}>
              <div className="min-w-0">
                <p className="text-[14px] font-medium truncate">{d.title}</p>
                <p className="text-[12px] text-stone-500">{d.chunks} chunks · {d.chars.toLocaleString()} chars</p>
              </div>
              <span className="text-[12px] text-stone-400 flex-shrink-0">
                {new Date(d.ingested_at).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
