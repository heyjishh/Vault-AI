import { useEffect, useState } from 'react'
import { createPortal } from 'react-dom'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { Copy, Loader2, Sparkles, X } from 'lucide-react'
import { createApi, ragApi } from '../api/client'
import type { Citation } from '../api/types'
import { useSession } from '../stores/session'
import { ResponseDownload } from './ResponseDownload'

/**
 * Piece 31: an Insight button on document sources. One click summarises
 * the SOURCE DOCUMENT itself (not the answer) — the Interact analyst
 * runs over the document's full text. The modal opens on Summary and
 * offers the other Interact views of the same document as tabs:
 * Key clauses, Risks, Chronology.
 *
 * Scope is deliberate: vault documents only. Web sources have no vault
 * document to analyse, and Confidential Vault text is never fetched
 * into this view (Private citations get no Insight button).
 *
 * Results are cached per document + view for the page session, so
 * reopening an Insight — or opening it for the same document from a
 * second citation — is instant and does not run (or bill) the model
 * again.
 */

const VIEWS = [
  { action: 'summary', label: 'Summary', busy: 'Summarising the full document…' },
  { action: 'clauses', label: 'Key clauses', busy: 'Extracting the key clauses…' },
  { action: 'risks', label: 'Risks', busy: 'Reviewing the document for risks…' },
  { action: 'chronology', label: 'Chronology', busy: 'Building the chronology…' },
] as const

type InsightAction = (typeof VIEWS)[number]['action']

const cache = new Map<string, string>()
const cacheKey = (documentId: string, action: InsightAction) => `${documentId}:${action}`

