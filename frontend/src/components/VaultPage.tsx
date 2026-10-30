import { useRef, useState } from 'react'
import { motion } from 'framer-motion'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ExternalLink, FileText, FileUp, FolderUp, Loader2, Scale, ScanText, Trash2, X } from 'lucide-react'
import { mattersApi, openOriginalDocument, vaultApi } from '../api/client'
import type { DocumentInfo, IngestReport } from '../api/types'
import { useSession } from '../stores/session'
import { useMatter } from '../stores/matter'
import { EmptyState, Pager, Spinner, usePagination } from './ui'
import {
  UploadProgressList, formatElapsed, isPendingStatus, useNow,
  applyBatchCaps, batchLabelFor, filesFromDataTransfer, UPLOAD_ACCEPT,
  UPLOAD_FORMATS_HINT, MAX_BATCH_FILES, MAX_BATCH_FOLDERS,
  type ProgressItemView,
} from './UploadProgress'

export default function VaultPage() {
  const qc = useQueryClient()
  const storeMatterId = useMatter((s) => s.currentMatterId)
  const { data: matterList } = useQuery({ queryKey: ['matters'], queryFn: mattersApi.list })
  // Brief-only vault: the Vault shows ONLY the active brief's
  // documents — there is no full-vault view. The active brief is
  // the stored pick when it still exists, otherwise the default
  // brief (the fallback AppShell also resolves into the store).
  const matterId =
    (storeMatterId && matterList?.some((m) => m.id === storeMatterId)
      ? storeMatterId
      : undefined) ??
    matterList?.find((m) => m.is_default)?.id ??
    matterList?.[0]?.id ??
    storeMatterId ??
    undefined
  const scopeTitle = matterList?.find((m) => m.id === matterId)?.title
  const { data: docs, isPending } = useQuery({
    queryKey: ['docs', matterId ?? 'pending'],
    queryFn: () => vaultApi.listInMatter(matterId!),
    enabled: !!matterId,
  })
  const isLoading = !matterId || isPending
  const [title, setTitle] = useState('')
  const [paste, setPaste] = useState('')
  const [prefix, setPrefix] = useState('')
  const [dragging, setDragging] = useState(false)
  const [docQuery, setDocQuery] = useState('')
  const filteredDocs = (docs ?? []).filter((d) => d.title.toLowerCase().includes(docQuery.trim().toLowerCase()))
  const pagedDocs = usePagination(filteredDocs, 8)
  const fileRef = useRef<HTMLInputElement>(null)
  const folderRef = useRef<HTMLInputElement>(null)
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

  // File uploads (Piece 18): a per-file staged queue — the picker
  // takes multiple files and a drop takes every dropped file (and
  // traverses dropped folders), all under the shared batch caps.
  // Piece 19 adds the missing folder PICKER (webkitdirectory)
  // beside the dropzone, feeding the same queue via enqueuePicked,
  // rendered by the shared UploadProgressList. This replaces the
  // single-upload mutation and its Piece 17 failure row: every file
  // gets its own row with stage, byte progress ('uploading' tracks
  // real bytes sent; once sent, 'indexing' until the server's ingest
  // response lands — Ready = indexed, confirmed by the response),
  // elapsed, and inline Retry/Dismiss on failure. Items process
  // sequentially (ingest is LLM-backed); one failure never blocks
  // the rest.
  interface FileItem extends ProgressItemView { file: File; fileTitle: string }
  const [fileItems, setFileItemsState] = useState<FileItem[]>([])
  const fileItemsRef = useRef<FileItem[]>([])
  const fileKeyRef = useRef(0)
  const fileProcessingRef = useRef(false)
  const [fileBatchLabel, setFileBatchLabel] = useState<string | null>(null)
  const matterRef = useRef(matterId)
  matterRef.current = matterId
  const setFileItems = (next: FileItem[]) => {
    fileItemsRef.current = next
    setFileItemsState(next)
  }
  const patchFileItem = (key: number, patch: Partial<FileItem>) =>
    setFileItems(fileItemsRef.current.map((i) => (i.key === key ? { ...i, ...patch } : i)))
  const processFileQueue = async () => {
    if (fileProcessingRef.current) return
    fileProcessingRef.current = true
    try {
      for (;;) {
        const item = fileItemsRef.current.find((i) => i.status === 'queued')
        if (!item) break
        patchFileItem(item.key, { status: 'uploading', progress: 0, startedAt: Date.now() })
        try {
          const r = await vaultApi.uploadFile(item.fileTitle, item.file, matterRef.current, (fraction) => {
            if (fraction !== null && fraction >= 1) {
              patchFileItem(item.key, { status: 'indexing', progress: 1 })
            } else {
              patchFileItem(item.key, { status: 'uploading', progress: fraction })
            }
          })
          patchFileItem(item.key, { status: 'ready', progress: 1, note: `${r.chunks} chunks indexed` })
          afterChange(r, 'Document')
        } catch (e) {
          patchFileItem(item.key, { status: 'failed', error: (e as Error).message })
          toast('error', (e as Error).message)
        }
      }
    } finally {
      fileProcessingRef.current = false
    }
  }
  const enqueueFiles = (files: { file: File; relPath: string }[], titleOverride?: string) => {
    if (files.length === 0) return
    const items: FileItem[] = applyBatchCaps(files).map(({ file, relPath, skipReason }) => ({
      key: ++fileKeyRef.current,
      name: relPath || file.name,
      status: skipReason ? ('skipped' as const) : ('queued' as const),
      error: skipReason,
      file,
      fileTitle: titleOverride ?? file.name.replace(/\.[^.]+$/, ''),
    }))
    setFileBatchLabel(batchLabelFor(files))
    setFileItems([...fileItemsRef.current, ...items])
    void processFileQueue()
  }
  // A lone file picked/dropped while the Title field is filled keeps
  // the long-standing behavior of using that title (the field is
  // shared with the Paste card); with several files, each is titled
  // by its own name — one title cannot name many documents.
  const enqueuePicked = (files: { file: File; relPath: string }[]) => {
    if (files.length === 1 && title.trim()) {
      enqueueFiles(files, title.trim())
      setTitle('')
    } else {
      enqueueFiles(files)
    }
  }
  const filesBusy = fileItems.some((i) => isPendingStatus(i.status))
  const activeFileItem = fileItems.find((i) => i.status === 'uploading' || i.status === 'indexing')
  const fileNow = useNow(filesBusy)
  // Piece 17: pasted text gets the same staged progress as every
  // other ingest surface (shared UploadProgressList) instead of a
  // bare "Indexing…" button label. A text post has no measurable
  // byte progress, so the wait is the Indexing stage; Ready is set
  // only when the server's ingest response lands.
  const [pasteItem, setPasteItem] = useState<(ProgressItemView & { title: string; text: string }) | null>(null)
  const pasteKeyRef = useRef(0)
  const pasteBusy = pasteItem != null && isPendingStatus(pasteItem.status)
  const runPaste = async (itemTitle: string, text: string) => {
    const key = ++pasteKeyRef.current
    setPasteItem({
      key, name: itemTitle, status: 'indexing', progress: null,
      startedAt: Date.now(), title: itemTitle, text,
    })
    try {
      const r = await vaultApi.uploadText(itemTitle, 'pasted.txt', text, matterId)
      setPasteItem((cur) => (cur && cur.key === key
        ? { ...cur, status: 'ready', progress: 1, note: `${r.chunks} chunks indexed` }
        : cur))
      afterChange(r, 'Pasted text')
      setPaste('')
      setTitle('')
    } catch (e) {
      setPasteItem((cur) => (cur && cur.key === key
        ? { ...cur, status: 'failed', error: (e as Error).message }
        : cur))
    }
  }
  const ingestStartRef = useRef(0)
  const ingest = useMutation({
    mutationFn: () => {
      ingestStartRef.current = Date.now()
      return vaultApi.ingestS3(prefix || undefined)
    },
    onSuccess: (r) => afterChange(r, 'Bucket ingest'),
    onError: (e) => toast('error', (e as Error).message),
  })
  const ingestNow = useNow(ingest.isPending)
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

  async function handleDrop(e: React.DragEvent) {
    e.preventDefault()
    setDragging(false)
    enqueuePicked(await filesFromDataTransfer(e.dataTransfer))
  }

  const busy = filesBusy || pasteBusy || ingest.isPending

  return (
    <div className="max-w-4xl mx-auto px-6 py-10">
      <h1 className="font-display text-[2rem] tracking-tight">Vault</h1>
      <p className="text-stone-500 mt-1.5 text-[15px]">
        Documents are parsed (PDF text layer + OCR fallback), chunked, embedded and added to the knowledge graph.
      </p>
      {matterId && (
        <p className="mt-2 inline-flex rounded-full bg-stone-500/10 text-stone-600 px-3 py-1 text-[12px] font-medium">
          Showing documents in {scopeTitle ?? 'the active brief'} — uploads file here
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
        {filesBusy ? (
          <Loader2 size={26} className="animate-spin mx-auto text-ink" />
        ) : (
          <FileUp size={26} className="mx-auto text-stone-400" />
        )}
        <p className="font-medium text-[15px] mt-3">
          {filesBusy && activeFileItem
            ? `${activeFileItem.status === 'indexing' ? 'Indexing' : 'Uploading'} — ${activeFileItem.name}`
              + (activeFileItem.status === 'uploading' && activeFileItem.progress != null
                  ? ` ${Math.round(activeFileItem.progress * 100)}%` : '')
              + (activeFileItem.startedAt != null ? ` · ${formatElapsed(fileNow - activeFileItem.startedAt)}` : '')
              + (fileItems.filter((i) => isPendingStatus(i.status)).length > 1
                  ? ` · ${fileItems.filter((i) => isPendingStatus(i.status)).length} in queue` : '')
            : filesBusy
              ? 'Adding documents…'
              : 'Drop files here, or click to browse'}
        </p>
        {filesBusy && activeFileItem && (
          <div className="h-1.5 mt-4 max-w-sm mx-auto rounded-full bg-stone-200 overflow-hidden">
            <div
              className={`h-full transition-all duration-300 ${
                activeFileItem.status === 'indexing' ? 'bg-stone-400 animate-pulse' : 'bg-stone-500'
              } ${activeFileItem.status === 'uploading' && activeFileItem.progress == null ? 'animate-pulse' : ''}`}
              style={{
                width:
                  activeFileItem.status === 'uploading' && activeFileItem.progress != null
                    ? `${Math.round(activeFileItem.progress * 100)}%`
                    : '100%',
              }}
            />
          </div>
        )}
        <p className="text-[13px] text-stone-500 mt-1 flex items-center justify-center gap-1.5 flex-wrap">
          {UPLOAD_FORMATS_HINT} <span className="inline-flex items-center gap-1 rounded-full bg-stone-100 border border-stone-200 px-2 py-0.5 text-[11px]"><ScanText size={11} /> scanned pages & photos auto-OCR'd</span>
        </p>
        <input
          ref={fileRef} type="file" multiple className="hidden"
          accept={UPLOAD_ACCEPT}
          onChange={(e) => {
            const files = Array.from(e.target.files ?? []).map((file) => ({ file, relPath: '' }))
            e.target.value = ''
            enqueuePicked(files)
          }}
        />
      </motion.div>

      <div className="mt-3 flex items-center justify-center gap-3 flex-wrap">
        <label className="inline-flex items-center gap-1.5 rounded-full border border-stone-300 px-3.5 py-1.5 text-[13px] font-medium cursor-pointer hover:bg-stone-100 transition-colors">
          <FolderUp size={13} /> Upload a folder
          <input
            ref={folderRef} type="file" className="hidden"
            accept={UPLOAD_ACCEPT}
            {...({ webkitdirectory: '' } as Record<string, string>)}
            onChange={(e) => {
              const files = Array.from(e.target.files ?? []).map((file) => ({
                file,
                relPath: file.webkitRelativePath || '',
              }))
              e.target.value = ''
              enqueuePicked(files)
            }}
          />
        </label>
        <span className="text-[12.5px] text-stone-400">
          up to {MAX_BATCH_FILES} files and {MAX_BATCH_FOLDERS} folders per batch
        </span>
      </div>

      {fileItems.length > 0 && (
        <UploadProgressList
          className="mt-4"
          items={fileItems}
          headerLabel={fileBatchLabel ?? undefined}
          onClear={() => { setFileItems([]); setFileBatchLabel(null) }}
          onRetry={(key) => {
            patchFileItem(key, { status: 'queued', error: undefined, progress: null })
            void processFileQueue()
          }}
          onDismiss={(key) => setFileItems(fileItemsRef.current.filter((i) => i.key !== key))}
        />
      )}

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
            onClick={() => void runPaste(title || 'Pasted document', paste)} disabled={busy || !paste.trim()}
            className="mt-2 rounded-lex border border-stone-300 px-3.5 py-2 text-[13px] font-medium hover:bg-stone-100 disabled:opacity-50"
          >
            Index pasted text
          </button>
          {pasteItem && (
            <UploadProgressList
              items={[pasteItem]}
              onRetry={() => void runPaste(pasteItem.title, pasteItem.text)}
              onDismiss={() => setPasteItem(null)}
            />
          )}
        </div>
        <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex p-5">
          <h3 className="font-medium text-[15px]">Ingest from S3 / R2</h3>
          <p className="text-[13px] text-stone-500 mt-2 leading-relaxed">
            Pulls every supported file under a prefix from the bucket configured on the server
            (works with AWS S3 and Cloudflare R2 endpoints). Bulk-ingested documents are filed
            into the default brief.
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
            {ingest.isPending ? `Ingesting… ${formatElapsed(ingestNow - ingestStartRef.current)}` : 'Start ingest'}
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
        <EmptyState title="No documents in this brief yet" hint="Drop a PDF above to file the first one here — documents live inside briefs, and this page shows only the active brief." />
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
                onClick={async () => {
                  const res = await openOriginalDocument(d.id)
                  if (res === 'not-retained') {
                    toast('info', "The original file wasn't retained for this document — it was uploaded before originals were kept. Its extracted text is what exists.")
                  } else if (res === 'error') {
                    toast('error', 'Could not open the original document')
                  }
                }}
                title="Original — open the file as it was uploaded"
                className="p-2 rounded-lex text-stone-400 hover:text-ink hover:bg-stone-100 transition-colors"
              >
                <ExternalLink size={15} />
              </button>
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
