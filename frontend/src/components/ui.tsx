import { useMemo, useState } from 'react'
import type { Citation } from '../api/types'
import { SourceModal } from './SourceModal'

/** Answer text with inline [doc:N] / [web:N] refs rendered as clickable chips (Harvey style). */
export function AnswerText({
  text, citations, onCite,
}: {
  text: string
  citations: Citation[]
  onCite?: (c: Citation) => void
}) {
  const parts = useMemo(() => text.split(/(\[(?:doc|web):\d+\])/g), [text])
  return (
    <span className="whitespace-pre-wrap leading-relaxed">
      {parts.map((part, i) => {
        const m = part.match(/^\[(doc|web):(\d+)\]$/)
        if (!m) return <span key={i}>{part}</span>
        const idx = parseInt(m[2], 10) - 1
        const src = citations[idx]
        return (
          <button
            key={i}
            className="cite-chip"
            title={src ? src.snippet.slice(0, 220) : part}
            onClick={() => src && onCite?.(src)}
          >
            {m[1]}:{m[2]}
          </button>
        )
      })}
    </span>
  )
}

export function CitationCard({ citation, active, go }: {
  citation: Citation
  active?: boolean
  go?: (view: string) => void
}) {
  const [modalOpen, setModalOpen] = useState(false)
  return (
    <div
      className={`rounded-lex-lg border bg-white shadow-lex transition-colors cursor-pointer ${active ? 'border-ink' : 'border-stone-200 hover:border-stone-300'}`}
      onClick={() => setModalOpen(true)}
      title="View the full document"
    >
      <div className="flex items-start gap-3 px-4 py-3">
        <span className="mt-0.5 flex-shrink-0 rounded bg-stone-100 border border-stone-200 px-1.5 py-0.5 text-[11px] font-mono font-semibold text-stone-600">
          {Math.round(citation.score * 100)}%
        </span>
        <div className="min-w-0">
          <p className="text-meta2 font-medium text-ink truncate flex items-center gap-1.5">
            {citation.doc_title ?? citation.doc_id}
            {citation.doc_id.startsWith('private:') && (
              <span className="inline-flex items-center gap-1 rounded-full bg-ink text-ivory px-1.5 py-px text-[10px] font-semibold uppercase tracking-wide shrink-0">
                Private
              </span>
            )}
            {citation.stale && (
              <span
                title={citation.stale}
                className="inline-flex items-center gap-1 rounded-full bg-amber-50 border border-amber-600/30 text-amber-800 px-1.5 py-px text-[10px] font-semibold uppercase tracking-wide shrink-0"
              >
                Superseded
              </span>
            )}
          </p>
          <p className="text-meta2 text-stone-600 leading-relaxed mt-0.5 line-clamp-2">
            {citation.sentence ?? citation.snippet}
          </p>
        </div>
      </div>
      {modalOpen && (
        <SourceModal citation={citation} onClose={() => setModalOpen(false)} go={go} />
      )}
    </div>
  )
}

export function StatusBadge({ status }: { status: string }) {
  const map: Record<string, string> = {
    supported: 'bg-green-50 text-green-700 border-green-200',
    refuted: 'bg-[#feeaea] text-[#a02c2c] border-[#f26161]/30',
    inconclusive: 'bg-amber-50 text-amber-700 border-amber-200',
    ready: 'bg-green-50 text-green-700 border-green-200',
  }
  const cls = map[status.toLowerCase()] ?? 'bg-stone-100 text-stone-600 border-stone-200'
  return (
    <span className={`inline-flex items-center rounded-full border px-2 py-0.5 text-[11px] font-semibold uppercase tracking-wide ${cls}`}>
      {status}
    </span>
  )
}

export function StatCard({ label, value, sub }: { label: string; value: string | number; sub?: string }) {
  return (
    <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-5 py-4">
      <p className="text-meta uppercase tracking-wider text-stone-500 font-medium">{label}</p>
      <p className="font-display text-3xl mt-1.5 tracking-tight">{value}</p>
      {sub && <p className="text-meta text-stone-500 mt-1">{sub}</p>}
    </div>
  )
}

