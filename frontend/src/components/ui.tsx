import { useMemo, useState } from 'react'
import { createPortal } from 'react-dom'
import { BookOpen, Gavel, Moon, PenLine, Scale, Sparkles, Sun, Trash2, X } from 'lucide-react'
import type { Citation } from '../api/types'
import { SourceModal } from './SourceModal'
import { DocInsightButton } from './DocInsight'

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

/**
 * Piece 33 (consent revision): the one consent dialog for every
 * permanent delete in the app — history entries, documents, briefs.
 * Nothing destructive fires without this dialog's explicit
 * "Delete permanently" click. Rendered via portal; Escape and
 * backdrop click cancel.
 */
export function ConfirmDialog({
  title, body, confirmLabel = 'Delete permanently', busy, onConfirm, onCancel,
}: {
  title: string
  body: string
  confirmLabel?: string
  busy?: boolean
  onConfirm: () => void
  onCancel: () => void
}) {
  /* Piece 47: typed confirmation — the delete button stays disabled
     until the user types DELETE, so a permanent delete can never
     happen on a stray click. */
  const [typed, setTyped] = useState('')
  const armed = typed.trim() === 'DELETE'
  return createPortal(
    <div
      className="fixed inset-0 z-[80] bg-ink/40 flex items-center justify-center p-4"
      onClick={() => !busy && onCancel()}
    >
      <div
        className="dialog-pop w-full max-w-md rounded-lex-lg bg-white dark:bg-[#0a1a12] shadow-lex-lg overflow-hidden"
        onClick={(e) => e.stopPropagation()}
        role="alertdialog"
        aria-modal="true"
        aria-label={title}
      >
        <div className="flex items-start gap-3 px-6 pt-5">
          <Trash2 size={17} className="text-[#a02c2c] shrink-0 mt-0.5" />
          <div className="min-w-0 flex-1">
            <h3 className="font-display text-[17px] leading-snug">{title}</h3>
            <p className="text-[13.5px] text-stone-500 dark:text-stone-400 mt-1 leading-relaxed">{body}</p>
          </div>
          <button
            onClick={() => !busy && onCancel()}
            className="p-1 rounded text-stone-400 dark:text-stone-500 hover:text-ink transition-colors"
            aria-label="Cancel"
          >
            <X size={16} />
          </button>
        </div>
        <div className="px-6 pt-4">
          <label className="block">
            <span className="text-[12.5px] font-medium text-stone-500 dark:text-stone-400">
              Type <span className="font-bold text-[#a02c2c] tracking-wide">DELETE</span> to confirm
            </span>
            <input
              type="text"
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
              onKeyDown={(e) => { if (e.key === 'Enter' && armed && !busy) onConfirm() }}
              placeholder="DELETE"
              autoFocus
              autoComplete="off"
              spellCheck={false}
              className="mt-1.5 w-full rounded-lex border border-stone-300 dark:border-white/15 bg-transparent px-3 py-2 text-[13.5px] font-medium tracking-[0.18em] outline-none placeholder:text-stone-300 dark:placeholder:text-stone-600 focus:border-[#a02c2c]"
            />
          </label>
        </div>
        <div className="flex justify-end gap-2 px-6 py-4 mt-2 bg-ivory dark:bg-[#14120f] border-t border-stone-100 dark:border-white/10">
          <button
            onClick={() => !busy && onCancel()}
            className="rounded-lex border border-stone-300 dark:border-white/10 px-4 py-1.5 text-[13px] font-medium text-stone-600 dark:text-stone-300 hover:bg-stone-100 dark:hover:bg-white/10 transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={onConfirm}
            disabled={busy || !armed}
            className="rounded-lex bg-[#a02c2c] px-4 py-1.5 text-[13px] font-semibold text-white hover:bg-[#8c2626] disabled:opacity-50 transition-colors"
          >
            {busy ? 'Deleting…' : confirmLabel}
          </button>
        </div>
      </div>
    </div>,
    document.body,
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
      className={`rounded-lex-lg border bg-white dark:bg-[#0a1a12] transition-colors cursor-pointer ${active ? 'border-ink' : 'border-stone-200 dark:border-white/10 hover:border-stone-300'}`}
      onClick={() => setModalOpen(true)}
      title="View the full document"
    >
      <div className="flex items-start gap-3 px-4 py-3">
        <span className="mt-0.5 flex-shrink-0 text-meta2 tabular-nums text-stone-500 dark:text-stone-400">
          {Math.round(citation.score * 100)}%
        </span>
        <div className="min-w-0">
          <p className="text-meta2 font-medium text-ink dark:text-[#f4f2ee] truncate flex items-center gap-1.5">
            {citation.doc_title ?? citation.doc_id}
            {citation.doc_id.startsWith('private:') && (
              <span className="text-[11px] text-stone-500 dark:text-stone-400 shrink-0">
                Private
              </span>
            )}
            {citation.stale && (
              <span
                title={citation.stale}
                className="inline-flex items-center gap-1.5 text-[11px] text-stone-600 dark:text-stone-300 shrink-0"
              >
                <span className="w-1.5 h-1.5 rounded-full bg-[#b7791f] shrink-0" />
                Superseded
              </span>
            )}
          </p>
          <p className="text-meta2 text-stone-600 dark:text-stone-300 leading-relaxed mt-0.5 line-clamp-2">
            {citation.sentence ?? citation.snippet}
          </p>
        </div>
      </div>
      {citation.doc_id.startsWith('doc:') && !citation.source_url && (
        <div className="border-t border-stone-100 dark:border-white/10 px-4 py-2">
          <DocInsightButton citation={citation} />
        </div>
      )}
      {modalOpen && (
        <SourceModal citation={citation} onClose={() => setModalOpen(false)} go={go} />
      )}
    </div>
  )
}

