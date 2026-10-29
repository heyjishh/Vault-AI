import { useEffect, useRef, useState } from 'react'
import { useMutation, useQuery } from '@tanstack/react-query'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { CalendarClock, FileSearch, ListTree, ScanSearch, ShieldAlert } from 'lucide-react'
import { createApi, vaultApi, type InteractResponse } from '../api/client'
import { useSession } from '../stores/session'
import { EmptyState, Spinner } from './ui'

const ACTIONS = [
  { id: 'summary', label: 'Summary', icon: FileSearch, hint: 'Parties, key terms, dates, outcomes' },
  { id: 'chronology', label: 'Chronology', icon: CalendarClock, hint: 'Dated timeline of events' },
  { id: 'clauses', label: 'Key clauses', icon: ListTree, hint: 'Operative provisions & who they favour' },
  { id: 'risks', label: 'Risk review', icon: ShieldAlert, hint: 'Onerous terms, gaps, deadlines' },
  { id: 'chat', label: 'Ask the documents', icon: ScanSearch, hint: 'Free-form question across the selection' },
] as const

/** Mirrors the server cap (MAX_INTERACT_DOCS in routes/create.rs):
 *  the analysis budget is split across the selected documents. */
export const MAX_INTERACT_DOCS = 5

function treatmentChip(status: string) {
  if (status === 'overruled') return 'bg-rose-100 text-rose-800'
  if (status === 'good_law' || status === 'followed' || status === 'applied') return 'bg-emerald-100 text-emerald-800'
  return 'bg-amber-100 text-amber-800'
}

