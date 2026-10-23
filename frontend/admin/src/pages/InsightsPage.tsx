import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ChevronRight, Sparkles, X } from 'lucide-react'
import { adminApi, type ActivityEvent, type UserInsight } from '../api'
import { BarChart, Donut, ErrorBanner, PageHead, Spinner, StatCard } from '../ui'

function inr(n: number): string {
  return '₹' + n.toLocaleString('en-IN')
}

function when(iso: string | null): string {
  if (!iso) return 'never'
  const d = new Date(iso)
  return d.toLocaleDateString('en-IN', { day: 'numeric', month: 'short' }) +
    ' ' + d.toLocaleTimeString('en-IN', { hour: 'numeric', minute: '2-digit' })
}

const KIND_LABEL: Record<string, string> = {
  query: 'Asked', research: 'Researched', workflow: 'Agent run',
  interact: 'Interacted', draft: 'Drafted', upload: 'Uploaded',
}

function ActivityLine({ e }: { e: ActivityEvent }) {
  return (
    <div className="px-5 py-3 border-b border-stone-100 last:border-0">
      <div className="flex items-baseline gap-2 text-[12px] text-stone-400">
        <span className="font-medium uppercase tracking-wide">{KIND_LABEL[e.kind] ?? e.kind}</span>
        {e.detail && <span>· {e.detail}</span>}
        <span className="ml-auto shrink-0">{when(e.created_at)}</span>
      </div>
      <p className="text-[13.5px] font-medium mt-0.5">{e.title}</p>
      {e.result_excerpt && <p className="text-[12.5px] text-stone-500 line-clamp-2 mt-0.5">{e.result_excerpt}</p>}
      <p className="text-meta text-stone-400 mt-1">
        {e.tokens.toLocaleString('en-IN')} tokens{e.citations > 0 ? ` · ${e.citations} sources` : ''}
      </p>
    </div>
  )
}

