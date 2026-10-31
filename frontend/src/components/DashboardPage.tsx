import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import {
  ArrowRight, ArrowUp, Bot, Briefcase, FileSearch, FlaskConical, Lock, MessageSquare, PenLine, Sparkles, Trash2, Upload,
} from 'lucide-react'
import { dashboardApi, mattersApi, type ActivityEvent } from '../api/client'
import { useAuth } from '../stores/auth'
import { useSession } from '../stores/session'
import type { View } from './AppShell'
import AssistantAvatar from './AssistantAvatar'
import { ConfirmDialog, DateChips, dateInRange, DualToneBackdrop, EmptyState, Pager, Spinner, usePagination, type DateRange } from './ui'

const KIND_META: Record<string, { label: string; icon: React.ElementType; view: View }> = {
  query: { label: 'Asked', icon: MessageSquare, view: 'assistant' },
  research: { label: 'Researched', icon: FlaskConical, view: 'research' },
  workflow: { label: 'Agent run', icon: Bot, view: 'agents' },
  interact: { label: 'Interacted', icon: FileSearch, view: 'interact' },
  draft: { label: 'Drafted', icon: PenLine, view: 'draft' },
  upload: { label: 'Uploaded', icon: Upload, view: 'vault' },
}

function when(iso: string): string {
  const d = new Date(iso)
  const today = new Date()
  const sameDay = d.toDateString() === today.toDateString()
  return sameDay
    ? d.toLocaleTimeString('en-IN', { hour: 'numeric', minute: '2-digit' })
    : d.toLocaleDateString('en-IN', { day: 'numeric', month: 'short' })
}

function greeting(): string {
  const h = new Date().getHours()
  if (h < 12) return 'Good morning'
  if (h < 17) return 'Good afternoon'
  return 'Good evening'
}

function fmtDate(iso: string): string {
  return new Date(iso).toLocaleDateString('en-IN', { day: 'numeric', month: 'long', year: 'numeric' })
}

const SUGGESTIONS = [
  'What does Section 86 of the Income Tax Act cover?',
  'Summarise the key obligations in my latest document',
  'Draft a legal notice for a delayed payment',
]

const ACTIONS: { view: View; icon: React.ElementType; title: string; hint: string }[] = [
  { view: 'assistant', icon: MessageSquare, title: 'Ask Clause', hint: 'Cited answers from your vault' },
  { view: 'research', icon: FlaskConical, title: 'Research', hint: 'Hypotheses tested against evidence' },
  { view: 'agents', icon: Bot, title: 'Agents', hint: 'Contract review, due diligence, memos' },
  { view: 'interact', icon: FileSearch, title: 'Interact', hint: 'Summaries, chronologies, clause maps' },
  { view: 'draft', icon: PenLine, title: 'Draft', hint: 'Notices, agreements, petitions' },
  { view: 'private', icon: Lock, title: 'Private Vault', hint: 'Sealed documents only you can open' },
]

