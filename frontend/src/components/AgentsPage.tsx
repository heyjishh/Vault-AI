import { useEffect, useState } from 'react'
import { useMutation, useQuery } from '@tanstack/react-query'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { History, X } from 'lucide-react'
import { ragApi } from '../api/client'
import { useSession } from '../stores/session'
import type { RunRecord, WorkflowRun } from '../api/types'
import { CitationCard, Spinner } from './ui'
import { ToolTraceList } from './AgentShared'
import { AgentChatPanel, type AgentSuggestion } from './AgentChatPage'

/** Piece 89: the Agents page is THE one agent box. The workflow
 *  cards, the autonomous-research form, and the tool-agent form
 *  that used to divide this page are gone as inputs — their
 *  engines are tools inside the one loop (Piece 87/88), and these
 *  chips are how the page surfaces them: a chip puts a pre-steered
 *  prompt into the single composer; it is not a form. */
const SUGGESTIONS: AgentSuggestion[] = [
  { label: 'Contract risk review', prompt: 'Use the contract_review workflow to review this contract for risk, obligations, and missing protections: ' },
  { label: 'Due diligence', prompt: 'Use the due_diligence workflow to run due diligence on: ' },
  { label: 'Legal memo', prompt: 'Use the memo_draft workflow to draft a structured IRAC legal memo on: ' },
  { label: 'Counter-argument brief', prompt: 'Use the counter_argument workflow to argue the other side at full strength, then draft my point-by-point rebuttal, on: ' },
  { label: 'Deep research', prompt: 'Use deep research to investigate this question and give me a verdict-first report: ' },
  { label: 'Draft a document', prompt: 'Draft a legal notice for: ' },
  { label: 'Research team', prompt: 'Use the research team to investigate this goal and reconcile one cited report: ' },
]

function kindLabel(kind: string): string {
  switch (kind) {
    case 'workflow':
      return 'Workflow'
    case 'autonomous_orchestrated':
      return 'Research team'
    case 'autonomous_tools':
      return 'Agent'
    case 'document_edit':
      return 'Document edit'
    case 'autonomous':
      return 'Autonomous'
    default:
      return 'Agent'
  }
}

/** Run history (Piece 80), kept on the Agents page as a read-only
 *  section below the chat: every workflow, team, tool-agent, chat,
 *  and edit run — status, steps, tokens, and the full report. It
 *  is not an input. */
function RunHistory({ tick, go }: { tick: number; go?: (view: string) => void }) {
  const history = useQuery({ queryKey: ['agent-runs', tick], queryFn: () => ragApi.agentRuns(30) })
  const [openRun, setOpenRun] = useState<RunRecord | null>(null)
  const openHistoryRun = useMutation({
    mutationFn: (id: string) => ragApi.agentRun(id),
    onSuccess: (r) => setOpenRun(r),
  })

  // Dashboard/history restore: a past workflow run reopens here as
  // a read-only report (the run itself is never re-executed).
  const restore = useSession((st) => st.restore)
  const consumeRestore = useSession((st) => st.consumeRestore)
  const [restored, setRestored] = useState<WorkflowRun | null>(null)
  useEffect(() => {
    if (!restore || restore.kind !== 'workflow') return
    const p = consumeRestore('workflow')
    if (!p) return
    setRestored(p.run as WorkflowRun)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [restore])

  return (
    <div className="w-full px-6 pb-10">
      {restored && (
        <div className="mb-8 rounded-lex-lg border border-stone-200 dark:border-white/10 bg-ivory dark:bg-[#14120f] p-5 fade-up">
          <div className="flex items-start justify-between gap-3">
            <div>
              <p className="font-display text-lg tracking-tight">{restored.workflow_name}</p>
              <p className="text-meta text-stone-400 dark:text-stone-500 mt-0.5">
                Restored run · stopped: {restored.stopped} · ~{(restored.tokens_estimate ?? 0).toLocaleString()} tokens
              </p>
              <p className="text-meta2 text-stone-600 dark:text-stone-300 mt-1.5">{restored.input}</p>
            </div>
            <button onClick={() => setRestored(null)} className="text-stone-400 hover:text-stone-600 shrink-0"><X size={16} /></button>
          </div>
          <div className="prose-lex mt-3">
            <ReactMarkdown remarkPlugins={[remarkGfm]}>{restored.final_report}</ReactMarkdown>
          </div>
          {restored.citations.length > 0 && (
            <div className="grid gap-2 mt-4">
              {restored.citations.map((c) => <CitationCard key={c.chunk_id} citation={c} go={go} />)}
            </div>
          )}
        </div>
      )}

      <h2 className="font-display text-xl tracking-tight flex items-center gap-2">
        <History size={18} className="text-stone-500 dark:text-stone-400" /> Run history
      </h2>
      <p className="text-meta text-stone-500 dark:text-stone-400 mt-1">Every workflow and agent run is recorded — status, steps, tokens, and the full report.</p>
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
                {kindLabel(r.kind)}
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
  )
}

export default function AgentsPage({ go }: { go?: (view: string) => void } = {}) {
  const [historyTick, setHistoryTick] = useState(0)
  return (
    <>
      <AgentChatPanel
        go={go}
        heading="Agents"
        subheading="One agent, one box: ask a question, commission research, draft a document, run a workflow, or hand a goal to the research team — the agent picks its own tools and shows its work. Document edits are always proposed first and written back only on your approval."
        suggestions={SUGGESTIONS}
        onTurnDone={() => setHistoryTick((t) => t + 1)}
      />
      <RunHistory tick={historyTick} go={go} />
    </>
  )
}