export function StatusBadge({ status }: { status: string }) {
  // Piece 34: status is a small coloured dot + sentence-case word —
  // the meaning survives, the tinted pill does not.
  const dots: Record<string, string> = {
    supported: '#16a34a',
    refuted: '#a02c2c',
    inconclusive: '#b7791f',
    ready: '#16a34a',
  }
  const dot = dots[status.toLowerCase()] ?? '#8f8b85'
  return (
    <span className="inline-flex items-center gap-1.5 text-[12px] text-stone-700">
      <span className="w-1.5 h-1.5 rounded-full shrink-0" style={{ background: dot }} />
      {status}
    </span>
  )
}

export function StatCard({ label, value, sub }: { label: string; value: string | number; sub?: string }) {
  // Piece 34: a ledger KPI — bare type, no tile. Pages place these in
  // one hairline-divided strip (see MatterPage overview).
  // Piece 45: card-hover adds the shared hover lift.
  return (
    <div className="card-hover px-5 py-4">
      <p className="text-[13px] text-stone-500 dark:text-stone-400">{label}</p>
      <p className="text-[24px] font-medium mt-1 tabular-nums">{value}</p>
      {sub && <p className="text-meta text-stone-500 dark:text-stone-400 mt-1">{sub}</p>}
    </div>
  )
}

export function EmptyState({ title, hint }: { title: string; hint?: string }) {
  return (
    <div className="px-6 py-10 text-center">
      <p className="text-lg font-medium text-ink-800 dark:text-[#e8e5e0]">{title}</p>
      {hint && <p className="text-meta2 text-stone-500 dark:text-stone-400 mt-1.5">{hint}</p>}
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
  const btn = 'inline-flex items-center gap-1 rounded-lex border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] px-3 py-1.5 text-[12.5px] text-stone-600 dark:text-stone-300 hover:text-ink hover:border-stone-300 disabled:opacity-40 disabled:hover:text-stone-600 transition-colors'
  return (
    <div className="flex items-center gap-3 mt-3">
      <span className="text-[12.5px] text-stone-400 dark:text-stone-500">Showing {from}–{to} of {total}</span>
      <span className="ml-auto flex items-center gap-1.5">
        <button className={btn} disabled={page <= 1} onClick={() => onPage(page - 1)}>
          <ChevronLeft size={13} /> Prev
        </button>
        <span className="text-[12.5px] text-stone-500 dark:text-stone-400 px-1">Page {page} of {pages}</span>
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
          className={`rounded-lex px-3 py-1.5 text-[12.5px] font-medium transition-colors ${
            value === o.id ? 'bg-ink text-ivory' : 'border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] text-stone-500 dark:text-stone-400 hover:text-ink hover:border-stone-300'
          }`}
        >
          {o.label}
        </button>
      ))}
    </div>
  )
}

/**
 * Piece 43: the brand mark is the jurisvault.ai logo itself — the
 * green shield-and-scales emblem (public/logo.png, the same file the
 * main website ships as its header logo). The PNG sits on a white
 * ground, so it is presented in the site's own treatment: a white
 * rounded chip with a soft shadow. (Piece 37's drawn medallion is
 * retired.)
 */
export function BrandMark({ size = 36 }: { size?: number }) {
  return (
    <span
      aria-hidden="true"
      className="inline-flex bg-white rounded-lg p-0.5 shadow-sm shadow-black/10 shrink-0"
    >
      <img
        src="/logo.png"
        alt=""
        draggable={false}
        className="object-contain rounded-[5px]"
        style={{ width: size, height: size }}
      />
    </span>
  )
}

/**
 * Piece 35: dark-mode toggle. The initial class is set pre-paint by
 * the inline script in index.html (stored `jurisvault.theme`, else
 * the system preference); this button flips it and persists the
 * choice. Styled like the shell's other icon buttons.
 */
export function ThemeToggle({ className = '' }: { className?: string }) {
  const [dark, setDark] = useState(
    () => typeof document !== 'undefined' && document.documentElement.classList.contains('dark'),
  )
  function toggle() {
    const next = !dark
    setDark(next)
    document.documentElement.classList.toggle('dark', next)
    try {
      localStorage.setItem('jurisvault.theme', next ? 'dark' : 'light')
    } catch { /* storage unavailable — theme lasts for the session */ }
  }
  return (
    <button
      onClick={toggle}
      title={dark ? 'Switch to light mode' : 'Switch to dark mode'}
      aria-label="Toggle dark mode"
      className={`p-2 rounded-lex text-stone-500 dark:text-stone-400 hover:bg-stone-100 dark:hover:bg-white/10 hover:text-ink transition-colors ${className}`}
    >
      {dark ? <Sun size={16} /> : <Moon size={16} />}
    </button>
  )
}