function ActivityRow({ e, go }: { e: ActivityEvent; go: (v: View) => void }) {
  const meta = KIND_META[e.kind] ?? KIND_META.query
  const Icon = meta.icon
  const setRestore = useSession((s) => s.setRestore)
  const askBridge = useSession((s) => s.ask)
  const toast = useSession((s) => s.toast)
  const qc = useQueryClient()
  // Piece 33: per-entry delete — the trash opens the shared consent
  // dialog; the entry is deleted permanently only on its explicit
  // "Delete permanently" click. Only the history entry goes;
  // usage/billing records are separate stores and stay untouched.
  const [confirming, setConfirming] = useState(false)
  const [deleting, setDeleting] = useState(false)
  async function remove() {
    setDeleting(true)
    try {
      await dashboardApi.deleteActivity(e.id)
      await qc.invalidateQueries({ queryKey: ['dashboard'] })
      toast('success', 'History entry deleted')
    } catch (err) {
      toast('error', `Could not delete: ${(err as Error).message}`)
      setDeleting(false)
      setConfirming(false)
    }
  }
  // A history click reopens the work itself: full result restored into
  // its page when a payload was captured; older query rows (pre-payload)
  // fall back to re-asking the question.
  function open() {
    if (e.payload) {
      setRestore(e.kind, e.payload)
    } else if (e.kind === 'query') {
      askBridge(e.title)
    }
    go(meta.view)
  }
  return (
    <div className="flex items-start border-b border-stone-100 dark:border-white/10 last:border-0 hover:bg-ivory transition-colors">
      <button
        onClick={open}
        className="min-w-0 flex-1 text-left px-5 py-3.5 flex gap-3.5"
      >
        <span className="w-8 h-8 rounded-full bg-ivory dark:bg-[#14120f] border border-stone-200 dark:border-white/10 flex items-center justify-center shrink-0 mt-0.5">
          <Icon size={14} className="text-stone-500 dark:text-stone-400" />
        </span>
        <span className="min-w-0 flex-1">
          <span className="flex items-baseline gap-2">
            <span className="text-[12px] font-medium text-stone-400 dark:text-stone-500">{meta.label}</span>
            {e.detail && <span className="text-[12px] text-stone-400 dark:text-stone-500">· {e.detail}</span>}
            <span className="ml-auto text-[12px] text-stone-400 dark:text-stone-500 shrink-0">{when(e.created_at)}</span>
          </span>
          <span className="block text-[14px] font-medium truncate mt-0.5">{e.title}</span>
          {e.result_excerpt && (
            <span className="block text-[13px] text-stone-500 dark:text-stone-400 line-clamp-2 mt-0.5">{e.result_excerpt}</span>
          )}
          <span className="block text-meta text-stone-400 dark:text-stone-500 mt-1">
            {e.tokens.toLocaleString('en-IN')} tokens{e.citations > 0 ? ` · ${e.citations} sources` : ''}
          </span>
        </span>
      </button>
      <button
        onClick={() => setConfirming(true)}
        title="Delete this history entry"
        className="mr-4 mt-3.5 shrink-0 p-1.5 rounded-lex text-stone-400 dark:text-stone-500 hover:text-[#a02c2c] hover:bg-stone-100 dark:hover:bg-white/10 transition-colors"
      >
        <Trash2 size={14} />
      </button>
      {confirming && (
        <ConfirmDialog
          title="Delete this history entry permanently?"
          body={`"${e.title}" will be removed from your history for good — it will no longer appear here or be restorable. Your token usage and billing records are not affected.`}
          busy={deleting}
          onConfirm={() => { void remove() }}
          onCancel={() => setConfirming(false)}
        />
      )}
    </div>
  )
}

function UsageRing({ pct }: { pct: number }) {
  const r = 34
  const c = 2 * Math.PI * r
  // Piece 34: the ring is a token-usage bar, so it takes the one data
  // colour; Piece 35: that colour is now the dual-tone gradient.
  // Red stays the ≥90% data signal.
  const over = pct >= 90
  return (
    <div className="relative w-[92px] h-[92px] shrink-0">
      <svg viewBox="0 0 84 84" className="w-full h-full -rotate-90">
        <defs>
          <linearGradient id="usage-ring-gradient" x1="0%" y1="0%" x2="100%" y2="100%">
            <stop offset="0%" stopColor="#c4664a" />
            <stop offset="100%" stopColor="#8fa382" />
          </linearGradient>
        </defs>
        <circle cx="42" cy="42" r={r} fill="none" stroke="#eeece7" strokeWidth="8" className="usage-ring-track" />
        <circle
          cx="42" cy="42" r={r} fill="none" stroke={over ? '#a02c2c' : 'url(#usage-ring-gradient)'} strokeWidth="8" strokeLinecap="round"
          strokeDasharray={c} strokeDashoffset={c - (c * Math.min(100, pct)) / 100}
        />
      </svg>
      <div className="absolute inset-0 flex flex-col items-center justify-center">
        <span className="text-[22px] font-medium tabular-nums leading-none">{pct}%</span>
        <span className="text-[11px] text-stone-400 dark:text-stone-500 mt-0.5">used</span>
      </div>
    </div>
  )
}