export function EmptyState({ title, hint }: { title: string; hint?: string }) {
  return (
    <div className="rounded-lex-lg border border-dashed border-stone-300 bg-white/50 px-6 py-10 text-center">
      <p className="font-display text-lg text-ink-800">{title}</p>
      {hint && <p className="text-meta2 text-stone-500 mt-1.5">{hint}</p>}
    </div>
  )
}

/* ——— List controls: pagination + date-range filter ——— */

import { useMemo as _useMemo, useState as _useState } from 'react'
import { ChevronLeft, ChevronRight } from 'lucide-react'

/** Slice a list into pages. Resets/clamps the page when the list shrinks. */
export function usePagination<T>(items: T[], pageSize: number) {
  const [page, setPage] = _useState(1)
  const pages = Math.max(1, Math.ceil(items.length / pageSize))
  const safePage = Math.min(page, pages)
  const pageItems = _useMemo(
    () => items.slice((safePage - 1) * pageSize, safePage * pageSize),
    [items, safePage, pageSize],
  )
  const from = items.length === 0 ? 0 : (safePage - 1) * pageSize + 1
  const to = Math.min(items.length, safePage * pageSize)
  return { page: safePage, setPage, pages, pageItems, from, to, total: items.length }
}

export function Pager({
  page, pages, from, to, total, onPage,
}: {
  page: number; pages: number; from: number; to: number; total: number; onPage: (p: number) => void
}) {
  if (pages <= 1) return null
  const btn = 'inline-flex items-center gap-1 rounded-full border border-stone-200 bg-white px-3 py-1.5 text-[12.5px] text-stone-600 hover:text-ink hover:border-stone-300 disabled:opacity-40 disabled:hover:text-stone-600 transition-colors'
  return (
    <div className="flex items-center gap-3 mt-3">
      <span className="text-[12.5px] text-stone-400">Showing {from}–{to} of {total}</span>
      <span className="ml-auto flex items-center gap-1.5">
        <button className={btn} disabled={page <= 1} onClick={() => onPage(page - 1)}>
          <ChevronLeft size={13} /> Prev
        </button>
        <span className="text-[12.5px] text-stone-500 px-1">Page {page} of {pages}</span>
        <button className={btn} disabled={page >= pages} onClick={() => onPage(page + 1)}>
          Next <ChevronRight size={13} />
        </button>
      </span>
    </div>
  )
}

export type DateRange = 'all' | 'today' | 'week' | 'month'

export function dateInRange(iso: string, range: DateRange): boolean {
  if (range === 'all') return true
  const d = new Date(iso)
  const now = new Date()
  if (range === 'today') return d.toDateString() === now.toDateString()
  const days = range === 'week' ? 7 : 30
  return now.getTime() - d.getTime() <= days * 24 * 3600 * 1000
}

export function DateChips({ value, onChange }: { value: DateRange; onChange: (r: DateRange) => void }) {
  const opts: { id: DateRange; label: string }[] = [
    { id: 'all', label: 'All time' },
    { id: 'today', label: 'Today' },
    { id: 'week', label: 'This week' },
    { id: 'month', label: 'This month' },
  ]
  return (
    <div className="flex flex-wrap gap-1.5">
      {opts.map((o) => (
        <button
          key={o.id}
          onClick={() => onChange(o.id)}
          className={`rounded-full px-3 py-1.5 text-[12.5px] font-medium transition-colors ${
            value === o.id ? 'bg-ink text-ivory' : 'border border-stone-200 bg-white text-stone-500 hover:text-ink hover:border-stone-300'
          }`}
        >
          {o.label}
        </button>
      ))}
    </div>
  )
}

export function Spinner({ label }: { label?: string }) {
  return (
    <div className="flex items-center gap-3 text-stone-500 py-6 justify-center">
      <span className="flex gap-1">
        <span className="dot-pulse w-1.5 h-1.5 rounded-full bg-stone-500 inline-block" />
        <span className="dot-pulse w-1.5 h-1.5 rounded-full bg-stone-500 inline-block" />
        <span className="dot-pulse w-1.5 h-1.5 rounded-full bg-stone-500 inline-block" />
      </span>
      {label && <span className="text-meta2">{label}</span>}
    </div>
  )
}
