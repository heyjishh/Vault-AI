import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import {
  ArrowRight, ArrowUp, Bot, Briefcase, FileSearch, FlaskConical, Lock, MessageSquare, PenLine, Sparkles, Upload,
} from 'lucide-react'
import { dashboardApi, mattersApi, type ActivityEvent } from '../api/client'
import { useAuth } from '../stores/auth'
import { useSession } from '../stores/session'
import type { View } from './AppShell'
import AssistantAvatar from './AssistantAvatar'
import { DateChips, dateInRange, EmptyState, Pager, Spinner, usePagination, type DateRange } from './ui'

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
    <button
      onClick={open}
      className="w-full text-left px-5 py-3.5 flex gap-3.5 hover:bg-ivory transition-colors border-b border-stone-100 last:border-0"
    >
      <span className="w-8 h-8 rounded-full bg-ivory border border-stone-200 flex items-center justify-center shrink-0 mt-0.5">
        <Icon size={14} className="text-stone-500" />
      </span>
      <span className="min-w-0 flex-1">
        <span className="flex items-baseline gap-2">
          <span className="text-[12px] font-medium uppercase tracking-wide text-stone-400">{meta.label}</span>
          {e.detail && <span className="text-[12px] text-stone-400">· {e.detail}</span>}
          <span className="ml-auto text-[12px] text-stone-400 shrink-0">{when(e.created_at)}</span>
        </span>
        <span className="block text-[14px] font-medium truncate mt-0.5">{e.title}</span>
        {e.result_excerpt && (
          <span className="block text-[13px] text-stone-500 line-clamp-2 mt-0.5">{e.result_excerpt}</span>
        )}
        <span className="block text-meta text-stone-400 mt-1">
          {e.tokens.toLocaleString('en-IN')} tokens{e.citations > 0 ? ` · ${e.citations} sources` : ''}
        </span>
      </span>
    </button>
  )
}

