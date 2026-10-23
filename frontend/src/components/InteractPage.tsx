import { useEffect, useState } from 'react'
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
  { id: 'chat', label: 'Ask the document', icon: ScanSearch, hint: 'Free-form question on this document' },
] as const

export default function InteractPage({ matterId }: { matterId?: string } = {}) {
  const toast = useSession((s) => s.toast)
  // History restore: reopen the exact analysis on its document.
  const restore = useSession((s) => s.restore)
  const consumeRestore = useSession((s) => s.consumeRestore)
  useEffect(() => {
    if (!restore || restore.kind !== 'interact') return
    const p = consumeRestore('interact')
    if (!p) return
    setDocId(String(p.document_id ?? ''))
    setResult({
      document_id: String(p.document_id ?? ''),
      document_title: String(p.document_title ?? ''),
      action: String(p.action ?? 'summary'),
      output: String(p.output ?? ''),
      truncated: Boolean(p.truncated),
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [restore])
  const docs = useQuery({
    queryKey: ['vault-docs', matterId ?? 'all'],
    queryFn: () => (matterId ? vaultApi.listInMatter(matterId) : vaultApi.list()),
  })
  const [docId, setDocId] = useState<string>(() => {
    // Arriving from document search? Open that document directly.
    const fromSearch = sessionStorage.getItem('jurisvault.interactDoc')
    if (fromSearch) sessionStorage.removeItem('jurisvault.interactDoc')
    return fromSearch ?? ''
  })
  const [question, setQuestion] = useState('')
  const [result, setResult] = useState<InteractResponse | null>(null)

  const run = useMutation({
    mutationFn: (action: string) => createApi.interact(docId, action, action === 'chat' ? question : undefined),
    onSuccess: (r) => setResult(r),
    onError: (e) => toast('error', (e as Error).message),
  })

  const selected = docs.data?.find((d) => d.id === docId)

  return (
    <div className="max-w-5xl mx-auto px-6 py-8">
      <h1 className="font-display text-[2rem] tracking-tight">Interact</h1>
      <p className="text-stone-500 mt-1.5 text-[15px]">
        Work a single document: summarise it, pull its chronology, map its clauses, or interrogate it directly.
      </p>

      <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-6 py-5 mt-6">
        <label className="block">
          <span className="text-meta uppercase tracking-wider text-stone-500 font-medium">Document</span>
          <select
            value={docId}
            onChange={(e) => { setDocId(e.target.value); setResult(null) }}
            className="mt-1.5 w-full rounded-lex border border-stone-300 px-3 py-2.5 text-[14px] bg-white"
          >
            <option value="">Choose a document from the vault…</option>
            {(docs.data ?? []).map((d) => (
              <option key={d.id} value={d.id}>{d.title} ({d.chunks} chunks)</option>
            ))}
          </select>
        </label>

        {selected && (
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
              placeholder="For “Ask the document”: e.g. What is the notice period, and who can terminate?"
              className="mt-3 w-full rounded-lex border border-stone-300 px-3 py-2.5 text-[14px]"
            />
          </>
        )}
        {docs.data && docs.data.length === 0 && (
          <p className="text-[13.5px] text-stone-500 mt-3">The vault is empty — upload a document first, then come back here to work it.</p>
        )}
      </div>

      {run.isPending && <Spinner label="Reading the document…" />}
      {!run.isPending && result && (
        <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-6 py-5 mt-5">
          <div className="flex items-baseline justify-between gap-3 flex-wrap">
            <h2 className="font-display text-xl">
              {ACTIONS.find((a) => a.id === result.action)?.label} — {result.document_title}
            </h2>
            {result.truncated && (
              <span className="text-meta text-stone-400">Long document — analysed the first section in full</span>
            )}
          </div>
          <div className="prose-lex mt-3 text-[14.5px] leading-relaxed">
            <ReactMarkdown remarkPlugins={[remarkGfm]}>{result.output}</ReactMarkdown>
          </div>
        </div>
      )}
      {!run.isPending && !result && !selected && (
        <div className="mt-5"><EmptyState title="Pick a document to begin" hint="Interact reads the document itself — not the whole corpus." /></div>
      )}
    </div>
  )
}