/** The Insight modal: one document, four analyst views of it. */
export function DocInsightModal({ documentId, title, onClose }: {
  documentId: string
  title: string
  onClose: () => void
}) {
  const toast = useSession((s) => s.toast)
  const [action, setAction] = useState<InsightAction>('summary')
  const [output, setOutput] = useState<string | null>(() => cache.get(cacheKey(documentId, 'summary')) ?? null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [nonce, setNonce] = useState(0)

  useEffect(() => {
    const h = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [onClose])

  useEffect(() => {
    const cached = cache.get(cacheKey(documentId, action))
    if (cached !== undefined) {
      setOutput(cached)
      setError(null)
      setBusy(false)
      return
    }
    let live = true
    setBusy(true)
    setError(null)
    setOutput(null)
    createApi
      .interact([documentId], action)
      .then((r) => {
        if (!live) return
        cache.set(cacheKey(documentId, action), r.output)
        setOutput(r.output)
        setBusy(false)
      })
      .catch((e) => {
        if (!live) return
        setError(e instanceof Error ? e.message : 'Could not generate this insight')
        setBusy(false)
      })
    return () => { live = false }
  }, [documentId, action, nonce])

  const view = VIEWS.find((v) => v.action === action) ?? VIEWS[0]

  return createPortal(
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-ink/45 p-4"
      onClick={(e) => { e.stopPropagation(); onClose() }}
    >
      <div
        className="flex max-h-[86vh] w-full max-w-3xl flex-col rounded-lex-lg bg-white dark:bg-[#1d1f19] shadow-lex-md"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-start gap-3 border-b border-stone-200 dark:border-white/10 px-6 py-4">
          <span className="mt-0.5 inline-flex items-center gap-1 rounded bg-ink px-1.5 py-0.5 text-[11px] font-semibold text-ivory">
            <Sparkles size={11} /> Insight
          </span>
          <div className="min-w-0 flex-1">
            <p className="truncate text-[15px] font-semibold leading-snug">{title}</p>
            <p className="mt-0.5 text-[12.5px] text-stone-500 dark:text-stone-400">
              AI insight from the document's full text — generated once, then kept for this session.
            </p>
          </div>
          <button
            onClick={onClose}
            className="rounded-lex p-1.5 text-stone-500 dark:text-stone-400 transition-colors hover:bg-stone-100 dark:hover:bg-white/10 hover:text-ink"
            title="Close"
          >
            <X size={17} />
          </button>
        </div>

        <div className="flex items-center gap-1.5 border-b border-stone-100 dark:border-white/10 px-6 py-2.5">
          {VIEWS.map((v) => (
            <button
              key={v.action}
              onClick={() => setAction(v.action)}
              className={`rounded-lex px-3 py-1.5 text-[12.5px] font-medium transition-colors ${
                action === v.action
                  ? 'bg-ink text-ivory'
                  : 'border border-stone-200 dark:border-white/10 bg-white dark:bg-[#1d1f19] text-stone-600 dark:text-stone-300 hover:text-ink hover:border-stone-300'
              }`}
            >
              {v.label}
            </button>
          ))}
        </div>

        <div className="overflow-y-auto px-6 py-5">
          {busy ? (
            <div className="flex items-center gap-2.5 py-8 text-[13.5px] text-stone-500 dark:text-stone-400">
              <Loader2 size={16} className="animate-spin" />
              {view.busy} Long documents can take a few seconds.
            </div>
          ) : error ? (
            <div>
              <p className="text-[13.5px] text-stone-600 dark:text-stone-300">Couldn’t generate this insight: {error}</p>
              <button
                onClick={() => setNonce((n) => n + 1)}
                className="mt-3 rounded-lex bg-ink px-3.5 py-2 text-[12.5px] font-medium text-ivory hover:bg-ink-900"
              >
                Try again
              </button>
            </div>
          ) : output ? (
            <div className="prose-lex text-[14.5px] leading-relaxed">
              <ReactMarkdown remarkPlugins={[remarkGfm]}>{output}</ReactMarkdown>
            </div>
          ) : null}
        </div>

        <div className="flex items-center justify-between gap-3 border-t border-stone-200 dark:border-white/10 px-6 py-3">
          <span className="text-[12px] text-stone-500 dark:text-stone-400">
            Check against the original document before relying on this for a filing.
          </span>
          {output && (
            <span className="inline-flex items-center gap-2">
              <button
                onClick={() => {
                  navigator.clipboard.writeText(output).catch(() => {})
                  toast('success', 'Insight copied')
                }}
                className="flex items-center gap-1.5 rounded-lex border border-stone-200 dark:border-white/10 bg-white dark:bg-[#1d1f19] px-2.5 py-1.5 text-[12px] font-medium text-stone-600 dark:text-stone-300 hover:text-ink hover:border-stone-300 transition-colors"
              >
                <Copy size={13} /> Copy
              </button>
              <ResponseDownload
                question={`Document insight — ${view.label}: ${title}`}
                title={title}
                answerMarkdown={output}
                citations={[]}
              />
            </span>
          )}
        </div>
      </div>
    </div>,
    document.body,
  )
}

/**
 * The button itself. Give it a citation (it resolves the citation's
 * chunk to the real document id on click) or a documentId + title
 * directly when the host already resolved them (the source viewer).
 * Renders nothing for web / private citations — see the scope note
 * at the top of this file.
 */
export function DocInsightButton({ citation, documentId, title, className, label }: {
  citation?: Citation
  documentId?: string
  title?: string
  className?: string
  label?: string
}) {
  const toast = useSession((s) => s.toast)
  const [open, setOpen] = useState(false)
  const [resolving, setResolving] = useState(false)
  const [resolved, setResolved] = useState<{ id: string; title: string } | null>(
    documentId ? { id: documentId, title: title ?? documentId } : null,
  )

  // A host that resolves the document id after mount (the source
  // viewer fetches it) hands it over as it arrives.
  useEffect(() => {
    if (documentId) setResolved({ id: documentId, title: title ?? documentId })
  }, [documentId, title])

  if (citation && (!citation.doc_id.startsWith('doc:') || citation.source_url)) return null

  const onClick = async (e: React.MouseEvent) => {
    e.stopPropagation()
    if (resolved) {
      setOpen(true)
      return
    }
    if (!citation) return
    setResolving(true)
    try {
      const doc = await ragApi.sourceDocument({ chunk_id: citation.chunk_id })
      if (!doc.document_id) throw new Error('unresolved')
      setResolved({ id: doc.document_id, title: doc.title ?? citation.doc_title ?? doc.document_id })
      setOpen(true)
    } catch {
      toast('error', 'Could not resolve this document for an insight')
    } finally {
      setResolving(false)
    }
  }

  return (
    <>
      <button
        onClick={(e) => { void onClick(e) }}
        disabled={resolving}
        title="Summarise this document"
        className={className ?? 'flex items-center gap-1.5 text-[12.5px] font-medium text-ink dark:text-[#f4f2ee] hover:underline underline-offset-2 disabled:opacity-50'}
      >
        {resolving ? <Loader2 size={12} className="animate-spin" /> : <Sparkles size={12} />}
        {label ?? 'Insight — summarise this document'}
      </button>
      {open && resolved && (
        <DocInsightModal
          documentId={resolved.id}
          title={resolved.title}
          onClose={() => setOpen(false)}
        />
      )}
    </>
  )
}
