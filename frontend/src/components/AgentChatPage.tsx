import { useEffect, useMemo, useRef, useState } from 'react'
import { useMutation, useQuery } from '@tanstack/react-query'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { Bot, Check, Paperclip, Plus, Send, Square, X } from 'lucide-react'
import { agentChatStream, mattersApi, ragApi, vaultApi } from '../api/client'
import type { AgentApproval, AgentChatMessage, AgentChatThread, Citation, EditRecord } from '../api/types'
import { CitationCard } from './ui'
import { SourceModal, citeMarkdownComponents, linkifyCitations } from './SourceModal'
import { DiffView, ToolTraceList } from './AgentsPage'

/** One live stream event in the in-flight turn's activity feed. */
type LiveEvent =
  | { kind: 'call'; n: number; tool: string; digest: string }
  | { kind: 'result'; n: number; tool: string; status: string; summary: string }
  | { kind: 'sub'; n: number; phase: string; goal?: string; stopped?: string; steps?: number; tokens?: number }

interface LiveTurn {
  events: LiveEvent[]
  text: string
  citations: Citation[]
  approvals: AgentApproval[]
  status: string
  error: string | null
}

const EMPTY_LIVE: LiveTurn = { events: [], text: '', citations: [], approvals: [], status: '', error: null }

/** An edit awaiting (or having received) the user's decision.
 *  Pending shows the diff and the Approve / Reject buttons, wired
 *  to the Piece 85 decide endpoints; decided shows the receipt. */
function ApprovalCard({ approval, record, busy, onDecide }: {
  approval: AgentApproval
  record?: EditRecord
  busy: boolean
  onDecide: (id: string, approve: boolean) => void
}) {
  const status = record?.status ?? 'pending'
  return (
    <div className="rounded-lex-lg border border-amber-300/60 dark:border-amber-200/20 bg-amber-50 dark:bg-[#221a08] p-4">
      <p className="text-meta font-semibold text-stone-700 dark:text-stone-200">
        Pending your approval — edit to “{approval.doc_title}”
      </p>
      <p className="text-meta2 text-stone-500 dark:text-stone-400 mt-1">{approval.instruction}</p>
      <div className="mt-3">
        <DiffView diff={approval.diff} />
      </div>
      {status === 'pending' ? (
        <div className="flex items-center gap-2 mt-3">
          <button
            onClick={() => onDecide(approval.edit_id, true)}
            disabled={busy}
            className="btn-brand flex items-center gap-1.5 rounded-lex text-ivory px-3.5 py-1.5 text-meta2 font-medium disabled:opacity-40"
          >
            <Check size={14} /> Approve & write back
          </button>
          <button
            onClick={() => onDecide(approval.edit_id, false)}
            disabled={busy}
            className="flex items-center gap-1.5 rounded-lex border border-stone-300 dark:border-white/15 px-3.5 py-1.5 text-meta2 font-medium text-stone-600 dark:text-stone-300 disabled:opacity-40"
          >
            <X size={14} /> Reject
          </button>
          <span className="text-meta text-stone-400 dark:text-stone-500">Approval files a new version; the original stays in the vault.</span>
        </div>
      ) : (
        <p className="text-meta2 mt-3 text-stone-600 dark:text-stone-300">
          {status === 'approved' ? 'Approved' : 'Rejected'}
          {record?.receipt ? ` — ${record.receipt}` : ''}
        </p>
      )}
    </div>
  )
}

function LiveEventList({ events }: { events: LiveEvent[] }) {
  if (events.length === 0) return null
  return (
    <div className="space-y-1.5">
      {events.map((e, i) => {
        if (e.kind === 'call') {
          return (
            <p key={i} className="text-meta2 text-stone-600 dark:text-stone-300">
              <span className="font-medium">{e.n}. {e.tool.replace('_', ' ')}</span>
              {e.digest && <span className="text-stone-400 dark:text-stone-500"> — {e.digest}</span>}
            </p>
          )
        }
        if (e.kind === 'result') {
          return (
            <p key={i} className="text-meta2 pl-4 text-stone-500 dark:text-stone-400">
              → {e.summary} <span className="text-stone-400 dark:text-stone-500">({e.status})</span>
            </p>
          )
        }
        return (
          <p key={i} className="text-meta2 text-stone-600 dark:text-stone-300">
            <span className="font-medium">Sub-agent {e.phase}</span>
            {e.goal && <span> — “{e.goal}”</span>}
            {e.phase === 'completed' && (
              <span className="text-stone-400 dark:text-stone-500"> · stopped: {e.stopped} · {e.steps} steps · ~{(e.tokens ?? 0).toLocaleString()} tok</span>
            )}
          </p>
        )
      })}
    </div>
  )
}

