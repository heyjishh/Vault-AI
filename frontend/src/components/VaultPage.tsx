import { useRef, useState } from 'react'
import { motion } from 'framer-motion'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { FileText, FileUp, Loader2, Scale, ScanText, Trash2, X } from 'lucide-react'
import { mattersApi, vaultApi } from '../api/client'
import type { DocumentInfo, IngestReport } from '../api/types'
import { useSession } from '../stores/session'
import { useMatter } from '../stores/matter'
import { EmptyState, Pager, Spinner, usePagination } from './ui'

export default function VaultPage() {
  const qc = useQueryClient()
  const matterId = useMatter((s) => s.currentMatterId) ?? undefined
  const { data: matterList } = useQuery({ queryKey: ['matters'], queryFn: mattersApi.list })
  const scopeTitle = matterList?.find((m) => m.id === matterId)?.title
  const { data: docs, isLoading } = useQuery({
    queryKey: ['docs', matterId ?? 'all'],
    queryFn: () => (matterId ? vaultApi.listInMatter(matterId) : vaultApi.list()),
  })
  const [title, setTitle] = useState('')
  const [paste, setPaste] = useState('')
  const [prefix, setPrefix] = useState('')
  const [dragging, setDragging] = useState(false)
  const [docQuery, setDocQuery] = useState('')
  const filteredDocs = (docs ?? []).filter((d) => d.title.toLowerCase().includes(docQuery.trim().toLowerCase()))
  const pagedDocs = usePagination(filteredDocs, 8)
  const fileRef = useRef<HTMLInputElement>(null)
  const toast = useSession((s) => s.toast)

  function afterChange(r: IngestReport, what: string) {
    qc.invalidateQueries({ queryKey: ['docs'] })
    qc.invalidateQueries({ queryKey: ['stats'] })
    qc.invalidateQueries({ queryKey: ['graph'] })
    toast(
      'success',
      `${what} indexed — ${r.chunks} chunks, ${r.entities} entities` +
        (r.modal_processed > 0 ? `, ${r.modal_processed} multimodal items` : ''),
    )
  }

  const uploadFile = useMutation({
    mutationFn: (f: File) => vaultApi.uploadFile(title || f.name.replace(/\.[^.]+$/, ''), f, matterId),
    onSuccess: (r) => { afterChange(r, r.documents > 0 ? 'Document' : 'File'); setTitle('') },
    onError: (e) => toast('error', (e as Error).message),
  })
  const uploadText = useMutation({
    mutationFn: () => vaultApi.uploadText(title || 'Pasted document', 'pasted.txt', paste, matterId),
    onSuccess: (r) => { afterChange(r, 'Pasted text'); setPaste(''); setTitle('') },
    onError: (e) => toast('error', (e as Error).message),
  })
  const ingest = useMutation({
    mutationFn: () => vaultApi.ingestS3(prefix || undefined),
    onSuccess: (r) => afterChange(r, 'Bucket ingest'),
    onError: (e) => toast('error', (e as Error).message),
  })
  const [metaDoc, setMetaDoc] = useState<DocumentInfo | null>(null)
  const [indexDocId, setIndexDocId] = useState<string | null>(null)
  const { data: indexChunks, isLoading: indexLoading } = useQuery({
    queryKey: ['doc-chunks', indexDocId],
    queryFn: () => vaultApi.chunks(indexDocId!),
    enabled: !!indexDocId,
  })
  const saveMeta = useMutation({
    mutationFn: (v: { id: string; meta: Parameters<typeof vaultApi.updateMeta>[1] }) =>
      vaultApi.updateMeta(v.id, v.meta),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['docs'] })
      setMetaDoc(null)
      toast('success', 'Validity metadata saved')
    },
    onError: (e) => toast('error', (e as Error).message),
  })
  const remove = useMutation({
    mutationFn: (id: string) => vaultApi.remove(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['docs'] })
      qc.invalidateQueries({ queryKey: ['stats'] })
      toast('info', 'Document removed from the index')
    },
    onError: (e) => toast('error', (e as Error).message),
  })

  function handleDrop(e: React.DragEvent) {
    e.preventDefault()
    setDragging(false)
    const f = e.dataTransfer.files?.[0]
    if (f) uploadFile.mutate(f)
  }

  const busy = uploadFile.isPending || uploadText.isPending || ingest.isPending

  return (
    <div className="max-w-4xl mx-auto px-6 py-10">
      <h1 className="font-display text-[2rem] tracking-tight">Vault</h1>
      <p className="text-stone-500 mt-1.5 text-[15px]">
        Documents are parsed (PDF text layer + OCR fallback), chunked, embedded and added to the knowledge graph.
      </p>
      {matterId && (
        <p className="mt-2 inline-flex rounded-full bg-stone-500/10 text-stone-600 px-3 py-1 text-[12px] font-medium">
          Showing documents in {scopeTitle ?? 'the selected brief'} — uploads file here
        </p>
      )}

      <motion.div
        onDragOver={(e) => { e.preventDefault(); setDragging(true) }}
        onDragLeave={() => setDragging(false)}
        onDrop={handleDrop}
        animate={dragging ? { scale: 1.01 } : { scale: 1 }}
        className={`mt-6 rounded-lex-lg border-2 border-dashed transition-colors p-8 text-center cursor-pointer ${
          dragging ? 'border-ink bg-stone-100' : 'border-stone-300 bg-white/60 hover:border-stone-400'
        }`}
        onClick={() => fileRef.current?.click()}
      >
        {uploadFile.isPending ? (
          <Loader2 size={26} className="animate-spin mx-auto text-ink" />
        ) : (
          <FileUp size={26} className="mx-auto text-stone-400" />
        )}
        <p className="font-medium text-[15px] mt-3">
          {uploadFile.isPending ? 'Indexing your document…' : 'Drop a file here, or click to browse'}
        </p>
        <p className="text-[13px] text-stone-500 mt-1 flex items-center justify-center gap-1.5">
          PDF · TXT · MD <span className="inline-flex items-center gap-1 rounded-full bg-stone-100 border border-stone-200 px-2 py-0.5 text-[11px]"><ScanText size={11} /> scanned pages auto-OCR'd</span>
        </p>
        <input
          ref={fileRef} type="file" className="hidden"
          accept=".pdf,.txt,.md,.doc,.docx"
          onChange={(e) => { const f = e.target.files?.[0]; if (f) uploadFile.mutate(f); e.target.value = '' }}
        />
      </motion.div>

      <div className="grid md:grid-cols-2 gap-4 mt-4">
        <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex p-5">
          <h3 className="font-medium text-[15px]">Paste text</h3>
          <input
            value={title} onChange={(e) => setTitle(e.target.value)}
            placeholder="Title (optional)"
            className="mt-3 w-full rounded-lex border border-stone-300 px-3 py-2 text-[13px] outline-none focus:border-ink placeholder:text-stone-400"
          />
          <textarea
            value={paste} onChange={(e) => setPaste(e.target.value)} rows={5}
            placeholder="Paste statutes, clauses, notes…"
            className="mt-2 w-full rounded-lex border border-stone-300 px-3 py-2 text-[13px] outline-none focus:border-ink placeholder:text-stone-400 resize-none"
          />
          <button
            onClick={() => uploadText.mutate()} disabled={busy || !paste.trim()}
            className="mt-2 rounded-lex border border-stone-300 px-3.5 py-2 text-[13px] font-medium hover:bg-stone-100 disabled:opacity-50"
          >
            {uploadText.isPending ? 'Indexing…' : 'Index pasted text'}
          </button>
        </div>
        <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex p-5">
          <h3 className="font-medium text-[15px]">Ingest from S3 / R2</h3>
          <p className="text-[13px] text-stone-500 mt-2 leading-relaxed">
            Pulls every supported file under a prefix from the bucket configured on the server
            (works with AWS S3 and Cloudflare R2 endpoints).
          </p>
          <input
            value={prefix} onChange={(e) => setPrefix(e.target.value)}
            placeholder="Prefix (e.g. acts/income-tax/)"
            className="mt-3 w-full rounded-lex border border-stone-300 px-3 py-2 text-[13px] outline-none focus:border-ink placeholder:text-stone-400"
          />
          <button
            onClick={() => ingest.mutate()} disabled={busy}
            className="mt-3 rounded-lex bg-ink text-ivory px-3.5 py-2 text-[13px] font-medium hover:bg-ink-900 disabled:opacity-50"
          >
            {ingest.isPending ? 'Ingesting…' : 'Start ingest'}
          </button>
        </div>
      </div>

      <div className="flex items-center justify-between gap-3 mt-10 mb-3 flex-wrap">
        <h2 className="font-display text-xl tracking-tight">Documents</h2>
        {docs && docs.length > 0 && (
          <input
            value={docQuery}
            onChange={(e) => setDocQuery(e.target.value)}
            placeholder="Search documents…"
            className="rounded-full border border-stone-200 bg-white px-3.5 py-1.5 text-[13px] outline-none focus:border-stone-400 w-56"
          />
        )}
      </div>
      {isLoading ? (
        <Spinner label="Loading vault…" />
      ) : !docs || docs.length === 0 ? (
        <EmptyState title="No documents yet" hint="Drop a PDF above or ingest from S3 to build your corpus." />
      ) : filteredDocs.length === 0 ? (
        <EmptyState title="No documents match" hint="Try a different search term." />
      ) : (
        <>
        <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex overflow-hidden">
          {pagedDocs.pageItems.map((d, i) => (
            <div key={d.id} className={i > 0 ? 'border-t border-stone-200' : ''}>
            <div className="flex items-center gap-4 px-5 py-3.5">
              <FileText size={18} className="text-stone-400 flex-shrink-0" />
              <div className="min-w-0 flex-1">
                <p className="text-[15px] font-medium truncate">{d.title}</p>
                <p className="text-[12px] text-stone-500">
                  {d.chunks} chunks · {d.chars.toLocaleString()} chars · {new Date(d.ingested_at).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' })}
                </p>
                {(d.meta?.jurisdiction || d.meta?.effective_from || d.meta?.effective_to || d.meta?.superseded_by) && (
                  <p className="text-[12px] text-stone-500 mt-0.5 flex items-center gap-1.5 flex-wrap">
                    <Scale size={11} className="text-stone-400" />
                    {d.meta?.jurisdiction && <span>{d.meta.jurisdiction}</span>}
                    {(d.meta?.effective_from || d.meta?.effective_to) && (
                      <span>in force {d.meta?.effective_from ?? '…'} → {d.meta?.effective_to ?? 'present'}</span>
                    )}
                    {d.meta?.superseded_by && (
                      <span className="rounded-full bg-amber-50 border border-amber-600/30 text-amber-800 px-1.5 py-px text-[10px] font-semibold uppercase tracking-wide">
                        Superseded by {d.meta.superseded_by}
                      </span>
                    )}
                  </p>
                )}
              </div>
              <button
                onClick={() => setIndexDocId(indexDocId === d.id ? null : d.id)}
                title="Indexing — keywords, generated questions and statute hyperedges per chunk"
                className={`p-2 rounded-lex transition-colors ${indexDocId === d.id ? 'text-ink bg-stone-100' : 'text-stone-400 hover:text-ink hover:bg-stone-100'}`}
              >
                <ScanText size={15} />
              </button>
              <button
                onClick={() => setMetaDoc(d)}
                title="Edit validity — jurisdiction, in-force dates, superseded by"
                className="p-2 rounded-lex text-stone-400 hover:text-ink hover:bg-stone-100 transition-colors"
              >
                <Scale size={15} />
              </button>
              <button
                onClick={() => { if (confirm(`Remove "${d.title}" from the index?`)) remove.mutate(d.id) }}
                title="Remove from index"
                className="p-2 rounded-lex text-stone-400 hover:text-[#a02c2c] hover:bg-[#feeaea] transition-colors"
              >
                <Trash2 size={15} />
              </button>
            </div>
            {indexDocId === d.id && (
              <div className="border-t border-stone-100 bg-[#fcfbf9] px-5 py-4">
                <p className="text-[11.5px] font-medium uppercase tracking-wide text-stone-400">
                  Indexing — generated at ingest (augmentation + statute hyperedges)
                </p>
                {indexLoading ? (
                  <p className="text-[13px] text-stone-400 mt-2">Loading chunks…</p>
                ) : (indexChunks ?? []).length === 0 ? (
                  <p className="text-[13px] text-stone-400 mt-2">No chunks found for this document.</p>
                ) : (
                  <div className="mt-3 space-y-3">
                    {(indexChunks ?? []).map((c) => (
                      <div key={c.id} className="rounded-lex border border-stone-200 bg-white px-4 py-3">
                        <p className="text-[13px] text-stone-600 leading-relaxed">{c.text_preview}…</p>
                        {c.hyper_id && (
                          <span className="mt-2 inline-block rounded-full bg-[#eef3fb] border border-[#c9d8f2] text-[#31558f] px-2 py-px text-[11px] font-medium">
                            Hyperedge: {c.hyper_id.split('#').pop()}
                          </span>
                        )}
                        {c.aug_keywords && (
                          <p className="mt-2 text-[12px] text-stone-500">
                            <span className="font-medium text-stone-600">Keywords:</span> {c.aug_keywords}
                          </p>
                        )}
                        {c.aug_questions && (
                          <div className="mt-1.5 flex flex-wrap gap-1.5">
                            {c.aug_questions.split(' ;; ').map((q, qi) => (
                              <span key={qi} className="inline-block rounded-full border border-stone-200 bg-[#fcfbf9] px-2.5 py-1 text-[12px] text-stone-600">
                                {q}
                              </span>
                            ))}
                          </div>
                        )}
                        {!c.aug_keywords && !c.aug_questions && (
                          <p className="mt-2 text-[12px] text-stone-400">No augmentation — indexed before augmentation shipped, or it was disabled at ingest.</p>
                        )}
                      </div>
                    ))}
                  </div>
                )}
              </div>
            )}
            </div>
          ))}
        </div>
        <Pager page={pagedDocs.page} pages={pagedDocs.pages} from={pagedDocs.from} to={pagedDocs.to} total={pagedDocs.total} onPage={pagedDocs.setPage} />
        </>
      )}
      {metaDoc && (
        <MetaEditor
          doc={metaDoc}
          saving={saveMeta.isPending}
          onClose={() => setMetaDoc(null)}
          onSave={(meta) => saveMeta.mutate({ id: metaDoc.id, meta })}
        />
      )}
    </div>
  )
}