export default function InsightsPage() {
  const queryClient = useQueryClient()
  const insights = useQuery({ queryKey: ['insights'], queryFn: adminApi.insights })
  const [selected, setSelected] = useState<UserInsight | null>(null)
  const [promoted, setPromoted] = useState<string | null>(null)

  const drill = useQuery({
    queryKey: ['user-activity', selected?.user_id],
    queryFn: () => adminApi.userActivity(selected!.user_id),
    enabled: !!selected,
  })

  const promote = useMutation({
    mutationFn: (activityId: string) => adminApi.goldenFromActivity(activityId),
    onSuccess: (_r, activityId) => {
      setPromoted(activityId)
      queryClient.invalidateQueries({ queryKey: ['golden-rules'] })
    },
  })

  if (insights.isLoading) return <Spinner label="Loading insights…" />
  if (insights.isError || !insights.data) return <ErrorBanner message="Could not load insights." />
  const d = insights.data

  const eventBars = d.series_14d.map((p) => ({ label: p.date.slice(5), value: p.events }))
  const tokenBars = d.series_14d.map((p) => ({ label: p.date.slice(5), value: p.tokens }))
  // Map top-query text back to a representative activity id for promotion.
  const activityIdFor = (question: string) =>
    d.recent_activity.find((e) => e.kind === 'query' && e.title.toLowerCase() === question.toLowerCase())?.id

  return (
    <div className="max-w-5xl mx-auto px-6 py-10">
      <PageHead
        title="Insights"
        sub="Every user, every question, every token — the whole product in one place. Click a user to see their work; click a top question to promote it into the golden ruleset."
      />

      <div className="grid grid-cols-2 xl:grid-cols-4 gap-4 mt-6">
        <StatCard label="Total users" value={d.total_users} sub={`${d.active_subscriptions} paid subscriptions`} />
        <StatCard label="Questions captured" value={d.total_queries} sub={`${d.total_events} total activities`} />
        <StatCard label="Tokens billed" value={d.total_tokens_billed.toLocaleString('en-IN')} sub="this period, all users" />
        <StatCard label="Revenue" value={inr(d.collected_inr)} sub={`MRR ${inr(d.mrr_inr)} · ${d.documents} docs · ${d.chunks} chunks`} />
      </div>

      <div className="grid xl:grid-cols-3 gap-4 mt-5">
        <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-5 py-4">
          <p className="text-meta uppercase tracking-wider text-stone-500 font-medium">Activity — last 14 days</p>
          <div className="mt-3"><BarChart data={eventBars} /></div>
        </div>
        <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-5 py-4">
          <p className="text-meta uppercase tracking-wider text-stone-500 font-medium">Tokens — last 14 days</p>
          <div className="mt-3"><BarChart data={tokenBars} /></div>
        </div>
        <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-5 py-4">
          <p className="text-meta uppercase tracking-wider text-stone-500 font-medium">Plan mix</p>
          <div className="mt-3">
            <Donut data={d.plan_distribution.map((p) => ({ label: p.plan, value: p.users }))} />
          </div>
        </div>
      </div>

      <div className="grid xl:grid-cols-5 gap-6 mt-8">
        <div className="xl:col-span-3">
          <h2 className="font-display text-xl mb-3">Users — click to drill down</h2>
          <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex overflow-hidden">
            <table className="w-full text-[13.5px]">
              <thead>
                <tr className="text-left text-meta uppercase tracking-wider text-stone-500 border-b border-stone-200">
                  <th className="px-5 py-3 font-medium">User</th>
                  <th className="px-5 py-3 font-medium">Plan</th>
                  <th className="px-5 py-3 font-medium">Activity</th>
                  <th className="px-5 py-3 font-medium">Tokens</th>
                  <th className="px-5 py-3 font-medium">Spent</th>
                  <th className="px-5 py-3 font-medium">Last active</th>
                  <th className="px-2 py-3"></th>
                </tr>
              </thead>
              <tbody>
                {d.per_user.map((u) => (
                  <tr
                    key={u.user_id}
                    onClick={() => setSelected(u)}
                    className={`cursor-pointer border-b border-stone-100 last:border-0 hover:bg-ivory ${selected?.user_id === u.user_id ? 'bg-ivory' : ''}`}
                  >
                    <td className="px-5 py-3 font-medium">{u.email}</td>
                    <td className="px-5 py-3">{u.plan_name}</td>
                    <td className="px-5 py-3">{u.events} <span className="text-stone-400">({u.queries}q · {u.researches}r)</span></td>
                    <td className="px-5 py-3">{u.tokens_billed.toLocaleString('en-IN')} <span className="text-stone-400">/ {u.token_quota.toLocaleString('en-IN')}</span></td>
                    <td className="px-5 py-3">{inr(u.spent_inr)}</td>
                    <td className="px-5 py-3 text-stone-500">{when(u.last_active)}</td>
                    <td className="px-2 py-3 text-stone-300"><ChevronRight size={15} /></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <h2 className="font-display text-xl mt-8 mb-3">Top questions</h2>
          <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex overflow-hidden">
            {d.top_queries.length === 0 && <p className="px-5 py-4 text-stone-500 text-[13.5px]">No questions captured yet.</p>}
            {d.top_queries.map((q) => {
              const actId = activityIdFor(q.question)
              return (
                <div key={q.question} className="px-5 py-3 flex items-center gap-3 border-b border-stone-100 last:border-0">
                  <span className="rounded-full bg-ink text-ivory text-[11.5px] px-2 py-0.5 font-medium shrink-0">×{q.count}</span>
                  <span className="text-[13.5px] flex-1 min-w-0 truncate" title={q.question}>{q.question}</span>
                  {actId && (
                    promoted === actId ? (
                      <span className="text-[12px] text-[#2f6b2f] shrink-0">In golden set ✓</span>
                    ) : (
                      <button
                        onClick={() => promote.mutate(actId)}
                        disabled={promote.isPending}
                        className="text-[12px] shrink-0 inline-flex items-center gap-1 rounded-full border border-stone-300 px-2.5 py-1 hover:border-ink disabled:opacity-50"
                      >
                        <Sparkles size={12} /> Make golden rule
                      </button>
                    )
                  )}
                </div>
              )
            })}
          </div>
        </div>

        <div className="xl:col-span-2">
          {selected ? (
            <div className="rounded-lex-lg border border-ink bg-white shadow-lex overflow-hidden sticky top-4">
              <div className="px-5 py-4 border-b border-stone-200 flex items-start justify-between">
                <div>
                  <h2 className="font-display text-lg leading-tight">{selected.email}</h2>
                  <p className="text-meta text-stone-500 mt-0.5">
                    {selected.plan_name} · {selected.events} activities · spent {inr(selected.spent_inr)}
                  </p>
                </div>
                <button onClick={() => setSelected(null)} className="text-stone-400 hover:text-ink"><X size={17} /></button>
              </div>
              <div className="max-h-[560px] overflow-y-auto">
                {drill.isLoading && <Spinner label="Loading activity…" />}
                {drill.data && drill.data.length === 0 && (
                  <p className="px-5 py-4 text-stone-500 text-[13.5px]">No activity captured for this user yet.</p>
                )}
                {drill.data?.map((e) => <ActivityLine key={e.id} e={e} />)}
              </div>
            </div>
          ) : (
            <>
              <h2 className="font-display text-xl mb-3">Live activity</h2>
              <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex overflow-hidden">
                {d.recent_activity.length === 0 && (
                  <p className="px-5 py-4 text-stone-500 text-[13.5px]">Nothing yet — user questions will stream in here.</p>
                )}
                {d.recent_activity.slice(0, 12).map((e) => (
                  <div key={e.id}>
                    <p className="px-5 pt-2.5 text-[11.5px] text-stone-400">{e.user_email}</p>
                    <ActivityLine e={e} />
                  </div>
                ))}
              </div>
              <p className="text-meta text-stone-400 mt-3">Select any user on the left to see everything they've asked, researched, and drafted.</p>
            </>
          )}
        </div>
      </div>
    </div>
  )
}
