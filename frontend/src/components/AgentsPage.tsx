import { useEffect, useState } from 'react'
import { useMutation, useQuery } from '@tanstack/react-query'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { Bot, ChevronDown, Compass, Play } from 'lucide-react'
import { mattersApi, ragApi } from '../api/client'
import { useSession } from '../stores/session'
import type { AutonomousRun, Citation, WorkflowDef, WorkflowRun } from '../api/types'
import { CitationCard, EmptyState, Spinner } from './ui'
import { CitedText, SourceModal, citeMarkdownComponents, linkifyCitations } from './SourceModal'

const ICONS: Record<string, string> = {
  contract_review: 'Review a contract clause-by-clause for risk, obligations and missing protections.',
  due_diligence: 'Run diligence across documents and the web for red flags and exposure.',
  memo_draft: 'Draft a structured IRAC legal memo from the corpus.',
}

export default function AgentsPage({ go }: { go?: (view: string) => void } = {}) {
  const { data: workflows, isLoading } = useQuery({ queryKey: ['workflows'], queryFn: ragApi.workflows })
  const [selected, setSelected] = useState<WorkflowDef | null>(null)
  const [input, setInput] = useState('')
  const [run, setRun] = useState<WorkflowRun | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [openStep, setOpenStep] = useState<number | null>(null)
  const [modalCitation, setModalCitation] = useState<Citation | null>(null)

  // History restore: reopen the exact run and its workflow context.
  const restore = useSession((st) => st.restore)
  const consumeRestore = useSession((st) => st.consumeRestore)
  useEffect(() => {
    if (!restore || restore.kind !== 'workflow') return
    const p = consumeRestore('workflow')
    if (!p) return
    const r = p.run as WorkflowRun
    setRun(r)
    setInput(String(p.input ?? r.input ?? ''))
    setError(null)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [restore])
  useEffect(() => {
    if (run && workflows && !selected) {
      const def = workflows.find((w) => w.id === run.workflow_id)
      if (def) setSelected(def)
    }
  }, [run, workflows, selected])

  const start = useMutation({
    mutationFn: () => ragApi.runWorkflow(selected!.id, input.trim()),
    onSuccess: (r) => { setRun(r); setError(null); setOpenStep(null) },
    onError: (e) => setError((e as Error).message),
  })

  // Autonomous mode: the planner chooses its own vault/web steps.
  const [goal, setGoal] = useState('')
  const [autoMatter, setAutoMatter] = useState('')
  const [autoSteps, setAutoSteps] = useState(6)
  const [autoRun, setAutoRun] = useState<AutonomousRun | null>(null)
  const [autoError, setAutoError] = useState<string | null>(null)
  const matters = useQuery({ queryKey: ['matters'], queryFn: mattersApi.list })
  const startAuto = useMutation({
    mutationFn: () => ragApi.runAutonomous(goal.trim(), autoMatter || undefined, autoSteps),
    onSuccess: (r) => { setAutoRun(r); setAutoError(null) },
    onError: (e) => setAutoError((e as Error).message),
  })

  return (
    <div className="max-w-4xl mx-auto px-6 py-10">
      <h1 className="font-display text-[2rem] tracking-tight flex items-center gap-3">
        <Bot className="text-stone-500" /> Agents
      </h1>
      <p className="text-stone-500 mt-1.5 text-[15px]">
        Multi-step agentic workflows — each step retrieves grounded evidence, feeds its findings forward, and the final report is synthesized with citations.
      </p>

      {isLoading ? (
        <Spinner label="Loading workflows…" />
      ) : !workflows || workflows.length === 0 ? (
        <div className="mt-6"><EmptyState title="No workflows available" /></div>
      ) : (
        <div className="grid md:grid-cols-3 gap-3 mt-6">
          {workflows.map((w) => (
            <button
              key={w.id}
              onClick={() => { setSelected(w); setRun(null); setError(null) }}
              className={`text-left rounded-lex-lg border bg-white shadow-lex p-5 transition-colors ${
                selected?.id === w.id ? 'border-ink' : 'border-stone-200 hover:border-stone-300'
              }`}
            >
              <p className="font-display text-lg tracking-tight">{w.name}</p>
              <p className="text-meta2 text-stone-500 mt-1.5 leading-relaxed">{ICONS[w.id] ?? w.description}</p>
              <p className="text-meta text-stone-400 mt-3">{w.steps.length} steps</p>
            </button>
          ))}
        </div>
      )}

      {selected && (
        <div className="mt-6 rounded-lex-lg border border-stone-200 bg-white shadow-lex p-5 fade-up">
          <label className="block text-meta font-medium text-stone-600 mb-1.5">{selected.input_label}</label>
          <textarea
            value={input} onChange={(e) => setInput(e.target.value)} rows={4}
            placeholder="Paste the contract text, deal summary, or legal question…"
            className="w-full rounded-lex border border-stone-300 px-3 py-2.5 text-[15px] outline-none focus:border-ink placeholder:text-stone-400 resize-y"
          />
          <div className="flex items-center justify-between mt-3">
            <p className="text-meta text-stone-400">Steps: {selected.steps.map((s) => s.name).join(' → ')}</p>
            <button
              onClick={() => input.trim() && start.mutate()} disabled={start.isPending || !input.trim()}
              className="flex items-center gap-2 rounded-lex bg-ink text-ivory px-4 py-2 text-meta2 font-medium hover:bg-ink-900 disabled:opacity-40"
            >
              <Play size={14} /> {start.isPending ? 'Running…' : 'Run workflow'}
            </button>
          </div>
        </div>
      )}

      <div className="mt-8 rounded-lex-lg border border-stone-200 bg-white shadow-lex p-5">
        <h2 className="font-display text-xl tracking-tight flex items-center gap-2">
          <Compass size={18} className="text-stone-500" /> Autonomous research
        </h2>
        <p className="text-meta2 text-stone-500 mt-1">
          Give it a goal — a lead agent splits it across specialist researchers, a drafter, and an independent checker, then reconciles their work into a cited report. Fixed workflows below run a set recipe; this one chooses.
        </p>
        <textarea
          value={goal} onChange={(e) => setGoal(e.target.value)} rows={3}
          placeholder="e.g. What does the vault say about compensation for delayed possession, and what is the current law on the web?"
          className="w-full mt-3 rounded-lex border border-stone-300 px-3 py-2.5 text-[15px] outline-none focus:border-ink placeholder:text-stone-400 resize-y"
        />
        <div className="flex flex-wrap items-center gap-3 mt-3">
          <select value={autoMatter} onChange={(e) => setAutoMatter(e.target.value)}
            className="rounded-lex border border-stone-300 px-2.5 py-2 text-meta2 bg-white outline-none focus:border-ink max-w-[220px]">
            <option value="">Whole vault</option>
            {(matters.data ?? []).map((m) => <option key={m.id} value={m.id}>{m.title}</option>)}
          </select>
          <label className="flex items-center gap-2 text-meta2 text-stone-600">
            Max steps
            <input type="number" min={1} max={10} value={autoSteps}
              onChange={(e) => setAutoSteps(Math.min(10, Math.max(1, Number(e.target.value) || 6)))}
              className="w-16 rounded-lex border border-stone-300 px-2 py-1.5 outline-none focus:border-ink" />
          </label>
          <button
            onClick={() => goal.trim() && startAuto.mutate()} disabled={startAuto.isPending || !goal.trim()}
            className="ml-auto flex items-center gap-2 rounded-lex bg-ink text-ivory px-4 py-2 text-meta2 font-medium hover:bg-ink-900 disabled:opacity-40"
          >
            <Play size={14} /> {startAuto.isPending ? 'Researching…' : 'Run autonomous agent'}
          </button>
        </div>
        {autoError && (
          <div className="mt-4 rounded-lex border border-brand-red/30 bg-[#feeaea] px-4 py-3 text-meta2 text-[#a02c2c]">{autoError}</div>
        )}
        {startAuto.isPending && <Spinner label="The agent is planning and running its own research steps…" />}
        {autoRun && !startAuto.isPending && (
          <div className="mt-5 fade-up">
            {(autoRun.sub_agents?.length ?? 0) > 0 && (
              <div className="space-y-1.5 mb-4">
                {autoRun.sub_agents!.map((r) => (
                  <div key={r.name} className="flex items-start gap-2.5 text-meta2">
                    <span className="shrink-0 rounded-full bg-stone-100 border border-stone-200 px-2 py-0.5 text-meta font-medium text-stone-600">
                      {r.role} · {r.name} · {r.status}
                    </span>
                    <span className="text-stone-600">
                      {r.input}
                      {r.citations.length > 0 && <span className="text-stone-400"> · {r.citations.length} sources</span>}
                      <span className="text-stone-400"> · {r.tokens_used.toLocaleString()} tokens</span>
                    </span>
                  </div>
                ))}
                {(autoRun.conflicts?.length ?? 0) > 0 && (
                  <p className="text-meta2 text-[#a02c2c]">⚠ Conflicts flagged (see the reconciliation section in the report): {autoRun.conflicts!.join(' · ')}</p>
                )}
              </div>
            )}
            <div className="space-y-1.5 mb-4">
              {(autoRun.steps ?? []).map((s) => (
                <div key={s.n} className="flex items-start gap-2.5 text-meta2">
                  <span className="shrink-0 rounded-full bg-stone-100 border border-stone-200 px-2 py-0.5 text-meta font-medium text-stone-600">
                    {s.n}. {s.action.replace('_', ' ')}
                  </span>
                  <span className="text-stone-600">
                    {s.input || s.reason}
                    {s.citations.length > 0 && <span className="text-stone-400"> · {s.citations.length} sources</span>}
                  </span>
                </div>
              ))}
              <p className="text-meta text-stone-400">
                Stopped: {autoRun.stopped === 'finished' ? 'goal complete' : autoRun.stopped === 'budget' ? `step budget (${autoRun.max_steps}) reached` : autoRun.stopped}
              </p>
            </div>
            <div className="rounded-lex-lg border border-stone-200 bg-ivory p-5">
              <div className="prose-lex">
                <ReactMarkdown
                  remarkPlugins={[remarkGfm]}
                  components={citeMarkdownComponents(autoRun.citations, setModalCitation)}
                >
                  {linkifyCitations(autoRun.final_report, autoRun.citations)}
                </ReactMarkdown>
              </div>
            </div>
            {autoRun.citations.length > 0 && (
              <div className="grid gap-2 mt-4">
                {autoRun.citations.map((c) => <CitationCard key={c.chunk_id} citation={c} go={go} />)}
              </div>
            )}
          </div>
        )}
      </div>

      {error && (
        <div className="mt-5 rounded-lex border border-brand-red/30 bg-[#feeaea] px-4 py-3 text-meta2 text-[#a02c2c]">{error}</div>
      )}
      {start.isPending && <Spinner label="Agents are working — each step grounds its findings in the corpus…" />}

      {run && !start.isPending && (
        <div className="mt-8 fade-up">
          <h2 className="font-display text-2xl tracking-tight">{run.workflow_name}</h2>
          <div className="mt-4 rounded-lex-lg border border-stone-200 bg-white shadow-lex p-6">
            <div className="prose-lex">
              <ReactMarkdown
                remarkPlugins={[remarkGfm]}
                components={citeMarkdownComponents(run.citations, setModalCitation)}
              >
                {linkifyCitations(run.final_report, run.citations)}
              </ReactMarkdown>
            </div>
          </div>

          <h3 className="font-display text-xl tracking-tight mt-8 mb-3">Step findings</h3>
          <div className="space-y-2">
            {run.steps.map((s, i) => (
              <div key={s.step_id} className="rounded-lex-lg border border-stone-200 bg-white shadow-lex overflow-hidden">
                <button
                  onClick={() => setOpenStep(openStep === i ? null : i)}
                  className="w-full flex items-center justify-between px-5 py-3.5 text-left"
                >
                  <span className="font-medium text-[15px]">{i + 1}. {s.name}</span>
                  <span className="flex items-center gap-2 text-meta text-stone-400">
                    {s.citations.length} citations
                    <ChevronDown size={15} className={`transition-transform ${openStep === i ? 'rotate-180' : ''}`} />
                  </span>
                </button>
                {openStep === i && (
                  <div className="px-5 pb-4 border-t border-stone-200 pt-3">
                    <p className="text-meta2 text-stone-700 leading-relaxed whitespace-pre-wrap">
                      <CitedText text={s.findings} citations={s.citations} onOpen={setModalCitation} />
                    </p>
                    {s.citations.length > 0 && (
                      <div className="grid gap-2 mt-3">
                        {s.citations.map((c) => <CitationCard key={c.chunk_id} citation={c} go={go} />)}
                      </div>
                    )}
                  </div>
                )}
              </div>
            ))}
          </div>

          {run.citations.length > 0 && (
            <div className="mt-8">
              <p className="text-meta uppercase tracking-wider text-stone-400 font-medium mb-2.5">All sources · {run.citations.length}</p>
              <div className="grid gap-2">
                {run.citations.map((c) => <CitationCard key={c.chunk_id} citation={c} go={go} />)}
              </div>
            </div>
          )}
        </div>
      )}
      {modalCitation && (
        <SourceModal citation={modalCitation} onClose={() => setModalCitation(null)} go={go} />
      )}
    </div>
  )
}
