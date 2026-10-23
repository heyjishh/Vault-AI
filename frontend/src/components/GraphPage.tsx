import { useMemo, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { ragApi } from '../api/client'
import type { Entity } from '../api/types'
import { EmptyState, Spinner } from './ui'

const W = 860
const H = 560
const KIND_COLORS: Record<string, string> = {
  statute: '#0f0e0d',
  act: '#0f0e0d',
  section: '#524f49',
  person: '#16a34a',
  organization: '#b45309',
  concept: '#706d66',
  place: '#0369a1',
  date: '#8f8b85',
}
const colorFor = (kind: string) => KIND_COLORS[kind.toLowerCase()] ?? '#706d66'

export default function GraphPage() {
  const { data, isLoading } = useQuery({ queryKey: ['graph'], queryFn: ragApi.graph })
  const [selected, setSelected] = useState<Entity | null>(null)
  const [kindFilter, setKindFilter] = useState<string>('all')

  const { positions, kinds } = useMemo(() => {
    const entities = data?.entities ?? []
    const relations = data?.relations ?? []
    const nameSet = new Set(entities.map((e) => e.name))
    const degree = new Map<string, number>()
    relations.forEach((r) => {
      degree.set(r.from, (degree.get(r.from) ?? 0) + 1)
      degree.set(r.to, (degree.get(r.to) ?? 0) + 1)
    })
    const sorted = [...entities].sort((a, b) => (degree.get(b.name) ?? 0) - (degree.get(a.name) ?? 0))
    const pos = new Map<string, { x: number; y: number; r: number }>()
    const cx = W / 2, cy = H / 2
    sorted.forEach((e, i) => {
      if (i === 0) { pos.set(e.name, { x: cx, y: cy, r: 9 }); return }
      // golden-angle spiral
      const angle = i * 2.39996
      const rad = 34 * Math.sqrt(i)
      pos.set(e.name, {
        x: cx + rad * Math.cos(angle),
        y: cy + rad * Math.sin(angle) * 0.72,
        r: Math.max(4, 7 - (degree.get(e.name) ?? 0) * 0.2),
      })
    })
    void nameSet
    const kinds = [...new Set(entities.map((e) => e.kind))]
    return { positions: pos, kinds }
  }, [data])

  const visible = (e: Entity) => kindFilter === 'all' || e.kind === kindFilter
  const rels = (data?.relations ?? []).filter((r) => {
    const a = data?.entities.find((e) => e.name === r.from)
    const b = data?.entities.find((e) => e.name === r.to)
    return a && b && visible(a) && visible(b)
  })

  return (
    <div className="max-w-5xl mx-auto px-6 py-10">
      <h1 className="font-display text-[2rem] tracking-tight">Knowledge Graph</h1>
      <p className="text-stone-500 mt-1.5 text-[15px]">
        Entities and relations extracted from your corpus. Click a node to inspect it.
      </p>

      <div className="mt-5 flex flex-wrap gap-1.5">
        {['all', ...kinds].map((k) => (
          <button
            key={k}
            onClick={() => setKindFilter(k)}
            className={`rounded-full px-3 py-1 text-meta font-medium border transition-colors ${
              kindFilter === k ? 'bg-ink text-ivory border-ink' : 'border-stone-200 bg-white text-stone-500 hover:text-ink'
            }`}
          >
            {k}
          </button>
        ))}
      </div>

      {isLoading ? (
        <Spinner label="Loading graph…" />
      ) : !data || data.entities.length === 0 ? (
        <div className="mt-6"><EmptyState title="Graph is empty" hint="Ingest documents in the Vault to extract entities and relations." /></div>
      ) : (
        <div className="mt-4 grid lg:grid-cols-[1fr_300px] gap-4">
          <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex overflow-hidden">
            <svg viewBox={`0 0 ${W} ${H}`} className="w-full h-auto block">
              {rels.map((r, i) => {
                const a = positions.get(r.from), b = positions.get(r.to)
                if (!a || !b) return null
                return <line key={i} x1={a.x} y1={a.y} x2={b.x} y2={b.y} stroke="#e5e5e3" strokeWidth={1} />
              })}
              {data.entities.filter(visible).map((e) => {
                const p = positions.get(e.name)
                if (!p) return null
                const isSel = selected?.name === e.name
                return (
                  <g key={e.name} onClick={() => setSelected(e)} className="cursor-pointer">
                    <circle cx={p.x} cy={p.y} r={isSel ? p.r + 3 : p.r} fill={colorFor(e.kind)} opacity={isSel ? 1 : 0.85} stroke={isSel ? '#0f0e0d' : '#fff'} strokeWidth={isSel ? 2 : 1} />
                    <text x={p.x} y={p.y - p.r - 4} textAnchor="middle" fontSize={10.5} fill="#524f49" className="select-none">
                      {e.name.length > 26 ? e.name.slice(0, 24) + '…' : e.name}
                    </text>
                  </g>
                )
              })}
            </svg>
          </div>

          <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex p-5 h-fit">
            {selected ? (
              <>
                <p className="text-meta uppercase tracking-wider font-medium" style={{ color: colorFor(selected.kind) }}>{selected.kind}</p>
                <h3 className="font-display text-xl mt-1 tracking-tight">{selected.name}</h3>
                <p className="text-meta2 text-stone-600 leading-relaxed mt-2">{selected.description || 'No description.'}</p>
                <p className="text-meta text-stone-400 mt-3">Appears in {selected.chunk_ids.length} chunk(s)</p>
                <div className="mt-4 border-t border-stone-200 pt-3">
                  <p className="text-meta uppercase tracking-wider text-stone-400 font-medium mb-2">Relations</p>
                  <div className="space-y-1.5">
                    {(data?.relations ?? [])
                      .filter((r) => r.from === selected.name || r.to === selected.name)
                      .slice(0, 8)
                      .map((r, i) => (
                        <p key={i} className="text-meta2 text-stone-600">
                          <span className="font-medium text-ink">{r.from}</span> → <span className="font-medium text-ink">{r.to}</span>
                          {r.description ? <span className="block text-stone-500 text-meta mt-0.5">{r.description}</span> : null}
                        </p>
                      ))}
                  </div>
                </div>
              </>
            ) : (
              <p className="text-meta2 text-stone-500">Select a node to see its description and relations.</p>
            )}
          </div>
        </div>
      )}
    </div>
  )
}