/**
 * Piece 36: the moving glow layer — the taste-skill page's hero-glow
 * recipe (index.css: background var(--grad-glow)) drifting slowly over
 * the page wash on the 26s dualDrift loop, stilled by reduced motion.
 * Purely decorative. Default is absolutely positioned (parent must be
 * relative + overflow-hidden); `fixed` pins it to the viewport, as in
 * AppShell where one instance glows behind the whole app.
 */
export function DualToneBackdrop({ fixed = false }: { fixed?: boolean }) {
  return (
    <div
      aria-hidden="true"
      className={`dual-tone-backdrop${fixed ? ' dual-tone-backdrop-fixed' : ''}`}
    >
      {/* Piece 44: the rotating conic sheen (CSS in index.css). */}
      <div className="backdrop-spin" aria-hidden />
    </div>
  )
}

/**
 * Piece 42: the jurisvault.ai auth scene — the brand site's running
 * sign-in background (deep-emerald base, 1px grid, a pulsing core
 * glow, twinkling spark dots), ported 1:1 (recipes in index.css).
 * Piece 44: the conic rings became the site's orbit system (dashed
 * outer orbit + plain inner orbit) with its five floating labelled
 * nodes. Rendered only by
 * the sign-in screen, whose root carries `dark auth-scene`, so the
 * scene is always dark whatever the app theme. Purely decorative:
 * every layer is pointer-events-none + aria-hidden.
 * Spark tuples: [left %, top %, twinkle delay s, size px].
 */
const AUTH_SPARKS: ReadonlyArray<readonly [number, number, number, number]> = [
  [8, 18, 0, 3], [16, 72, 0.6, 2], [24, 34, 1.4, 3], [33, 86, 0.3, 2],
  [42, 12, 2.1, 2], [55, 64, 1.0, 3], [63, 26, 1.8, 2], [71, 80, 0.9, 3],
  [78, 44, 2.6, 2], [86, 16, 0.2, 3], [91, 58, 1.2, 2], [95, 88, 2.2, 2],
  [48, 44, 3.0, 2], [5, 52, 1.6, 2],
]

/* Piece 44: the site's five floating auth nodes — positions,
   delays and labels are jurisvault.ai's own. */
const AUTH_NODES = [
  { label: 'AI Research', left: '60%', top: '9%', delay: '0s', Icon: Sparkles },
  { label: 'Case Laws', left: '13%', top: '29%', delay: '1.1s', Icon: Scale },
  { label: 'Drafts', left: '87%', top: '35%', delay: '2.2s', Icon: PenLine },
  { label: 'Statutes', left: '8%', top: '62%', delay: '3.3s', Icon: BookOpen },
  { label: 'Judgments', left: '77%', top: '66%', delay: '4.4s', Icon: Gavel },
] as const

export function AuthScene() {
  return (
    <>
      <div aria-hidden="true" className="auth-scene-base" />
      <div aria-hidden="true" className="auth-grid" />
      {/* Piece 44: the site's orbit system (dashed outer orbit 46s,
          plain inner orbit 68s reverse) replaces the conic rings. */}
      <div className="auth-orbit-wrap" aria-hidden>
        <div className="auth-orbit-box">
          <div className="auth-orbit auth-orbit-dashed" />
          <div className="auth-orbit auth-orbit-inner" />
        </div>
      </div>
      {/* Piece 44: floating labelled nodes, large screens only. */}
      <div aria-hidden="true">
        {AUTH_NODES.map(({ label, left, top, delay, Icon }) => (
          <div
            key={label}
            className="auth-node hidden lg:flex"
            style={{ left, top, animationDelay: delay }}
          >
            <span className="auth-node-dot">
              <Icon size={22} strokeWidth={1.6} />
            </span>
            <span className="auth-node-label">{label}</span>
          </div>
        ))}
      </div>
      <div aria-hidden="true" className="auth-core" />
      {AUTH_SPARKS.map(([left, top, delay, size], i) => (
        <span
          key={i}
          aria-hidden="true"
          className="auth-spark"
          style={{
            left: `${left}%`,
            top: `${top}%`,
            width: size,
            height: size,
            animationDelay: `${delay}s`,
          }}
        />
      ))}
    </>
  )
}

export function Spinner({ label }: { label?: string }) {
  return (
    <div className="flex items-center gap-3 text-stone-500 dark:text-stone-400 py-6 justify-center">
      <span className="flex gap-1">
        <span className="dot-pulse w-1.5 h-1.5 rounded-full bg-stone-500 inline-block" />
        <span className="dot-pulse w-1.5 h-1.5 rounded-full bg-stone-500 inline-block" />
        <span className="dot-pulse w-1.5 h-1.5 rounded-full bg-stone-500 inline-block" />
      </span>
      {label && <span className="text-meta2">{label}</span>}
    </div>
  )
}