function MetaEditor(props: {
  doc: DocumentInfo
  saving: boolean
  onClose: () => void
  onSave: (meta: { jurisdiction?: string; effective_from?: string; effective_to?: string; superseded_by?: string }) => void
}) {
  const { doc, saving, onClose, onSave } = props
  const [jurisdiction, setJurisdiction] = useState(doc.meta?.jurisdiction ?? '')
  const [from, setFrom] = useState(doc.meta?.effective_from ?? '')
  const [to, setTo] = useState(doc.meta?.effective_to ?? '')
  const [supersededBy, setSupersededBy] = useState(doc.meta?.superseded_by ?? '')
  const field = 'w-full rounded-lex border border-stone-200 px-2.5 py-1.5 text-[13px] outline-none focus:border-stone-400 placeholder:text-stone-400 bg-white'
  const label = 'block text-[11px] font-medium uppercase tracking-wide text-stone-500 mb-1'
  return (
    <div className="fixed inset-0 z-40 flex items-center justify-center bg-ink/30 p-4" onClick={onClose}>
      <div
        className="w-full max-w-md rounded-lex-lg border border-stone-200 bg-[#fafaf9] shadow-lex-md p-5"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-start justify-between">
          <div>
            <p className="text-[15px] font-semibold text-ink">Validity — {doc.title}</p>
            <p className="text-[12.5px] text-stone-500 mt-0.5">
              Ask cites a document only when it is in force on the requested date and matches the jurisdiction.
              Leave everything empty to clear.
            </p>
          </div>
          <button onClick={onClose} className="p-1.5 rounded-lex text-stone-400 hover:text-ink" title="Close">
            <X size={16} />
          </button>
        </div>
        <div className="mt-4 space-y-3">
          <div>
            <span className={label}>Jurisdiction</span>
            <input value={jurisdiction} onChange={(e) => setJurisdiction(e.target.value)} placeholder="e.g. IN, IN-DL, US-CA" className={field} />
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div>
              <span className={label}>In force from</span>
              <input type="date" value={from} onChange={(e) => setFrom(e.target.value)} className={field} />
            </div>
            <div>
              <span className={label}>No longer in force from</span>
              <input type="date" value={to} onChange={(e) => setTo(e.target.value)} className={field} />
            </div>
          </div>
          <div>
            <span className={label}>Superseded by</span>
            <input value={supersededBy} onChange={(e) => setSupersededBy(e.target.value)} placeholder="e.g. the Finance Act 2023" className={field} />
          </div>
        </div>
        <div className="mt-5 flex justify-end gap-2">
          <button onClick={onClose} className="rounded-full border border-stone-300 px-3.5 py-1.5 text-[13px] text-stone-600 hover:text-ink">
            Cancel
          </button>
          <button
            disabled={saving}
            onClick={() => onSave({
              jurisdiction: jurisdiction.trim() || undefined,
              effective_from: from || undefined,
              effective_to: to || undefined,
              superseded_by: supersededBy.trim() || undefined,
            })}
            className="rounded-full bg-ink text-ivory px-4 py-1.5 text-[13px] font-medium disabled:opacity-50"
          >
            {saving ? 'Saving…' : 'Save validity'}
          </button>
        </div>
      </div>
    </div>
  )
}