export default function AgentChatPage({ go }: { go?: (view: string) => void } = {}) {
  const matters = useQuery({ queryKey: ['matters'], queryFn: mattersApi.list })
  const vaultDocs = useQuery({ queryKey: ['vault-docs-for-chat'], queryFn: vaultApi.list })
  const [editTick, setEditTick] = useState(0)
  const edits = useQuery({ queryKey: ['agent-chat-edits', editTick], queryFn: () => ragApi.listEdits() })
  const [threadTick, setThreadTick] = useState(0)
  const threads = useQuery({ queryKey: ['agent-chat-threads', threadTick], queryFn: ragApi.agentChatThreads })

  const [thread, setThread] = useState<AgentChatThread | null>(null)
  const [input, setInput] = useState('')
  const [matterId, setMatterId] = useState('')
  const [refDocIds, setRefDocIds] = useState<string[]>([])
  const [attachOpen, setAttachOpen] = useState(false)
  const [live, setLive] = useState<LiveTurn | null>(null)
  const [sendError, setSendError] = useState<string | null>(null)
  const [modalCitation, setModalCitation] = useState<Citation | null>(null)
  const [decided, setDecided] = useState<Record<string, EditRecord>>({})
  const abortRef = useRef<AbortController | null>(null)
  const endRef = useRef<HTMLDivElement | null>(null)

  const editsById = useMemo(() => {
    const map = new Map<string, EditRecord>()
    for (const e of edits.data ?? []) map.set(e.id, e)
    for (const [id, rec] of Object.entries(decided)) map.set(id, rec)
    return map
  }, [edits.data, decided])

  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: 'smooth', block: 'end' })
  }, [thread?.messages.length, live?.text, live?.events.length])

  async function openThread(id: string) {
    if (live) return
    const t = await ragApi.agentChatThread(id)
    setThread(t)
    setMatterId(t.matter_id ?? '')
    setRefDocIds([])
    setSendError(null)
  }

  function newChat() {
    if (live) return
    setThread(null)
    setInput('')
    setRefDocIds([])
    setSendError(null)
  }

  const decide = useMutation({
    mutationFn: (args: { id: string; approve: boolean }) =>
      args.approve ? ragApi.approveEdit(args.id) : ragApi.rejectEdit(args.id),
    onSuccess: (rec) => {
      setDecided((d) => ({ ...d, [rec.id]: rec }))
      setEditTick((t) => t + 1)
    },
    onError: (e) => setSendError((e as Error).message),
  })

  async function send() {
    const message = input.trim()
    if (!message || live) return
    setInput('')
    setSendError(null)
    // The user's message shows immediately; the persisted copy
    // replaces it when the turn completes.
    const optimistic: AgentChatMessage = {
      role: 'user',
      text: message,
      created_at: new Date().toISOString(),
      doc_ids: refDocIds,
    }
    setThread((t) =>
      t
        ? { ...t, messages: [...t.messages, optimistic] }
        : { id: '', title: message.slice(0, 80), matter_id: matterId || null, messages: [optimistic], created_at: optimistic.created_at, updated_at: optimistic.created_at },
    )
    setLive({ ...EMPTY_LIVE, status: 'Agent is working on your message…' })
    const controller = new AbortController()
    abortRef.current = controller
    let doneThreadId = ''
    try {
      await agentChatStream(
        { message, thread_id: thread?.id || undefined, matter_id: matterId || undefined, doc_ids: refDocIds },
        {
          onStatus: (m) => setLive((l) => (l ? { ...l, status: m } : l)),
          onToolCall: (n, tool, digest) =>
            setLive((l) => (l ? { ...l, events: [...l.events, { kind: 'call', n, tool, digest }] } : l)),
          onToolResult: (n, tool, status, summary) =>
            setLive((l) => (l ? { ...l, events: [...l.events, { kind: 'result', n, tool, status, summary }] } : l)),
          onSubAgent: (phase, n, goal, stopped, steps, tokens) =>
            setLive((l) => (l ? { ...l, events: [...l.events, { kind: 'sub', n, phase, goal, stopped, steps, tokens }] } : l)),
          onApprovalRequired: (approval) =>
            setLive((l) => (l ? { ...l, approvals: [...l.approvals, approval] } : l)),
          onSources: (citations) => setLive((l) => (l ? { ...l, citations } : l)),
          onDelta: (text) => setLive((l) => (l ? { ...l, text: l.text + text } : l)),
          onError: (m) => setLive((l) => (l ? { ...l, error: m } : l)),
          onDone: (d) => { doneThreadId = d.thread_id },
        },
        controller.signal,
      )
    } catch (e) {
      if ((e as Error).name !== 'AbortError') setSendError((e as Error).message)
    } finally {
      abortRef.current = null
      setLive(null)
      setRefDocIds([])
      setEditTick((t) => t + 1)
      setThreadTick((t) => t + 1)
      // Reload the authoritative thread (persisted messages, run
      // links, edit ids) — for a brand-new thread the id only
      // exists server-side, so it comes from the done event.
      const reloadId = doneThreadId || thread?.id
      if (reloadId) {
        const t = await ragApi.agentChatThread(reloadId).catch(() => null)
        if (t) setThread(t)
      }
    }
  }

  function renderMessage(m: AgentChatMessage, i: number) {
    if (m.role === 'user') {
      return (
        <div key={i} className="flex justify-end">
          <div className="max-w-[85%] rounded-lex-lg bg-ink text-ivory px-4 py-3">
            <p className="text-[15px] whitespace-pre-wrap">{m.text}</p>
            {!!m.doc_ids?.length && (
              <p className="text-meta mt-1.5 opacity-70">
                Referenced: {m.doc_ids.map((id) => (vaultDocs.data ?? []).find((d) => d.id === id)?.title ?? id).join(', ')}
              </p>
            )}
          </div>
        </div>
      )
    }
    const citations = m.citations ?? []
    const approvals = (m.edit_ids ?? [])
      .map((id) => editsById.get(id))
      .filter((r): r is EditRecord => !!r)
      .map((r) => ({ edit_id: r.id, doc_id: r.doc_id, doc_title: r.doc_title, instruction: r.instruction, diff: r.diff }))
    return (
      <div key={i} className="space-y-3">
        {!!m.trace?.length && (
          <details className="rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] px-4 py-3">
            <summary className="cursor-pointer text-meta2 font-medium text-stone-600 dark:text-stone-300">
              Agent activity — {m.trace.filter((t) => t.tool !== 'finish' && t.tool !== 'model').length} tool calls
            </summary>
            <div className="mt-3">
              <ToolTraceList trace={m.trace} />
            </div>
          </details>
        )}
        <div className="rounded-lex-lg border border-stone-200 dark:border-white/10 bg-ivory dark:bg-[#14120f] p-5">
          <div className="prose-lex">
            <ReactMarkdown remarkPlugins={[remarkGfm]} components={citeMarkdownComponents(citations, setModalCitation)}>
              {linkifyCitations(m.text, citations)}
            </ReactMarkdown>
          </div>
        </div>
        {approvals.map((a) => (
          <ApprovalCard key={a.edit_id} approval={a} record={editsById.get(a.edit_id)} busy={decide.isPending} onDecide={(id, approve) => decide.mutate({ id, approve })} />
        ))}
        {citations.length > 0 && (
          <div className="grid gap-2">
            {citations.map((c) => <CitationCard key={c.chunk_id} citation={c} go={go} />)}
          </div>
        )}
        <p className="text-meta text-stone-400 dark:text-stone-500">
          {m.stopped && m.stopped !== 'finished' ? `Stopped: ${m.stopped} · ` : ''}
          {(m.tokens ?? 0) > 0 ? `~${(m.tokens ?? 0).toLocaleString()} tokens billed` : ''}
          {m.run_id ? ` · run ${m.run_id.slice(0, 8)}` : ''}
        </p>
      </div>
    )
  }

  const docsInScope = (vaultDocs.data ?? [])
  return (
    <div className="w-full px-6 py-10">
      <h1 className="font-display text-[2rem] tracking-tight flex items-center gap-3">
        <Bot className="text-stone-500 dark:text-stone-400" /> Agent Chat
      </h1>
      <p className="text-stone-500 dark:text-stone-400 mt-1.5 text-[15px]">
        One box: ask a question, commission research, or tell it to change a document — the agent picks its own tools. Document edits are always proposed first and written back only on your approval.
      </p>

      <div className="flex flex-col lg:flex-row gap-6 mt-6 items-start">
        {/* Thread list */}
        <div className="w-full lg:w-72 shrink-0 rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] p-3">
          <button
            onClick={newChat}
            disabled={!!live}
            className="btn-brand w-full flex items-center justify-center gap-2 rounded-lex text-ivory px-3 py-2 text-meta2 font-medium disabled:opacity-40"
          >
            <Plus size={14} /> New chat
          </button>
          <div className="mt-3 space-y-1">
            {(threads.data ?? []).map((t) => (
              <button
                key={t.id}
                onClick={() => openThread(t.id)}
                className={`w-full text-left rounded-lex px-3 py-2 transition-colors ${
                  thread?.id === t.id ? 'bg-stone-100 dark:bg-white/10' : 'hover:bg-stone-50 dark:hover:bg-white/5'
                }`}
              >
                <span className="block text-meta2 font-medium text-stone-700 dark:text-stone-200 truncate">{t.title}</span>
                <span className="block text-meta text-stone-400 dark:text-stone-500">{t.message_count} messages</span>
              </button>
            ))}
            {(threads.data ?? []).length === 0 && (
              <p className="text-meta2 text-stone-400 dark:text-stone-500 px-2 py-2">No chats yet — your conversations will be listed here.</p>
            )}
          </div>
        </div>

        {/* Conversation */}
        <div className="flex-1 min-w-0 w-full">
          <div className="space-y-5">
            {(thread?.messages ?? []).map(renderMessage)}
            {!thread && !live && (
              <div className="rounded-lex-lg border border-dashed border-stone-300 dark:border-white/15 p-8 text-center">
                <p className="text-[15px] text-stone-500 dark:text-stone-400">
                  Ask about your documents, send the agent to research, or name a document and the change you want — for example, “Change the notice period in the tenancy note to two months.”
                </p>
              </div>
            )}
            {live && (
              <div className="space-y-3">
                <div className="rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] px-4 py-3">
                  <p className="text-meta2 font-medium text-stone-600 dark:text-stone-300">
                    {live.status || 'Working…'}
                  </p>
                  <div className="mt-2">
                    <LiveEventList events={live.events} />
                  </div>
                </div>
                {live.approvals.map((a) => (
                  <ApprovalCard key={a.edit_id} approval={a} record={editsById.get(a.edit_id)} busy={decide.isPending} onDecide={(id, approve) => decide.mutate({ id, approve })} />
                ))}
                {live.text && (
                  <div className="rounded-lex-lg border border-stone-200 dark:border-white/10 bg-ivory dark:bg-[#14120f] p-5">
                    <div className="prose-lex">
                      <ReactMarkdown remarkPlugins={[remarkGfm]} components={citeMarkdownComponents(live.citations, setModalCitation)}>
                        {linkifyCitations(live.text, live.citations)}
                      </ReactMarkdown>
                    </div>
                  </div>
                )}
                {live.error && (
                  <div className="rounded-lex border border-brand-red/30 bg-[#feeaea] px-4 py-3 text-meta2 text-[#a02c2c]">{live.error}</div>
                )}
              </div>
            )}
            <div ref={endRef} />
          </div>

          {sendError && (
            <div className="mt-4 rounded-lex border border-brand-red/30 bg-[#feeaea] px-4 py-3 text-meta2 text-[#a02c2c]">{sendError}</div>
          )}

          {/* Composer */}
          <div className="mt-5 running-border rounded-lex-lg border border-stone-300 dark:border-white/10 bg-white dark:bg-[#0a1a12]">
            <textarea
              id="agent-chat-input"
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && !e.shiftKey) {
                  e.preventDefault()
                  void send()
                }
              }}
              rows={3}
              placeholder="Tell the agent what you need — a question, research, or a document change…"
              className="w-full resize-none px-4 pt-4 pb-2 text-[15px] outline-none placeholder:text-stone-400 bg-transparent"
            />
            {refDocIds.length > 0 && (
              <div className="flex flex-wrap gap-1.5 px-3 pb-1">
                {refDocIds.map((id) => (
                  <span key={id} className="inline-flex items-center gap-1 rounded-full bg-stone-100 dark:bg-white/10 px-2.5 py-1 text-meta text-stone-600 dark:text-stone-300">
                    {docsInScope.find((d) => d.id === id)?.title ?? id}
                    <button onClick={() => setRefDocIds((r) => r.filter((x) => x !== id))} aria-label="Remove reference">
                      <X size={12} />
                    </button>
                  </span>
                ))}
              </div>
            )}
            <div className="flex flex-wrap items-center justify-between gap-2 px-3 pb-3 pt-1">
              <div className="flex items-center gap-2">
                <select
                  value={matterId}
                  onChange={(e) => setMatterId(e.target.value)}
                  className="rounded-lex border border-stone-300 dark:border-white/10 px-2 py-1.5 text-meta2 bg-white dark:bg-[#0a1a12] outline-none focus:border-ink max-w-[220px]"
                  aria-label="Workspace"
                >
                  <option value="">Whole vault</option>
                  {(matters.data ?? []).map((m) => (
                    <option key={m.id} value={m.id}>{m.title}</option>
                  ))}
                </select>
                <button
                  onClick={() => setAttachOpen((o) => !o)}
                  className="flex items-center gap-1.5 rounded-lex border border-stone-300 dark:border-white/10 px-2.5 py-1.5 text-meta2 text-stone-600 dark:text-stone-300"
                >
                  <Paperclip size={13} /> Reference a document
                </button>
              </div>
              {live ? (
                <button
                  onClick={() => abortRef.current?.abort()}
                  className="flex items-center gap-2 rounded-lex border border-stone-300 dark:border-white/10 px-4 py-2 text-meta2 font-medium text-stone-600 dark:text-stone-300"
                >
                  <Square size={13} /> Stop
                </button>
              ) : (
                <button
                  onClick={() => void send()}
                  disabled={!input.trim()}
                  className="btn-brand flex items-center gap-2 rounded-lex text-ivory px-4 py-2 text-meta2 font-medium disabled:opacity-40"
                >
                  <Send size={14} /> Send
                </button>
              )}
            </div>
            {attachOpen && (
              <div className="border-t border-stone-200 dark:border-white/10 px-4 py-3 max-h-56 overflow-y-auto">
                <p className="text-meta text-stone-400 dark:text-stone-500 mb-2">Referenced documents are named in your message; the agent reads them with read_document, inside the chosen Workspace's scope.</p>
                <div className="space-y-1">
                  {docsInScope.map((d) => (
                    <label key={d.id} className="flex items-center gap-2 text-meta2 text-stone-600 dark:text-stone-300 cursor-pointer">
                      <input
                        type="checkbox"
                        checked={refDocIds.includes(d.id)}
                        onChange={(e) => setRefDocIds((r) => (e.target.checked ? [...r, d.id] : r.filter((x) => x !== d.id)))}
                      />
                      {d.title}
                    </label>
                  ))}
                  {docsInScope.length === 0 && <p className="text-meta2 text-stone-400">No documents in the vault yet.</p>}
                </div>
              </div>
            )}
          </div>
        </div>
      </div>

      {modalCitation && <SourceModal citation={modalCitation} onClose={() => setModalCitation(null)} go={go} />}
    </div>
  )
}