export default function InteractPage({ matterId, allowScopeToggle }: { matterId?: string; allowScopeToggle?: boolean } = {}) {
  const toast = useSession((s) => s.toast)
  // History restore: reopen the exact analysis on its document set.
  const restore = useSession((s) => s.restore)
  const consumeRestore = useSession((s) => s.consumeRestore)
  const [selectedIds, setSelectedIds] = useState<string[]>([])
  useEffect(() => {
    if (!restore || restore.kind !== 'interact') return
    const p = consumeRestore('interact')
    if (!p) return
    const ids = Array.isArray(p.document_ids) && p.document_ids.length > 0
      ? p.document_ids.map(String)
      : [String(p.document_id ?? '')].filter(Boolean)
    setSelectedIds(ids)
    setResult({
      document_id: String(p.document_id ?? ''),
      document_title: String(p.document_title ?? ''),
      action: String(p.action ?? 'summary'),
      output: String(p.output ?? ''),
      truncated: Boolean(p.truncated),
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [restore])

  // Brief scoping (standalone view only): with an active brief the
  // picker defaults to the brief's documents; the toggle widens it to
  // the full vault. Switching scope keeps only selected documents
  // that are present in the newly shown list.
  const canToggle = Boolean(allowScopeToggle && matterId)
  const [scope, setScope] = useState<'brief' | 'vault'>('brief')
  const effectiveMatterId = canToggle
    ? (scope === 'brief' ? matterId : undefined)
    : matterId
  const docs = useQuery({
    queryKey: ['vault-docs', effectiveMatterId ?? 'all'],
    queryFn: () => (effectiveMatterId ? vaultApi.listInMatter(effectiveMatterId) : vaultApi.list()),
  })
  const pruneOnNextData = useRef(false)
  useEffect(() => {
    if (!pruneOnNextData.current || !docs.data) return
    pruneOnNextData.current = false
    const visible = new Set(docs.data.map((d) => d.id))
    setSelectedIds((prev) => prev.filter((id) => visible.has(id)))
  }, [docs.data])
  const switchScope = (next: 'brief' | 'vault') => {
    if (next === scope) return
    pruneOnNextData.current = true
    setScope(next)
  }

  useEffect(() => {
    // Arriving from document search? Select that document directly;
    // widen to the full vault so it is visible in the picker.
    const fromSearch = sessionStorage.getItem('jurisvault.interactDoc')
    if (!fromSearch) return
    sessionStorage.removeItem('jurisvault.interactDoc')
    setSelectedIds([fromSearch])
    if (canToggle) setScope('vault')
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
  const [question, setQuestion] = useState('')
  const [result, setResult] = useState<InteractResponse | null>(null)

  const run = useMutation({
    mutationFn: (action: string) => createApi.interact(selectedIds, action, action === 'chat' ? question : undefined),
    onSuccess: (r) => setResult(r),
    onError: (e) => toast('error', (e as Error).message),
  })

  const toggleDoc = (id: string) => {
    setResult(null)
    setSelectedIds((prev) => {
      if (prev.includes(id)) return prev.filter((x) => x !== id)
      if (prev.length >= MAX_INTERACT_DOCS) {
        toast('error', `Interact covers at most ${MAX_INTERACT_DOCS} documents at once — the analysis budget is split across the selection.`)
        return prev
      }
      return [...prev, id]
    })
  }

  const hasSelection = selectedIds.length > 0

  return (
    <div className="max-w-5xl mx-auto px-6 py-8">
      <h1 className="font-display text-[2rem] tracking-tight">Interact</h1>
      <p className="text-stone-500 mt-1.5 text-[15px]">
        Work one document or a set of up to {MAX_INTERACT_DOCS}: summarise them, pull their chronology, map their clauses, or interrogate them together.
      </p>

      <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-6 py-5 mt-6">
        <div className="flex items-baseline justify-between gap-3 flex-wrap">
          <span className="text-meta uppercase tracking-wider text-stone-500 font-medium">
            Documents — {selectedIds.length} selected (max {MAX_INTERACT_DOCS})
          </span>
          {canToggle && (
            <div className="flex items-center gap-1.5">
              <button
                onClick={() => switchScope('brief')}
                className={`rounded-lex px-2.5 py-1 text-[12.5px] border ${scope === 'brief' ? 'bg-ink text-white border-ink' : 'border-stone-300 text-stone-600'}`}
              >
                This brief
              </button>
              <button
                onClick={() => switchScope('vault')}
                className={`rounded-lex px-2.5 py-1 text-[12.5px] border ${scope === 'vault' ? 'bg-ink text-white border-ink' : 'border-stone-300 text-stone-600'}`}
              >
                Full vault
              </button>
            </div>
          )}
        </div>
        {canToggle && (
          <p className="text-[12px] text-stone-400 mt-1.5">
            {scope === 'brief'
              ? 'Showing this brief’s filed documents. Switching scope keeps only selected documents that are in the new list.'
              : 'Showing the whole vault. Switching scope keeps only selected documents that are in the new list.'}
          </p>
        )}
        <div className="mt-2.5 max-h-64 overflow-y-auto rounded-lex border border-stone-200 divide-y divide-stone-100">
          {(docs.data ?? []).map((d) => (
            <label key={d.id} className="flex items-center gap-3 px-3 py-2.5 cursor-pointer hover:bg-stone-50">
              <input
                type="checkbox"
                checked={selectedIds.includes(d.id)}
                onChange={() => toggleDoc(d.id)}
                className="accent-stone-800"
              />
              <span className="text-[14px] flex-1">{d.title}</span>
              <span className="text-[12px] text-stone-400">{d.chunks} chunks</span>
            </label>
          ))}
          {docs.isPending && <p className="px-3 py-2.5 text-[13.5px] text-stone-400">Loading documents…</p>}
          {docs.data && docs.data.length === 0 && (
            <p className="px-3 py-2.5 text-[13.5px] text-stone-500">
              {canToggle && scope === 'brief'
                ? 'No documents are filed in this brief yet — switch to Full vault, or file documents from the brief page.'
                : 'The vault is empty — upload a document first, then come back here to work it.'}
            </p>
          )}
        </div>

        {hasSelection && (
          <>
            <div className="grid sm:grid-cols-2 lg:grid-cols-5 gap-2.5 mt-5">
              {ACTIONS.map((a) => (
                <button
                  key={a.id}
                  disabled={run.isPending || (a.id === 'chat' && !question.trim())}
                  onClick={() => run.mutate(a.id)}
                  className="rounded-lex border border-stone-200 px-3.5 py-3 text-left hover:border-ink disabled:opacity-40 transition-colors"
                >
                  <a.icon size={16} className="text-stone-500" />
                  <p className="text-[13.5px] font-medium mt-1.5">{a.label}</p>
                  <p className="text-[11.5px] text-stone-400 leading-snug">{a.hint}</p>
                </button>
              ))}
            </div>
            <input
              value={question}
              onChange={(e) => setQuestion(e.target.value)}
              placeholder="For “Ask the documents”: e.g. What is the notice period, and who can terminate?"
              className="mt-3 w-full rounded-lex border border-stone-300 px-3 py-2.5 text-[14px]"
            />
          </>
        )}
      </div>

      {run.isPending && <Spinner label="Reading the document…" />}
      {!run.isPending && result && (
        <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-6 py-5 mt-5">
          <div className="flex items-baseline justify-between gap-3 flex-wrap">
            <h2 className="font-display text-xl">
              {ACTIONS.find((a) => a.id === result.action)?.label} — {result.document_title}
              {result.documents && result.documents.length > 1 && (
                <span className="text-stone-400 text-base"> +{result.documents.length - 1} more</span>
              )}
            </h2>
            {result.truncated && (
              <span className="text-meta text-stone-400">Long document set — analysed within the shared budget</span>
            )}
          </div>
          {result.documents && result.documents.length > 0 && (
            <div className="flex gap-1.5 flex-wrap mt-2.5">
              {result.documents.map((d) => (
                <span key={d.document_id} className="inline-flex items-center gap-1.5 rounded-full bg-stone-100 px-2.5 py-1 text-[12px] text-stone-600">
                  {d.document_title}
                  {d.truncated && <span className="text-stone-400">(partial)</span>}
                  {d.treatment && d.treatment.status !== 'unknown' && (
                    <span className={`rounded-full px-1.5 py-0.5 text-[11px] font-medium ${treatmentChip(d.treatment.status)}`} title={d.treatment.evidence_passage ?? undefined}>
                      {d.treatment.status.replace('_', ' ')}
                    </span>
                  )}
                </span>
              ))}
            </div>
          )}
          <div className="prose-lex mt-3 text-[14.5px] leading-relaxed">
            <ReactMarkdown remarkPlugins={[remarkGfm]}>{result.output}</ReactMarkdown>
          </div>
        </div>
      )}
      {!run.isPending && !result && !hasSelection && (
        <div className="mt-5"><EmptyState title="Pick a document to begin" hint="Interact reads the documents themselves — not the whole corpus." /></div>
      )}
    </div>
  )
}
