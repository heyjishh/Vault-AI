import { useEffect, useState } from 'react'
import { useMutation, useQuery } from '@tanstack/react-query'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { Bot, ChevronDown, Compass, History, Pencil, Play, Plus, Wrench, X } from 'lucide-react'
import { mattersApi, ragApi, vaultApi } from '../api/client'
import { useSession } from '../stores/session'
import { RefineControls, useRefine } from './RefineControls'
import { ComposerLibraryButtons, promptInsert } from './PromptPicker'
import type { AutonomousRun, Citation, EditRecord, PassageDiff, PlanBranch, PlannedStep, RunRecord, ToolRun, ToolTraceEntry, WorkflowDef, WorkflowRun } from '../api/types'
import { CitationCard, EmptyState, Spinner } from './ui'
import { CitedText, SourceModal, citeMarkdownComponents, linkifyCitations } from './SourceModal'

/** Piece 84: the tool-call trace of a tool-agent run — every call the
 *  model made, what it cost, and whether it succeeded. */
function ToolTraceList({ trace }: { trace: ToolTraceEntry[] }) {
  if (trace.length === 0) return null
  return (
    <div className="space-y-1.5">
      {trace.map((t) => (
        <div key={t.n} className="flex items-start gap-2.5 text-meta2">
          <span className="shrink-0 text-meta font-medium text-stone-600 dark:text-stone-300">
            {t.n}. {t.tool.replace('_', ' ')} · {t.status}
          </span>
          <span className="text-stone-600 dark:text-stone-300 min-w-0">
            {t.args_digest && <span className="break-words">{t.args_digest}</span>}
            {t.error && <span className="text-[#a02c2c]"> — {t.error}</span>}
            {t.child && (
              <span className="text-stone-400 dark:text-stone-500">
                {' '}· sub-agent “{t.child.goal}” stopped: {t.child.stopped}, {t.child.steps} trace entries
              </span>
            )}
            <span className="text-stone-400 dark:text-stone-500">
              {' '}· {t.result_chars.toLocaleString()} chars · ~{t.tokens_estimate.toLocaleString()} tok · {t.duration_ms} ms
            </span>
          </span>
        </div>
      ))}
    </div>
  )
}

/** Piece 85: a passage diff an approver can actually read. */
function DiffView({ diff }: { diff: PassageDiff }) {
  if (diff.added.length === 0 && diff.removed.length === 0) {
    return <p className="text-meta2 text-stone-500 dark:text-stone-400">No textual change.</p>
  }
  const cap = (xs: string[]) => xs.slice(0, 8)
  return (
    <div className="space-y-1.5">
      {cap(diff.removed).map((p, i) => (
        <p key={`r${i}`} className="rounded-lex bg-[#feeaea] dark:bg-[#2a1212] px-3 py-2 text-meta2 text-[#7a2020] dark:text-[#e8a0a0] whitespace-pre-wrap">− {p}</p>
      ))}
      {diff.removed.length > 8 && <p className="text-meta text-stone-400">…and {diff.removed.length - 8} more removed passage(s)</p>}
      {cap(diff.added).map((p, i) => (
        <p key={`a${i}`} className="rounded-lex bg-[#e9f7ec] dark:bg-[#0f2416] px-3 py-2 text-meta2 text-[#205c2a] dark:text-[#9fd3a8] whitespace-pre-wrap">+ {p}</p>
      ))}
      {diff.added.length > 8 && <p className="text-meta text-stone-400">…and {diff.added.length - 8} more added passage(s)</p>}
    </div>
  )
}