function UsageRing({ pct }: { pct: number }) {
  const r = 34
  const c = 2 * Math.PI * r
  const color = pct >= 90 ? '#a02c2c' : pct >= 70 ? '#b7791f' : '#0f0e0d'
  return (
    <div className="relative w-[92px] h-[92px] shrink-0">
      <svg viewBox="0 0 84 84" className="w-full h-full -rotate-90">
        <circle cx="42" cy="42" r={r} fill="none" stroke="#eeece7" strokeWidth="8" />
        <circle
          cx="42" cy="42" r={r} fill="none" stroke={color} strokeWidth="8" strokeLinecap="round"
          strokeDasharray={c} strokeDashoffset={c - (c * Math.min(100, pct)) / 100}
        />
      </svg>
      <div className="absolute inset-0 flex flex-col items-center justify-center">
        <span className="font-display text-[22px] leading-none">{pct}%</span>
        <span className="text-[10px] uppercase tracking-wider text-stone-400 mt-0.5">used</span>
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
    <div className="max-w-5xl mx-auto px-6 py-8">
      {/* ——— Clause hero ——— */}
      <div className="rounded-lex-lg border border-stone-200 bg-gradient-to-br from-white via-white to-[#f4f1ea] shadow-lex px-6 py-6 sm:px-8">
        <div className="flex items-start gap-5">
          <AssistantAvatar size={76} />
          <div className="min-w-0 flex-1">
            <h1 className="font-display text-[1.8rem] leading-tight tracking-tight capitalize">
              {greeting()}, {firstName}
            </h1>
            <p className="text-stone-500 mt-1 text-[14.5px] max-w-xl">
              I'm <span className="font-semibold text-ink">Clause</span>, your legal research companion.
              Ask me anything across your briefs and vault — every answer cited to its source.
            </p>
            <form
              className="mt-4 flex items-center gap-2 rounded-full border border-stone-200 bg-white shadow-lex-sm pl-4 pr-1.5 py-1.5 focus-within:border-stone-400 transition-colors max-w-xl"
              onSubmit={(e) => { e.preventDefault(); askNow(heroQ) }}
            >
              <Sparkles size={15} className="text-stone-400 shrink-0" />
              <input
                value={heroQ}
                onChange={(e) => setHeroQ(e.target.value)}
                placeholder="Ask Clause anything…"
                className="flex-1 min-w-0 bg-transparent outline-none text-[14.5px] placeholder:text-stone-400"
              />
              <button
                type="submit"
                disabled={!heroQ.trim()}
                className="w-8 h-8 rounded-full bg-ink text-ivory flex items-center justify-center hover:bg-ink-900 disabled:opacity-40 transition-colors shrink-0"
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
                  className="rounded-full border border-stone-200 bg-white/70 px-3 py-1.5 text-[12px] text-stone-500 hover:text-ink hover:border-stone-300 transition-colors"
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
        <div className="lg:col-span-3 rounded-lex-lg border border-stone-200 bg-white shadow-lex px-6 py-5 flex items-center gap-6">
          <UsageRing pct={pct} />
          <div className="min-w-0">
            <p className="flex items-center gap-2">
              <span className="text-meta uppercase tracking-wider text-stone-500 font-medium">Subscription</span>
              <span className="text-[10.5px] font-semibold uppercase tracking-wide bg-ink text-ivory rounded px-1.5 py-0.5">{d.plan_name}</span>
            </p>
            <p className="font-display text-[26px] leading-tight mt-1.5">
              {remaining.toLocaleString('en-IN')} <span className="text-[16px] text-stone-500">tokens left</span>
            </p>
            <p className="text-[13px] text-stone-500 mt-0.5">
              {d.tokens_used.toLocaleString('en-IN')} of {d.token_quota.toLocaleString('en-IN')} used · renews {fmtDate(d.period_end)}
            </p>
            <button
              onClick={() => go('plans')}
              className="mt-3 inline-flex items-center gap-1.5 rounded-full bg-ink text-ivory px-4 py-2 text-[13px] font-medium hover:bg-ink-900 transition-colors"
            >
              {isFree ? 'Upgrade your plan' : 'Manage subscription'} <ArrowRight size={13} />
            </button>
          </div>
        </div>
        <div className="lg:col-span-2 rounded-lex-lg border border-stone-200 bg-white shadow-lex px-6 py-5">
          <p className="text-meta uppercase tracking-wider text-stone-500 font-medium">This period with Clause</p>
          <div className="grid grid-cols-2 gap-x-4 gap-y-4 mt-4">
            {[
              { n: d.counts.queries, label: 'questions answered' },
              { n: d.counts.researches, label: 'research reports' },
              { n: d.counts.drafts, label: 'drafts created' },
              { n: d.counts.workflows, label: 'agent runs' },
            ].map((s) => (
              <div key={s.label}>
                <p className="font-display text-[24px] leading-none">{s.n}</p>
                <p className="text-[12px] text-stone-500 mt-1">{s.label}</p>
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
            className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-5 py-4 text-left hover:border-ink transition-colors group"
          >
            <span className="flex items-center justify-between">
              <c.icon size={18} className="text-stone-500 group-hover:text-ink transition-colors" />
              <ArrowRight size={14} className="text-stone-300 group-hover:text-ink group-hover:translate-x-0.5 transition-all" />
            </span>
            <p className="font-display text-lg mt-2.5">{c.title}</p>
            <p className="text-[13px] text-stone-500">{c.hint}</p>
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
              <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex overflow-hidden">
                {act.pageItems.map((e) => <ActivityRow key={e.id} e={e} go={go} />)}
              </div>
              <Pager page={act.page} pages={act.pages} from={act.from} to={act.to} total={act.total} onPage={act.setPage} />
            </>
          )}
        </div>
        <div className="lg:col-span-2">
          <div className="flex items-baseline justify-between mb-3">
            <h2 className="font-display text-xl">Your briefs</h2>
            <button onClick={() => go('matters')} className="text-[13px] text-stone-500 hover:text-ink inline-flex items-center gap-1">
              All briefs <ArrowRight size={13} />
            </button>
          </div>
          {(matters.data ?? []).length === 0 ? (
            <EmptyState title="No briefs yet" hint="Briefs keep each matter's documents, memory, and history together." />
          ) : (
            <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex overflow-hidden">
              {(matters.data ?? []).slice(0, 3).map((m) => (
                <button
                  key={m.id}
                  onClick={() => (openMatter ? openMatter(m.id) : go('matters'))}
                  className="w-full text-left px-5 py-3.5 flex items-center gap-3 hover:bg-ivory border-b border-stone-100 last:border-0"
                >
                  <Briefcase size={16} className="text-stone-400 shrink-0" />
                  <span className="min-w-0">
                    <span className="block text-[14px] font-medium truncate">{m.title}</span>
                    <span className="block text-meta text-stone-400 capitalize">{m.status} · updated {when(m.updated_at)}</span>
                  </span>
                </button>
              ))}
            </div>
          )}
          {d.documents.length > 0 && (
            <>
              <p className="text-meta uppercase tracking-wider text-stone-400 font-medium mt-6 mb-2">Latest in your vault</p>
              <div className="space-y-1">
                {d.documents.slice(0, 4).map((doc) => (
                  <button
                    key={doc.id}
                    onClick={() => go('interact')}
                    className="w-full text-left px-2 py-1.5 rounded-lex text-[13.5px] text-stone-600 hover:text-ink hover:bg-stone-100/70 truncate transition-colors"
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