export default function DashboardPage({ go, openMatter }: { go: (v: View) => void; openMatter?: (id: string) => void }) {
  const user = useAuth((s) => s.user)
  const askBridge = useSession((s) => s.ask)
  const dash = useQuery({ queryKey: ['dashboard'], queryFn: dashboardApi.me })
  const matters = useQuery({ queryKey: ['matters'], queryFn: mattersApi.list })
  const [heroQ, setHeroQ] = useState('')
  const [range, setRange] = useState<DateRange>('all')
  const filteredActivity = (dash.data?.recent_activity ?? []).filter((e) => dateInRange(e.created_at, range))
  const act = usePagination(filteredActivity, 5)

  if (dash.isLoading) return <Spinner label="Loading your dashboard…" />
  if (dash.isError || !dash.data) return <EmptyState title="Dashboard unavailable" hint="Please try again in a moment." />
  const d = dash.data
  const pct = Math.min(100, Math.round((d.tokens_used / Math.max(1, d.token_quota)) * 100))
  const remaining = Math.max(0, d.token_quota - d.tokens_used)
  const firstName = (user?.email ?? 'there').split('@')[0]
  const isFree = d.plan_id === 'free'

  const askNow = (q: string) => {
    if (!q.trim()) return
    askBridge(q.trim())
    go('assistant')
  }

  return (
    <div className="w-full px-6 py-8">
      {/* ——— Clause hero ——— */}
      <div className="relative overflow-hidden rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#1e1c19] px-6 py-6 sm:px-8">
        {/* Piece 35: the moving dual-tone backdrop, clipped to the hero */}
        <DualToneBackdrop />
        <div className="relative flex items-start gap-5">
          <AssistantAvatar size={76} />
          <div className="min-w-0 flex-1">
            <h1 className="text-[24px] font-semibold leading-tight tracking-tight">
              {greeting()}, {firstName}
            </h1>
            <p className="text-stone-500 dark:text-stone-400 mt-1 text-[14px]">
              {remaining.toLocaleString('en-IN')} tokens left this period · {d.documents.length} documents in your vault · {(matters.data ?? []).length} briefs
            </p>
            <p className="text-stone-500 dark:text-stone-400 mt-1 text-[14.5px] max-w-xl">
              I'm <span className="font-semibold text-ink dark:text-[#f4f2ee]">Clause</span>, your legal research companion.
              Ask me anything across your briefs and vault — every answer cited to its source.
            </p>
            <form
              className="mt-4 flex items-center gap-2 rounded-lex border border-stone-200 dark:border-white/10 bg-white dark:bg-[#1e1c19] pl-4 pr-1.5 py-1.5 focus-within:border-stone-400 transition-colors"
              onSubmit={(e) => { e.preventDefault(); askNow(heroQ) }}
            >
              <Sparkles size={15} className="text-stone-400 dark:text-stone-500 shrink-0" />
              <input
                value={heroQ}
                onChange={(e) => setHeroQ(e.target.value)}
                placeholder="Ask Clause anything…"
                className="flex-1 min-w-0 bg-transparent outline-none text-[14.5px] placeholder:text-stone-400"
              />
              <button
                type="submit"
                disabled={!heroQ.trim()}
                className="w-8 h-8 rounded-lex bg-ink text-ivory flex items-center justify-center hover:bg-ink-900 disabled:opacity-40 transition-colors shrink-0"
                title="Ask Clause"
              >
                <ArrowUp size={15} />
              </button>
            </form>
            <div className="mt-3 flex flex-wrap gap-1.5">
              {SUGGESTIONS.map((s) => (
                <button
                  key={s}
                  onClick={() => askNow(s)}
                  className="rounded-lex border border-stone-200 dark:border-white/10 bg-white/70 px-3 py-1.5 text-[12px] text-stone-500 dark:text-stone-400 hover:text-ink hover:border-stone-300 transition-colors"
                >
                  {s}
                </button>
              ))}
            </div>
          </div>
        </div>
      </div>

      {/* ——— Subscription & activity ——— */}
      <div className="grid lg:grid-cols-5 gap-4 mt-5">
        <div className="lg:col-span-3 rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#1e1c19] px-6 py-5 flex items-center gap-6">
          <UsageRing pct={pct} />
          <div className="min-w-0">
            <p className="flex items-center gap-2">
              <span className="text-[13px] text-stone-500 dark:text-stone-400 font-medium">Subscription</span>
              <span className="text-[12px] text-stone-500 dark:text-stone-400">{d.plan_name}</span>
            </p>
            <p className="text-[26px] font-medium tabular-nums leading-tight mt-1.5">
              {remaining.toLocaleString('en-IN')} <span className="text-[16px] font-normal text-stone-500 dark:text-stone-400">tokens left</span>
            </p>
            <p className="text-[13px] text-stone-500 dark:text-stone-400 mt-0.5">
              {d.tokens_used.toLocaleString('en-IN')} of {d.token_quota.toLocaleString('en-IN')} used · renews {fmtDate(d.period_end)}
            </p>
            <button
              onClick={() => go('plans')}
              className="mt-3 inline-flex items-center gap-1.5 rounded-lex bg-ink text-ivory px-4 py-2 text-[13px] font-medium hover:bg-ink-900 transition-colors"
            >
              {isFree ? 'Upgrade your plan' : 'Manage subscription'} <ArrowRight size={13} />
            </button>
          </div>
        </div>
        <div className="lg:col-span-2 rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#1e1c19] px-6 py-5">
          <p className="text-[13px] text-stone-500 dark:text-stone-400 font-medium">This period with Clause</p>
          <div className="grid grid-cols-2 gap-x-4 gap-y-4 mt-4">
            {[
              { n: d.counts.queries, label: 'questions answered' },
              { n: d.counts.researches, label: 'research reports' },
              { n: d.counts.drafts, label: 'drafts created' },
              { n: d.counts.workflows, label: 'agent runs' },
            ].map((s) => (
              <div key={s.label}>
                <p className="text-[24px] font-medium tabular-nums leading-none">{s.n}</p>
                <p className="text-[12px] text-stone-500 dark:text-stone-400 mt-1">{s.label}</p>
              </div>
            ))}
          </div>
        </div>
      </div>

      {/* ——— What would you like to do? ——— */}
      <h2 className="font-display text-xl mt-9 mb-3">What would you like to do?</h2>
      <div className="grid sm:grid-cols-2 lg:grid-cols-3 gap-3">
        {ACTIONS.map((c) => (
          <button
            key={c.view}
            onClick={() => go(c.view)}
            className="rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#1e1c19] px-5 py-4 text-left hover:border-ink transition-colors group"
          >
            <span className="flex items-center justify-between">
              <c.icon size={18} className="text-stone-500 dark:text-stone-400 group-hover:text-ink transition-colors" />
              <ArrowRight size={14} className="text-stone-300 group-hover:text-ink group-hover:translate-x-0.5 transition-all" />
            </span>
            <p className="font-display text-lg mt-2.5">{c.title}</p>
            <p className="text-[13px] text-stone-500 dark:text-stone-400">{c.hint}</p>
          </button>
        ))}
      </div>

      {/* ——— Recent work + briefs ——— */}
      <div className="grid lg:grid-cols-5 gap-6 mt-9">
        <div className="lg:col-span-3">
          <div className="flex items-center justify-between gap-3 mb-3 flex-wrap">
            <h2 className="font-display text-xl">Pick up where you left off</h2>
            <DateChips value={range} onChange={(r) => { setRange(r); act.setPage(1) }} />
          </div>
          {d.recent_activity.length === 0 ? (
            <EmptyState title="Nothing yet" hint="Ask Clause your first question and it will appear here." />
          ) : filteredActivity.length === 0 ? (
            <EmptyState title="Nothing in this period" hint="Try a wider date range — your older work is still here." />
          ) : (
            <>
              <div className="rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#1e1c19] overflow-hidden">
                {act.pageItems.map((e) => <ActivityRow key={e.id} e={e} go={go} />)}
              </div>
              <Pager page={act.page} pages={act.pages} from={act.from} to={act.to} total={act.total} onPage={act.setPage} />
            </>
          )}
        </div>
        <div className="lg:col-span-2">
          <div className="flex items-baseline justify-between mb-3">
            <h2 className="font-display text-xl">Your briefs</h2>
            <button onClick={() => go('matters')} className="text-[13px] text-stone-500 dark:text-stone-400 hover:text-ink inline-flex items-center gap-1">
              All briefs <ArrowRight size={13} />
            </button>
          </div>
          {(matters.data ?? []).length === 0 ? (
            <EmptyState title="No briefs yet" hint="Briefs keep each matter's documents, memory, and history together." />
          ) : (
            <div className="rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#1e1c19] overflow-hidden">
              {(matters.data ?? []).slice(0, 3).map((m) => (
                <button
                  key={m.id}
                  onClick={() => (openMatter ? openMatter(m.id) : go('matters'))}
                  className="w-full text-left px-5 py-3.5 flex items-center gap-3 hover:bg-ivory border-b border-stone-100 dark:border-white/10 last:border-0"
                >
                  <Briefcase size={16} className="text-stone-400 dark:text-stone-500 shrink-0" />
                  <span className="min-w-0">
                    <span className="block text-[14px] font-medium truncate">{m.title}</span>
                    <span className="block text-meta text-stone-400 dark:text-stone-500 capitalize">{m.status} · updated {when(m.updated_at)}</span>
                  </span>
                </button>
              ))}
            </div>
          )}
          {d.documents.length > 0 && (
            <>
              <p className="text-[13px] text-stone-400 dark:text-stone-500 font-medium mt-6 mb-2">Latest in your vault</p>
              <div className="space-y-1">
                {d.documents.slice(0, 4).map((doc) => (
                  <button
                    key={doc.id}
                    onClick={() => go('interact')}
                    className="w-full text-left px-2 py-1.5 rounded-lex text-[13.5px] text-stone-600 dark:text-stone-300 hover:text-ink hover:bg-stone-100/70 dark:hover:bg-white/5 truncate transition-colors"
                  >
                    {doc.title}
                  </button>
                ))}
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  )
}