const ICONS: Record<string, string> = {
  contract_review: 'Review a contract clause-by-clause for risk, obligations and missing protections.',
  due_diligence: 'Run diligence across documents and the web for red flags and exposure.',
  memo_draft: 'Draft a structured IRAC legal memo from the corpus.',
  counter_argument: 'Argue the other side at full strength, then draft your point-by-point rebuttal.',
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

  // Prompt refinement (Piece 13) for the workflow input.
  const refineWf = useRefine({ value: input, setValue: setInput, surface: 'agents' })

  const [wfBudget, setWfBudget] = useState('')
  const [historyTick, setHistoryTick] = useState(0)
  const start = useMutation({
    mutationFn: () => ragApi.runWorkflow(selected!.id, input.trim(), refineWf.consumeTerms(), wfBudget ? Number(wfBudget) : undefined),
    onSuccess: (r) => { setRun(r); setError(null); setOpenStep(null); setHistoryTick((t) => t + 1) },
    onError: (e) => setError((e as Error).message),
  })

  // Autonomous mode: the planner chooses its own vault/web steps.
  const [goal, setGoal] = useState('')
  const [autoMatter, setAutoMatter] = useState('')
  const [autoSteps, setAutoSteps] = useState(6)
  const [autoRun, setAutoRun] = useState<AutonomousRun | null>(null)
  const [autoError, setAutoError] = useState<string | null>(null)
  const matters = useQuery({ queryKey: ['matters'], queryFn: mattersApi.list })
  // Prompt refinement (Piece 13) for the autonomous goal.
  const refineAuto = useRefine({ value: goal, setValue: setGoal, surface: 'agents' })
  const [autoBudget, setAutoBudget] = useState('')
  const [plan, setPlan] = useState<PlanBranch[] | null>(null)
  const startAuto = useMutation({
    mutationFn: () => ragApi.runAutonomous(goal.trim(), autoMatter || undefined, autoSteps, refineAuto.consumeTerms(), { max_tokens: autoBudget ? Number(autoBudget) : undefined }),
    onSuccess: (r) => { setAutoRun(r); setAutoError(null); setHistoryTick((t) => t + 1) },
    onError: (e) => setAutoError((e as Error).message),
  })
  // Plan approval (Piece 80): draft the lead's plan, edit it, then run
  // exactly the approved branches.
  const draftPlan = useMutation({
    mutationFn: () => ragApi.planAutonomous(goal.trim(), autoMatter || undefined),
    onSuccess: (r) => { setPlan(r.branches); setAutoError(null) },
    onError: (e) => setAutoError((e as Error).message),
  })
  const approvePlan = useMutation({
    mutationFn: () => ragApi.runAutonomous(goal.trim(), autoMatter || undefined, autoSteps, refineAuto.consumeTerms(), { max_tokens: autoBudget ? Number(autoBudget) : undefined, approved_branches: plan ?? undefined }),
    onSuccess: (r) => { setAutoRun(r); setAutoError(null); setPlan(null); setHistoryTick((t) => t + 1) },
    onError: (e) => setAutoError((e as Error).message),
  })

  // Tool agent (Piece 84): the model drives a schema-checked tool
  // loop — vault search, document reads, web research, graph queries,
  // and sub-agents — inside the same budget/approval guardrails.
  const [toolGoal, setToolGoal] = useState('')
  const [toolMatter, setToolMatter] = useState('')
  const [toolSteps, setToolSteps] = useState(12)
  const [toolBudget, setToolBudget] = useState('')
  const [toolPlan, setToolPlan] = useState<PlannedStep[] | null>(null)
  const [toolRun, setToolRun] = useState<ToolRun | null>(null)
  const [toolError, setToolError] = useState<string | null>(null)
  const draftToolPlan = useMutation({
    mutationFn: () => ragApi.planToolAgent(toolGoal.trim(), toolMatter || undefined),
    onSuccess: (r) => { setToolPlan(r.steps); setToolError(null) },
    onError: (e) => setToolError((e as Error).message),
  })
  const startTool = useMutation({
    mutationFn: () => ragApi.runToolAgent(toolGoal.trim(), toolMatter || undefined, {
      max_steps: toolSteps,
      max_tokens: toolBudget ? Number(toolBudget) : undefined,
      approved_plan: toolPlan ?? undefined,
    }),
    onSuccess: (r) => { setToolRun(r); setToolError(null); setToolPlan(null); setHistoryTick((t) => t + 1) },
    onError: (e) => setToolError((e as Error).message),
  })

  // Document edits (Piece 85): propose an edit, review its diff,
  // approve it into a new version — or reject it.
  const [editDocId, setEditDocId] = useState('')
  const [editInstruction, setEditInstruction] = useState('')
  const [editError, setEditError] = useState<string | null>(null)
  const [lastEdit, setLastEdit] = useState<EditRecord | null>(null)
  const [editTick, setEditTick] = useState(0)
  const vaultDocs = useQuery({ queryKey: ['vault-docs-for-edit'], queryFn: vaultApi.list })
  const pendingEdits = useQuery({ queryKey: ['edits-pending', editTick], queryFn: () => ragApi.listEdits('pending') })
  const docVersions = useQuery({
    queryKey: ['doc-versions', editDocId, editTick],
    queryFn: () => ragApi.documentVersions(editDocId),
    enabled: !!editDocId,
  })
  const proposeEdit = useMutation({
    mutationFn: () => ragApi.proposeEdit(editDocId, editInstruction.trim()),
    onSuccess: (r) => { setLastEdit(r); setEditError(null); setEditInstruction(''); setEditTick((t) => t + 1) },
    onError: (e) => setEditError((e as Error).message),
  })
  const decideEdit = useMutation({
    mutationFn: (args: { id: string; approve: boolean }) => args.approve ? ragApi.approveEdit(args.id) : ragApi.rejectEdit(args.id),
    onSuccess: (r) => { setLastEdit(r); setEditError(null); setEditTick((t) => t + 1); setHistoryTick((t) => t + 1) },
    onError: (e) => setEditError((e as Error).message),
  })

  // Run history (Piece 80): every workflow + autonomous run, recorded.
  const history = useQuery({ queryKey: ['agent-runs', historyTick], queryFn: () => ragApi.agentRuns(30) })
  const [openRun, setOpenRun] = useState<RunRecord | null>(null)
  const openHistoryRun = useMutation({
    mutationFn: (id: string) => ragApi.agentRun(id),
    onSuccess: (r) => setOpenRun(r),
  })

  return (
    <div className="w-full px-6 py-10">
      <h1 className="font-display text-[2rem] tracking-tight flex items-center gap-3">
        <Bot className="text-stone-500 dark:text-stone-400" /> Agents
      </h1>
      <p className="text-stone-500 dark:text-stone-400 mt-1.5 text-[15px]">
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
              className={`text-left rounded-lex-lg border bg-white dark:bg-[#0a1a12] p-5 transition-colors ${
                selected?.id === w.id ? 'border-ink' : 'border-stone-200 dark:border-white/10 hover:border-stone-300'
              }`}
            >
              <p className="font-display text-lg tracking-tight">{w.name}</p>
              <p className="text-meta2 text-stone-500 dark:text-stone-400 mt-1.5 leading-relaxed">{ICONS[w.id] ?? w.description}</p>
              <p className="text-meta text-stone-400 dark:text-stone-500 mt-3">{w.steps.length} steps</p>
            </button>
          ))}
        </div>
      )}

      {selected && (
        <div className="mt-6 rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] p-5 fade-up">
          <label className="block text-meta font-medium text-stone-600 dark:text-stone-300 mb-1.5">{selected.input_label}</label>
          <div className="running-border rounded-lex-lg border border-stone-300 dark:border-white/10 bg-white dark:bg-[#0a1a12]">
          <textarea
            id="agents-workflow-input"
            value={input} onChange={(e) => setInput(e.target.value)} rows={4}
            placeholder="Paste the contract text, deal summary, or legal question…"
            className="w-full resize-none px-4 pt-4 pb-2 text-[15px] outline-none placeholder:text-stone-400 bg-transparent"
          />
          <div className="flex items-center justify-between px-3 pb-3 pt-1">
            <div className="flex items-center gap-3">
              <ComposerLibraryButtons onPick={promptInsert(input, setInput, 'agents-workflow-input')} />
              <p className="text-meta text-stone-400 dark:text-stone-500">Steps: {selected.steps.map((s) => s.name).join(' → ')}</p>
              <label className="flex items-center gap-1.5 text-meta text-stone-500 dark:text-stone-400">
                Token budget
                <input type="number" min={0} step={1000} value={wfBudget} onChange={(e) => setWfBudget(e.target.value)} placeholder="∞"
                  className="w-20 rounded-lex border border-stone-300 dark:border-white/10 px-2 py-1 outline-none focus:border-ink bg-white dark:bg-[#0a1a12]" />
              </label>
              <RefineControls ctl={refineWf} disabled={!input.trim()} />
            </div>
            <button
              onClick={() => input.trim() && start.mutate()} disabled={start.isPending || !input.trim()}
              className="btn-brand flex items-center gap-2 rounded-lex text-ivory px-4 py-2 text-meta2 font-medium disabled:opacity-40"
            >
              <Play size={14} /> {start.isPending ? 'Running…' : 'Run workflow'}
            </button>
          </div>
          </div>
        </div>
      )}

      <div className="mt-8 rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] p-5">
        <h2 className="font-display text-xl tracking-tight flex items-center gap-2">
          <Compass size={18} className="text-stone-500 dark:text-stone-400" /> Autonomous research
        </h2>
        <p className="text-meta2 text-stone-500 dark:text-stone-400 mt-1">
          Give it a goal — a lead agent splits it across specialist researchers, a drafter, and an independent checker, then reconciles their work into a cited report. Fixed workflows below run a set recipe; this one chooses.
        </p>
        <div className="running-border mt-3 rounded-lex-lg border border-stone-300 dark:border-white/10 bg-white dark:bg-[#0a1a12]">
        <textarea
          id="agents-goal-input"
          value={goal} onChange={(e) => setGoal(e.target.value)} rows={3}
          placeholder="e.g. What does the vault say about compensation for delayed possession, and what is the current law on the web?"
          className="w-full resize-none px-4 pt-4 pb-2 text-[15px] outline-none placeholder:text-stone-400 bg-transparent"
        />
        <div className="flex flex-wrap items-center gap-3 px-3 pb-3 pt-1">
          <ComposerLibraryButtons onPick={promptInsert(goal, setGoal, 'agents-goal-input')} />
          <select value={autoMatter} onChange={(e) => setAutoMatter(e.target.value)}
            className="rounded-lex border border-stone-300 dark:border-white/10 px-2.5 py-2 text-meta2 bg-white dark:bg-[#0a1a12] outline-none focus:border-ink max-w-[220px]">
            <option value="">Whole vault</option>
            {(matters.data ?? []).map((m) => <option key={m.id} value={m.id}>{m.title}</option>)}
          </select>
          <label className="flex items-center gap-2 text-meta2 text-stone-600 dark:text-stone-300">
            Max steps
            <input type="number" min={1} max={10} value={autoSteps}
              onChange={(e) => setAutoSteps(Math.min(10, Math.max(1, Number(e.target.value) || 6)))}
              className="w-16 rounded-lex border border-stone-300 dark:border-white/10 px-2 py-1.5 outline-none focus:border-ink bg-white dark:bg-[#0a1a12]" />
          </label>
          <label className="flex items-center gap-2 text-meta2 text-stone-600 dark:text-stone-300">
            Token budget
            <input type="number" min={0} step={5000} value={autoBudget}
              onChange={(e) => setAutoBudget(e.target.value)} placeholder="∞"
              className="w-24 rounded-lex border border-stone-300 dark:border-white/10 px-2 py-1.5 outline-none focus:border-ink bg-white dark:bg-[#0a1a12]" />
          </label>
          <RefineControls ctl={refineAuto} disabled={!goal.trim()} />
          <button
            onClick={() => goal.trim() && draftPlan.mutate()} disabled={draftPlan.isPending || startAuto.isPending || !goal.trim()}
            className="ml-auto flex items-center gap-2 rounded-lex border border-stone-300 dark:border-white/10 px-4 py-2 text-meta2 font-medium disabled:opacity-40 hover:border-stone-400"
          >
            <Compass size={14} /> {draftPlan.isPending ? 'Drafting plan…' : 'Draft plan'}
          </button>
          <button
            onClick={() => goal.trim() && startAuto.mutate()} disabled={startAuto.isPending || !goal.trim()}
            className="btn-brand flex items-center gap-2 rounded-lex text-ivory px-4 py-2 text-meta2 font-medium disabled:opacity-40"
          >
            <Play size={14} /> {startAuto.isPending ? 'Researching…' : 'Run autonomous agent'}
          </button>
        </div>
        </div>
        {plan && (
          <div className="mt-4 rounded-lex-lg border border-stone-200 dark:border-white/10 bg-ivory dark:bg-[#14120f] p-4 fade-up">
            <p className="text-meta2 font-medium text-stone-700 dark:text-stone-200">Draft plan — approve, edit, or discard</p>
            <p className="text-meta text-stone-500 dark:text-stone-400 mt-1">
              The lead drafted these branches from a scoping pass over the vault. The run will execute exactly this list — nothing more, nothing re-planned mid-run.
            </p>
            <div className="space-y-2 mt-3">
              {plan.map((b, i) => (
                <div key={i} className="flex items-center gap-2">
                  <span className="text-meta text-stone-400 dark:text-stone-500 w-5 shrink-0">{i + 1}.</span>
                  <input value={b.query}
                    onChange={(e) => setPlan(plan.map((x, j) => j === i ? { ...x, query: e.target.value } : x))}
                    className="flex-1 rounded-lex border border-stone-300 dark:border-white/10 px-2.5 py-1.5 text-meta2 outline-none focus:border-ink bg-white dark:bg-[#0a1a12]" />
                  <input value={b.why} placeholder="Why this branch?"
                    onChange={(e) => setPlan(plan.map((x, j) => j === i ? { ...x, why: e.target.value } : x))}
                    className="w-52 rounded-lex border border-stone-300 dark:border-white/10 px-2.5 py-1.5 text-meta2 outline-none focus:border-ink bg-white dark:bg-[#0a1a12]" />
                  <button onClick={() => setPlan(plan.filter((_, j) => j !== i))}
                    className="text-stone-400 hover:text-[#a02c2c] shrink-0" title="Remove branch">
                    <X size={15} />
                  </button>
                </div>
              ))}
            </div>
            <div className="flex items-center gap-2 mt-3">
              <button onClick={() => setPlan([...plan, { query: '', why: '' }])}
                className="flex items-center gap-1.5 rounded-lex border border-stone-300 dark:border-white/10 px-3 py-1.5 text-meta2 font-medium hover:border-stone-400">
                <Plus size={13} /> Add branch
              </button>
              <span className="flex-1" />
              <button onClick={() => setPlan(null)}
                className="rounded-lex px-3 py-1.5 text-meta2 text-stone-500 dark:text-stone-400 hover:text-stone-700">
                Discard
              </button>
              <button onClick={() => plan.some((b) => b.query.trim()) && approvePlan.mutate()}
                disabled={approvePlan.isPending || !plan.some((b) => b.query.trim())}
                className="btn-brand flex items-center gap-2 rounded-lex text-ivory px-4 py-2 text-meta2 font-medium disabled:opacity-40">
                <Play size={14} /> {approvePlan.isPending ? 'Running approved plan…' : 'Approve & run'}
              </button>
            </div>
          </div>
        )}
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
                    <span className="shrink-0 text-meta font-medium text-stone-600 dark:text-stone-300">
                      {r.role} · {r.name} · {r.status}
                    </span>
                    <span className="text-stone-600 dark:text-stone-300">
                      {r.input}
                      {r.citations.length > 0 && <span className="text-stone-400 dark:text-stone-500"> · {r.citations.length} sources</span>}
                      <span className="text-stone-400 dark:text-stone-500"> · {r.tokens_used.toLocaleString()} tokens</span>
                    </span>
                  </div>
                ))}
                {(autoRun.conflicts?.length ?? 0) > 0 && (
                  <p className="text-meta2 text-[#a02c2c]">Conflicts flagged (see the reconciliation section in the report): {autoRun.conflicts!.join(' · ')}</p>
                )}
              </div>
            )}
            <div className="space-y-1.5 mb-4">
              {(autoRun.steps ?? []).map((s) => (
                <div key={s.n} className="flex items-start gap-2.5 text-meta2">
                  <span className="shrink-0 text-meta font-medium text-stone-600 dark:text-stone-300">
                    {s.n}. {s.action.replace('_', ' ')}
                  </span>
                  <span className="text-stone-600 dark:text-stone-300">
                    {s.input || s.reason}
                    {s.citations.length > 0 && <span className="text-stone-400 dark:text-stone-500"> · {s.citations.length} sources</span>}
                  </span>
                </div>
              ))}
              <p className="text-meta text-stone-400 dark:text-stone-500">
                Stopped: {autoRun.stopped === 'finished' ? 'goal complete' : autoRun.stopped === 'budget' ? `step budget (${autoRun.max_steps}) reached` : autoRun.stopped}
              </p>
            </div>
            <div className="rounded-lex-lg border border-stone-200 dark:border-white/10 bg-ivory dark:bg-[#14120f] p-5">
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

      <div className="mt-8 rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] p-5">
        <h2 className="font-display text-xl tracking-tight flex items-center gap-2">
          <Wrench size={18} className="text-stone-500 dark:text-stone-400" /> Tool agent
        </h2>
        <p className="text-meta2 text-stone-500 dark:text-stone-400 mt-1">
          Give it a goal and the agent works it with tools — searching the vault, reading documents, researching the web, querying the knowledge graph, and delegating branches to sub-agents — adapting as results come back. Every call is schema-checked, budget-capped, and recorded in the run trace below and in Run history.
        </p>
        <div className="running-border mt-3 rounded-lex-lg border border-stone-300 dark:border-white/10 bg-white dark:bg-[#0a1a12]">
          <textarea
            id="agents-tool-goal-input"
            value={toolGoal} onChange={(e) => setToolGoal(e.target.value)} rows={3}
            placeholder="e.g. List the documents in this workspace, read the tenancy note, and use a sub-agent to check what the vault says about deposit refunds."
            className="w-full resize-none px-4 pt-4 pb-2 text-[15px] outline-none placeholder:text-stone-400 bg-transparent"
          />
          <div className="flex flex-wrap items-center gap-3 px-3 pb-3 pt-1">
            <select value={toolMatter} onChange={(e) => setToolMatter(e.target.value)}
              className="rounded-lex border border-stone-300 dark:border-white/10 px-2.5 py-2 text-meta2 bg-white dark:bg-[#0a1a12] outline-none focus:border-ink max-w-[220px]">
              <option value="">Whole vault</option>
              {(matters.data ?? []).map((m) => <option key={m.id} value={m.id}>{m.title}</option>)}
            </select>
            <label className="flex items-center gap-2 text-meta2 text-stone-600 dark:text-stone-300">
              Max tool calls
              <input type="number" min={1} max={24} value={toolSteps}
                onChange={(e) => setToolSteps(Math.min(24, Math.max(1, Number(e.target.value) || 12)))}
                className="w-16 rounded-lex border border-stone-300 dark:border-white/10 px-2 py-1.5 outline-none focus:border-ink bg-white dark:bg-[#0a1a12]" />
            </label>
            <label className="flex items-center gap-2 text-meta2 text-stone-600 dark:text-stone-300">
              Token budget
              <input type="number" min={0} step={5000} value={toolBudget}
                onChange={(e) => setToolBudget(e.target.value)} placeholder="∞"
                className="w-24 rounded-lex border border-stone-300 dark:border-white/10 px-2 py-1.5 outline-none focus:border-ink bg-white dark:bg-[#0a1a12]" />
            </label>
            <button
              onClick={() => toolGoal.trim() && draftToolPlan.mutate()} disabled={draftToolPlan.isPending || startTool.isPending || !toolGoal.trim()}
              className="ml-auto flex items-center gap-2 rounded-lex border border-stone-300 dark:border-white/10 px-4 py-2 text-meta2 font-medium disabled:opacity-40 hover:border-stone-400"
            >
              <Compass size={14} /> {draftToolPlan.isPending ? 'Drafting plan…' : 'Draft plan'}
            </button>
            <button
              onClick={() => toolGoal.trim() && startTool.mutate()} disabled={startTool.isPending || !toolGoal.trim()}
              className="btn-brand flex items-center gap-2 rounded-lex text-ivory px-4 py-2 text-meta2 font-medium disabled:opacity-40"
            >
              <Play size={14} /> {startTool.isPending ? 'Working…' : 'Run tool agent'}
            </button>
          </div>
        </div>
        {toolPlan && (
          <div className="mt-4 rounded-lex-lg border border-stone-200 dark:border-white/10 bg-ivory dark:bg-[#14120f] p-4 fade-up">
            <p className="text-meta2 font-medium text-stone-700 dark:text-stone-200">Draft plan — approve, edit, or discard</p>
            <p className="text-meta text-stone-500 dark:text-stone-400 mt-1">
              The planner drafted these steps from a scoping pass over the vault. The tool agent will follow this plan with its tools, adapting queries as results come back.
            </p>
            <div className="space-y-2 mt-3">
              {toolPlan.map((s, i) => (
                <div key={i} className="flex items-center gap-2">
                  <span className="text-meta text-stone-400 dark:text-stone-500 w-5 shrink-0">{i + 1}.</span>
                  <select value={s.action}
                    onChange={(e) => setToolPlan(toolPlan.map((x, j) => j === i ? { ...x, action: e.target.value } : x))}
                    className="rounded-lex border border-stone-300 dark:border-white/10 px-2 py-1.5 text-meta2 outline-none focus:border-ink bg-white dark:bg-[#0a1a12]">
                    <option value="search_vault">search vault</option>
                    <option value="search_web">search web</option>
                  </select>
                  <input value={s.query}
                    onChange={(e) => setToolPlan(toolPlan.map((x, j) => j === i ? { ...x, query: e.target.value } : x))}
                    className="flex-1 rounded-lex border border-stone-300 dark:border-white/10 px-2.5 py-1.5 text-meta2 outline-none focus:border-ink bg-white dark:bg-[#0a1a12]" />
                  <input value={s.reason} placeholder="Why this step?"
                    onChange={(e) => setToolPlan(toolPlan.map((x, j) => j === i ? { ...x, reason: e.target.value } : x))}
                    className="w-52 rounded-lex border border-stone-300 dark:border-white/10 px-2.5 py-1.5 text-meta2 outline-none focus:border-ink bg-white dark:bg-[#0a1a12]" />
                  <button onClick={() => setToolPlan(toolPlan.filter((_, j) => j !== i))}
                    className="text-stone-400 hover:text-[#a02c2c] shrink-0" title="Remove step">
                    <X size={15} />
                  </button>
                </div>
              ))}
            </div>
            <div className="flex items-center gap-2 mt-3">
              <button onClick={() => setToolPlan([...toolPlan, { action: 'search_vault', query: '', reason: '' }])}
                className="flex items-center gap-1.5 rounded-lex border border-stone-300 dark:border-white/10 px-3 py-1.5 text-meta2 font-medium hover:border-stone-400">
                <Plus size={13} /> Add step
              </button>
              <span className="flex-1" />
              <button onClick={() => setToolPlan(null)}
                className="rounded-lex px-3 py-1.5 text-meta2 text-stone-500 dark:text-stone-400 hover:text-stone-700">
                Discard
              </button>
              <button onClick={() => toolPlan.some((s) => s.query.trim()) && startTool.mutate()}
                disabled={startTool.isPending || !toolPlan.some((s) => s.query.trim())}
                className="btn-brand flex items-center gap-2 rounded-lex text-ivory px-4 py-2 text-meta2 font-medium disabled:opacity-40">
                <Play size={14} /> {startTool.isPending ? 'Working…' : 'Approve & run'}
              </button>
            </div>
          </div>
        )}
        {toolError && (
          <div className="mt-4 rounded-lex border border-brand-red/30 bg-[#feeaea] px-4 py-3 text-meta2 text-[#a02c2c]">{toolError}</div>
        )}
        {startTool.isPending && <Spinner label="The tool agent is working — calling tools and reading results…" />}
        {toolRun && !startTool.isPending && (
          <div className="mt-5 fade-up">
            <div className="mb-4">
              <ToolTraceList trace={toolRun.trace} />
              <p className="text-meta text-stone-400 dark:text-stone-500 mt-2">
                Stopped: {toolRun.stopped === 'finished' ? 'goal complete' : toolRun.stopped} · estimated spend ~{toolRun.tokens_estimate.toLocaleString()} tokens
              </p>
            </div>
            <div className="rounded-lex-lg border border-stone-200 dark:border-white/10 bg-ivory dark:bg-[#14120f] p-5">
              <div className="prose-lex">
                <ReactMarkdown
                  remarkPlugins={[remarkGfm]}
                  components={citeMarkdownComponents(toolRun.citations, setModalCitation)}
                >
                  {linkifyCitations(toolRun.final_report, toolRun.citations)}
                </ReactMarkdown>
              </div>
            </div>
            {toolRun.citations.length > 0 && (
              <div className="grid gap-2 mt-4">
                {toolRun.citations.map((c) => <CitationCard key={c.chunk_id} citation={c} go={go} />)}
              </div>
            )}
          </div>
        )}
      </div>

      <div className="mt-8 rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] p-5">
        <h2 className="font-display text-xl tracking-tight flex items-center gap-2">
          <Pencil size={18} className="text-stone-500 dark:text-stone-400" /> Document edits
        </h2>
        <p className="text-meta2 text-stone-500 dark:text-stone-400 mt-1">
          Hand the agent a document and an instruction — it drafts the edit and shows you the diff. Nothing is written back until you approve; approval files a new version in the document's Workspaces, and the original stays in the vault, downloadable, in the version list.
        </p>
        <div className="flex flex-wrap items-center gap-3 mt-4">
          <select value={editDocId} onChange={(e) => setEditDocId(e.target.value)}
            className="rounded-lex border border-stone-300 dark:border-white/10 px-2.5 py-2 text-meta2 bg-white dark:bg-[#0a1a12] outline-none focus:border-ink max-w-[320px]">
            <option value="">Choose a document…</option>
            {(vaultDocs.data ?? []).map((d) => <option key={d.id} value={d.id}>{d.title}</option>)}
          </select>
          <input value={editInstruction} onChange={(e) => setEditInstruction(e.target.value)}
            placeholder="e.g. Change the deposit from one month of rent to two months"
            className="flex-1 min-w-[240px] rounded-lex border border-stone-300 dark:border-white/10 px-3 py-2 text-meta2 outline-none focus:border-ink bg-white dark:bg-[#0a1a12]" />
          <button
            onClick={() => editDocId && editInstruction.trim() && proposeEdit.mutate()}
            disabled={proposeEdit.isPending || !editDocId || !editInstruction.trim()}
            className="btn-brand flex items-center gap-2 rounded-lex text-ivory px-4 py-2 text-meta2 font-medium disabled:opacity-40"
          >
            <Pencil size={14} /> {proposeEdit.isPending ? 'Drafting edit…' : 'Propose edit'}
          </button>
        </div>
        {editError && (
          <div className="mt-4 rounded-lex border border-brand-red/30 bg-[#feeaea] px-4 py-3 text-meta2 text-[#a02c2c]">{editError}</div>
        )}
        {editDocId && (docVersions.data?.length ?? 0) > 0 && (
          <div className="mt-4">
            <p className="text-meta2 font-medium text-stone-700 dark:text-stone-200 mb-2">Versions</p>
            <div className="space-y-1.5">
              {docVersions.data!.map((v) => (
                <div key={v.doc_id} className="flex items-start gap-2.5 text-meta2">
                  <span className="shrink-0 text-meta font-medium text-stone-600 dark:text-stone-300">v{v.version}{v.current ? ' · current' : ''}</span>
                  <span className="text-stone-600 dark:text-stone-300">
                    {v.title}
                    {v.receipt && <span className="block text-meta text-stone-400 dark:text-stone-500">{v.receipt}</span>}
                  </span>
                </div>
              ))}
            </div>
          </div>
        )}
        {lastEdit && lastEdit.status !== 'pending' && (
          <div className="mt-4 rounded-lex border border-stone-200 dark:border-white/10 bg-ivory dark:bg-[#14120f] px-4 py-3 fade-up">
            <p className="text-meta2 text-stone-700 dark:text-stone-200">
              <span className="font-medium">{lastEdit.doc_title}</span> — edit {lastEdit.status}
              {lastEdit.version ? ` as version ${lastEdit.version}` : ''}.
            </p>
            {lastEdit.receipt && <p className="text-meta text-stone-500 dark:text-stone-400 mt-1">{lastEdit.receipt}</p>}
          </div>
        )}
        {(pendingEdits.data?.length ?? 0) > 0 && (
          <div className="mt-5">
            <p className="text-meta2 font-medium text-stone-700 dark:text-stone-200 mb-2">Pending approval · {pendingEdits.data!.length}</p>
            <div className="space-y-3">
              {pendingEdits.data!.map((e) => (
                <div key={e.id} className="rounded-lex-lg border border-stone-200 dark:border-white/10 bg-ivory dark:bg-[#14120f] p-4">
                  <div className="flex flex-wrap items-start gap-2">
                    <div className="flex-1 min-w-[200px]">
                      <p className="text-meta2 font-medium text-stone-800 dark:text-stone-100">{e.doc_title}{e.source === 'private' ? ' · Confidential Vault' : ''}</p>
                      <p className="text-meta text-stone-500 dark:text-stone-400 mt-0.5">“{e.instruction}” · {e.diff.added.length} passage(s) added, {e.diff.removed.length} removed · {new Date(e.created_at).toLocaleString()}</p>
                    </div>
                    <button onClick={() => decideEdit.mutate({ id: e.id, approve: false })} disabled={decideEdit.isPending}
                      className="rounded-lex px-3 py-1.5 text-meta2 text-stone-500 dark:text-stone-400 hover:text-[#a02c2c] disabled:opacity-40">
                      Reject
                    </button>
                    <button onClick={() => decideEdit.mutate({ id: e.id, approve: true })} disabled={decideEdit.isPending}
                      className="btn-brand flex items-center gap-2 rounded-lex text-ivory px-4 py-2 text-meta2 font-medium disabled:opacity-40">
                      <Play size={14} /> {decideEdit.isPending ? 'Writing back…' : 'Approve & write back'}
                    </button>
                  </div>
                  <div className="mt-3"><DiffView diff={e.diff} /></div>
                </div>
              ))}
            </div>
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
          {run.stopped === 'budget_tokens' ? (
            <p className="mt-2 rounded-lex border border-[#e5c07b]/50 bg-[#fdf6e0] dark:bg-[#2a2110] px-4 py-2.5 text-meta2 text-[#7a5b12] dark:text-[#e5c07b]">
              Stopped early — the token budget was reached after {run.steps.length} step(s). This report covers only the completed steps. Estimated spend: {(run.tokens_estimate ?? 0).toLocaleString()} tokens.
            </p>
          ) : (
            <p className="text-meta text-stone-400 dark:text-stone-500 mt-1.5">Estimated spend: {(run.tokens_estimate ?? 0).toLocaleString()} tokens</p>
          )}
          <div className="mt-4 rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] p-6">
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
              <div key={s.step_id} className="rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] overflow-hidden">
                <button
                  onClick={() => setOpenStep(openStep === i ? null : i)}
                  className="w-full flex items-center justify-between px-5 py-3.5 text-left"
                >
                  <span className="font-medium text-[15px]">{i + 1}. {s.name}</span>
                  <span className="flex items-center gap-2 text-meta text-stone-400 dark:text-stone-500">
                    {s.citations.length} citations
                    <ChevronDown size={15} className={`transition-transform ${openStep === i ? 'rotate-180' : ''}`} />
                  </span>
                </button>
                {openStep === i && (
                  <div className="px-5 pb-4 border-t border-stone-200 dark:border-white/10 pt-3">
                    <p className="text-meta2 text-stone-700 dark:text-stone-200 leading-relaxed whitespace-pre-wrap">
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
              <p className="text-[13px] text-stone-400 dark:text-stone-500 font-medium mb-2.5">All sources · {run.citations.length}</p>
              <div className="grid gap-2">
                {run.citations.map((c) => <CitationCard key={c.chunk_id} citation={c} go={go} />)}
              </div>
            </div>
          )}
        </div>
      )}
      <div className="mt-10">
        <h2 className="font-display text-xl tracking-tight flex items-center gap-2">
          <History size={18} className="text-stone-500 dark:text-stone-400" /> Run history
        </h2>
        <p className="text-meta text-stone-500 dark:text-stone-400 mt-1">Every workflow and autonomous run is recorded — status, steps, tokens, and the full report.</p>
        {history.isLoading ? (
          <Spinner label="Loading run history…" />
        ) : !history.data || history.data.length === 0 ? (
          <p className="text-meta2 text-stone-400 dark:text-stone-500 mt-3">No runs recorded yet.</p>
        ) : (
          <div className="mt-3 rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] divide-y divide-stone-100 dark:divide-white/5 overflow-hidden">
            {history.data.map((r) => (
              <button key={r.id} onClick={() => openHistoryRun.mutate(r.id)}
                className="w-full flex items-center gap-3 px-4 py-3 text-left hover:bg-stone-50 dark:hover:bg-white/5">
                <span className="shrink-0 text-meta font-medium px-2 py-0.5 rounded-full border border-stone-200 dark:border-white/10 text-stone-600 dark:text-stone-300">
                  {r.kind === 'workflow' ? 'Workflow' : r.kind === 'autonomous_orchestrated' ? 'Autonomous · team' : r.kind === 'autonomous_tools' ? 'Tool agent' : r.kind === 'document_edit' ? 'Document edit' : 'Autonomous'}
                </span>
                <span className="flex-1 min-w-0">
                  <span className="block text-meta2 font-medium text-stone-800 dark:text-stone-100 truncate">{r.name}</span>
                  <span className="block text-meta text-stone-400 dark:text-stone-500 truncate">{r.input}</span>
                </span>
                <span className={`shrink-0 text-meta font-medium ${r.status === 'completed' ? 'text-[#2e7d32] dark:text-[#7bc47f]' : r.status === 'failed' ? 'text-[#a02c2c]' : 'text-[#7a5b12] dark:text-[#e5c07b]'}`}>
                  {r.status.replace('_', ' ')}
                </span>
                <span className="shrink-0 text-meta text-stone-400 dark:text-stone-500">{r.steps} steps · {r.tokens.toLocaleString()} tok</span>
                <span className="shrink-0 text-meta text-stone-400 dark:text-stone-500">{new Date(r.created_at).toLocaleString()}</span>
              </button>
            ))}
          </div>
        )}
        {openRun && (
          <div className="mt-4 rounded-lex-lg border border-stone-200 dark:border-white/10 bg-ivory dark:bg-[#14120f] p-5 fade-up">
            <div className="flex items-start justify-between gap-3">
              <div>
                <p className="font-display text-lg tracking-tight">{openRun.name}</p>
                <p className="text-meta text-stone-400 dark:text-stone-500 mt-0.5">
                  {openRun.status.replace('_', ' ')} · {openRun.steps} steps · {openRun.tokens.toLocaleString()} tokens · {new Date(openRun.created_at).toLocaleString()}
                </p>
                <p className="text-meta2 text-stone-600 dark:text-stone-300 mt-1.5">{openRun.input}</p>
              </div>
              <button onClick={() => setOpenRun(null)} className="text-stone-400 hover:text-stone-600 shrink-0"><X size={16} /></button>
            </div>
            {openRun.error && (
              <p className="mt-3 rounded-lex border border-brand-red/30 bg-[#feeaea] px-4 py-2.5 text-meta2 text-[#a02c2c]">{openRun.error}</p>
            )}
            {(openRun.trace?.length ?? 0) > 0 && (
              <div className="mt-4">
                <p className="text-meta2 font-medium text-stone-700 dark:text-stone-200 mb-2">Tool trace</p>
                <ToolTraceList trace={openRun.trace!} />
              </div>
            )}
            {openRun.report && (
              <div className="prose-lex mt-3">
                <ReactMarkdown remarkPlugins={[remarkGfm]}>{openRun.report}</ReactMarkdown>
              </div>
            )}
          </div>
        )}
      </div>
      {modalCitation && (
        <SourceModal citation={modalCitation} onClose={() => setModalCitation(null)} go={go} />
      )}
    </div>
  )
}
