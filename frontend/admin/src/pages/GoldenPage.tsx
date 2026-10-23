import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Play, Plus, Trash2 } from 'lucide-react'
import { adminApi, type GoldenRule } from '../api'
import { ErrorBanner, PageHead, Spinner, StatCard } from '../ui'

function ResultBadge({ rule }: { rule: GoldenRule }) {
  const r = rule.last_result
  if (!r) return <span className="rounded-full bg-stone-100 text-stone-500 px-2 py-0.5 text-[12px]">not run yet</span>
  return r.passed ? (
    <span className="rounded-full bg-[#e7f4e4] text-[#2f6b2f] px-2 py-0.5 text-[12px]">passing</span>
  ) : (
    <span className="rounded-full bg-[#feeaea] text-[#a02c2c] px-2 py-0.5 text-[12px]">failing</span>
  )
}

export default function GoldenPage() {
  const queryClient = useQueryClient()
  const rules = useQuery({ queryKey: ['golden-rules'], queryFn: adminApi.goldenRules })
  const insights = useQuery({ queryKey: ['insights'], queryFn: adminApi.insights })

  const [question, setQuestion] = useState('')
  const [terms, setTerms] = useState('')
  const [sources, setSources] = useState('')
  const [notes, setNotes] = useState('')
  const [runError, setRunError] = useState<string | null>(null)

  const invalidate = () => queryClient.invalidateQueries({ queryKey: ['golden-rules'] })

  const add = useMutation({
    mutationFn: () =>
      adminApi.addGoldenRule({
        question: question.trim(),
        expected_terms: terms.split(',').map((s) => s.trim()).filter(Boolean),
        expected_sources: sources.split(',').map((s) => s.trim()).filter(Boolean),
        notes: notes.trim(),
      }),
    onSuccess: () => { setQuestion(''); setTerms(''); setSources(''); setNotes(''); invalidate() },
  })

  const promote = useMutation({
    mutationFn: (id: string) => adminApi.goldenFromActivity(id),
    onSuccess: invalidate,
  })

  const remove = useMutation({
    mutationFn: (id: string) => adminApi.deleteGoldenRule(id),
    onSuccess: invalidate,
  })

  const run = useMutation({
    mutationFn: adminApi.runGolden,
    onSuccess: () => { setRunError(null); invalidate() },
    onError: (e) => setRunError((e as Error).message),
  })

  if (rules.isLoading) return <Spinner label="Loading golden ruleset…" />
  if (rules.isError || !rules.data) return <ErrorBanner message="Could not load the golden ruleset." />
  const { rules: list, last_run } = rules.data
  const passing = list.filter((r) => r.last_result?.passed).length
  const captured = (insights.data?.recent_activity ?? []).filter((e) => e.kind === 'query')

  return (
    <div className="max-w-5xl mx-auto px-6 py-10">
      <PageHead
        title="Golden Ruleset"
        sub="The evaluation set that keeps the RAG honest. Promote real user questions into rules with expected terms and sources, then run the set — a rule passes only if the live answer contains every expected term and cites every expected source."
      />

      <div className="grid grid-cols-2 xl:grid-cols-4 gap-4 mt-6">
        <StatCard label="Rules" value={list.length} sub={`${list.filter((r) => r.source === 'captured').length} promoted from real queries`} />
        <StatCard label="Passing" value={last_run ? `${passing} / ${list.length}` : '—'} sub={last_run ? `last run ${new Date(last_run.ran_at).toLocaleString('en-IN')}` : 'run the set to score it'} />
        <StatCard label="Pass rate" value={last_run && last_run.total > 0 ? `${Math.round((last_run.passed / last_run.total) * 100)}%` : '—'} sub="of the last full run" />
        <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-5 py-4 flex flex-col justify-center">
          <button
            onClick={() => run.mutate()}
            disabled={run.isPending || list.length === 0}
            className="rounded-lex bg-ink text-ivory px-4 py-2.5 text-[13.5px] font-medium disabled:opacity-40 inline-flex items-center justify-center gap-2"
          >
            <Play size={14} /> {run.isPending ? 'Running evaluation…' : 'Run evaluation'}
          </button>
          <p className="text-meta text-stone-400 mt-2 text-center">Runs every rule against the live RAG (mix mode). Cost is billed to your admin account.</p>
        </div>
      </div>
      {runError && <div className="mt-4"><ErrorBanner message={runError} /></div>}
      {add.isError && <div className="mt-4"><ErrorBanner message={(add.error as Error).message} /></div>}

      {/* Rules table */}
      <h2 className="font-display text-xl mt-9 mb-3">Rules</h2>
      {list.length === 0 ? (
        <p className="text-stone-500 text-[13.5px]">No rules yet — add one below, or promote a captured question.</p>
      ) : (
        <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex overflow-hidden">
          <table className="w-full text-[13.5px]">
            <thead>
              <tr className="text-left text-meta uppercase tracking-wider text-stone-500 border-b border-stone-200">
                <th className="px-5 py-3 font-medium w-[38%]">Question</th>
                <th className="px-5 py-3 font-medium">Expected terms</th>
                <th className="px-5 py-3 font-medium">Expected sources</th>
                <th className="px-5 py-3 font-medium">Status</th>
                <th className="px-5 py-3 font-medium">Last answer</th>
                <th className="px-3 py-3"></th>
              </tr>
            </thead>
            <tbody>
              {list.map((r) => (
                <tr key={r.id} className="border-b border-stone-100 last:border-0 align-top">
                  <td className="px-5 py-3">
                    <span className="font-medium">{r.question}</span>
                    <span className="block text-meta text-stone-400 mt-0.5">
                      {r.source === 'captured' ? 'promoted from a user query' : 'written by admin'}
                      {r.notes ? ` · ${r.notes}` : ''}
                    </span>
                  </td>
                  <td className="px-5 py-3 text-stone-600">{r.expected_terms.join(', ') || '—'}</td>
                  <td className="px-5 py-3 text-stone-600">{r.expected_sources.join(', ') || '—'}</td>
                  <td className="px-5 py-3">
                    <ResultBadge rule={r} />
                    {r.last_result && !r.last_result.passed && (
                      <span className="block text-meta text-[#a02c2c] mt-1">
                        {r.last_result.missing_terms.length > 0 && <>missing terms: {r.last_result.missing_terms.join(', ')} </>}
                        {r.last_result.missing_sources.length > 0 && <>missing sources: {r.last_result.missing_sources.join(', ')}</>}
                      </span>
                    )}
                  </td>
                  <td className="px-5 py-3 text-stone-500 max-w-[260px]">
                    <span className="line-clamp-3">{r.last_result?.answer_excerpt ?? '—'}</span>
                  </td>
                  <td className="px-3 py-3">
                    <button onClick={() => remove.mutate(r.id)} className="text-stone-300 hover:text-[#a02c2c]" title="Delete rule">
                      <Trash2 size={15} />
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <div className="grid xl:grid-cols-2 gap-6 mt-9">
        {/* Add rule */}
        <div>
          <h2 className="font-display text-xl mb-3">Add a rule</h2>
          <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-6 py-5 grid gap-3.5">
            <label className="block">
              <span className="text-meta uppercase tracking-wider text-stone-500 font-medium">Question</span>
              <input value={question} onChange={(e) => setQuestion(e.target.value)} className="mt-1.5 w-full rounded-lex border border-stone-300 px-3 py-2 text-[14px]" placeholder="What does Section 86 of the IT Act deal with?" />
            </label>
            <label className="block">
              <span className="text-meta uppercase tracking-wider text-stone-500 font-medium">Expected terms (comma-separated)</span>
              <input value={terms} onChange={(e) => setTerms(e.target.value)} className="mt-1.5 w-full rounded-lex border border-stone-300 px-3 py-2 text-[14px]" placeholder="section 86, cert-in" />
            </label>
            <label className="block">
              <span className="text-meta uppercase tracking-wider text-stone-500 font-medium">Expected sources (comma-separated)</span>
              <input value={sources} onChange={(e) => setSources(e.target.value)} className="mt-1.5 w-full rounded-lex border border-stone-300 px-3 py-2 text-[14px]" placeholder="Section 86 IT Act" />
            </label>
            <label className="block">
              <span className="text-meta uppercase tracking-wider text-stone-500 font-medium">Notes</span>
              <input value={notes} onChange={(e) => setNotes(e.target.value)} className="mt-1.5 w-full rounded-lex border border-stone-300 px-3 py-2 text-[14px]" placeholder="Why this rule matters" />
            </label>
            <button
              onClick={() => add.mutate()}
              disabled={!question.trim() || add.isPending}
              className="rounded-lex bg-ink text-ivory px-4 py-2.5 text-[13.5px] font-medium disabled:opacity-40 inline-flex items-center justify-center gap-2"
            >
              <Plus size={14} /> Add rule
            </button>
          </div>
        </div>

        {/* Promote captured queries */}
        <div>
          <h2 className="font-display text-xl mb-3">Promote a captured query</h2>
          <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex overflow-hidden">
            {captured.length === 0 && (
              <p className="px-5 py-4 text-stone-500 text-[13.5px]">No captured questions yet — they appear as users ask.</p>
            )}
            {captured.slice(0, 8).map((e) => (
              <div key={e.id} className="px-5 py-3 flex items-center gap-3 border-b border-stone-100 last:border-0">
                <span className="min-w-0 flex-1">
                  <span className="block text-[13.5px] font-medium truncate">{e.title}</span>
                  <span className="block text-meta text-stone-400">{e.user_email} · {new Date(e.created_at).toLocaleDateString('en-IN', { day: 'numeric', month: 'short' })}</span>
                </span>
                <button
                  onClick={() => promote.mutate(e.id)}
                  disabled={promote.isPending}
                  className="text-[12px] shrink-0 rounded-full border border-stone-300 px-2.5 py-1 hover:border-ink disabled:opacity-50"
                >
                  Make golden rule
                </button>
              </div>
            ))}
          </div>
          <p className="text-meta text-stone-400 mt-3">
            Promoted rules start with no expectations — edit them by deleting and re-adding with terms/sources, or add expectations when you promote from Insights.
          </p>
        </div>
      </div>
    </div>
  )
}
