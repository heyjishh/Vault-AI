import { useEffect, useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { ArrowUp, Briefcase, CalendarClock, Copy, FileSearch, FolderUp, ListTree, Lock, RotateCcw, ScanSearch, ShieldAlert, Upload } from 'lucide-react'
import { createApi, privateVaultApi, vaultApi, type InteractDocumentResult, type InteractResponse, type InteractSource } from '../api/client'
import type { Citation } from '../api/types'
import { useSession } from '../stores/session'
import { RefineControls, useRefine } from './RefineControls'
import AssistantAvatar from './AssistantAvatar'
import { SourceModal, citeMarkdownComponents, linkifyCitations } from './SourceModal'
import { ResponseDownload, exportCitations } from './ResponseDownload'
import { EmptyState } from './ui'
import {
  UploadProgressList, UPLOAD_ACCEPT, UPLOAD_FORMATS_HINT, MAX_BATCH_FILES, MAX_BATCH_FOLDERS,
  applyBatchCaps, batchLabelFor, filesFromDataTransfer,
} from './UploadProgress'

/** The intake helpers live in the shared upload module (Piece 18) so
 *  Interact, the Brief page and the Vault page all apply the same
 *  batch caps; re-exported here for this module's previous surface. */
export {
  UPLOAD_ACCEPT, MAX_BATCH_FILES, MAX_BATCH_FOLDERS, topFolderOf,
  applyBatchCaps, batchLabelFor, filesFromDataTransfer,
} from './UploadProgress'

export interface UploadItem {
  key: number
  file: File
  relPath: string
  status: 'queued' | 'uploading' | 'indexing' | 'ready' | 'failed' | 'skipped'
  /** Bytes-sent fraction (0..1) during 'uploading'; null when the
   *  transport cannot measure it (indeterminate bar). */
  progress?: number | null
  error?: string
  docId?: string
  /** e.g. "selected" / "already selected" / "selection full". */
  note?: string
  /** When this item started processing (ms epoch) — elapsed readout. */
  startedAt?: number
}

const ACTIONS = [
  { id: 'summary', label: 'Summary', icon: FileSearch, hint: 'Parties, key terms, dates, outcomes' },
  { id: 'chronology', label: 'Chronology', icon: CalendarClock, hint: 'Dated timeline of events' },
  { id: 'clauses', label: 'Key clauses', icon: ListTree, hint: 'Operative provisions & who they favour' },
  { id: 'risks', label: 'Risk review', icon: ShieldAlert, hint: 'Onerous terms, gaps, deadlines' },
  { id: 'chat', label: 'Ask the documents', icon: ScanSearch, hint: 'Free-form question across the selection' },
] as const

/** The document actions (chat is the composer, not a button). */
const ACTION_BUTTONS = ACTIONS.filter((a) => a.id !== 'chat')

/** Piece 16: the only places Interact may read from — no Web, no
 *  system data, ever. Server-enforced via the request's source. */
const SOURCES: { id: InteractSource; label: string; hint: string }[] = [
  { id: 'vault', label: 'Vault', hint: "This brief's documents only" },
  { id: 'private', label: 'Private Vault', hint: 'Only your Confidential Vault' },
  { id: 'both', label: 'Both', hint: 'Brief documents + Private Vault, labelled by source' },
]

/** Mirrors the server cap (MAX_INTERACT_DOCS in routes/create.rs):
 *  the analysis budget is split across the selected documents. */
export const MAX_INTERACT_DOCS = 20

function treatmentChip(status: string) {
  if (status === 'overruled') return 'bg-rose-100 text-rose-800'
  if (status === 'good_law' || status === 'followed' || status === 'applied') return 'bg-emerald-100 text-emerald-800'
  return 'bg-amber-100 text-amber-800'
}

/** One Interact run in the Ask-style thread (Piece 16). */
interface ITurn {
  id: number
  action: string
  question: string | null
  source: InteractSource
  docIds: string[]
  /** Piece 13 refinement terms captured at send time (chat only). */
  refineTerms?: string[]
  status: 'pending' | 'done' | 'error'
  output: string
  citations: Citation[]
  documents: InteractDocumentResult[]
  documentTitle: string
  truncated: boolean
  error: string | null
  startedAt: number
  elapsedMs: number | null
}

let turnSeq = 0

export default function InteractPage({ matterId, go }: { matterId?: string; go?: (view: string) => void } = {}) {
  const toast = useSession((s) => s.toast)
  const qc = useQueryClient()
  // History restore: reopen the exact analysis on its document set.
  const restore = useSession((s) => s.restore)
  const consumeRestore = useSession((s) => s.consumeRestore)
  const [selectedIds, setSelectedIds] = useState<string[]>([])
  // Brief-wide working set (Piece 9): in brief scope the default
  // selection is every document filed in the brief, up to the cap.
  const [briefWide, setBriefWide] = useState(true)
  // Piece 16: which sources Interact reads from.
  const [source, setSource] = useState<InteractSource>('vault')
  const [turns, setTurns] = useState<ITurn[]>([])
  const [sourceModal, setSourceModal] = useState<Citation | null>(null)
  const { data: vaultStatus } = useQuery({ queryKey: ['private-vault-status'], queryFn: privateVaultApi.status })
  const privUnlocked = vaultStatus?.unlocked === true
  // Known-locked only: while the status is still loading the server
  // remains the authority (its 400 lands in the turn as an error).
  const privateLocked = source !== 'vault' && vaultStatus !== undefined && !privUnlocked

  useEffect(() => {
    if (!restore || restore.kind !== 'interact') return
    const p = consumeRestore('interact')
    if (!p) return
    const ids = Array.isArray(p.document_ids) && p.document_ids.length > 0
      ? p.document_ids.map(String)
      : [String(p.document_id ?? '')].filter(Boolean)
    setBriefWide(false)
    setSelectedIds(ids)
    setTurns([{
      id: ++turnSeq,
      action: String(p.action ?? 'summary'),
      question: null,
      source: 'vault',
      docIds: ids,
      status: 'done',
      output: String(p.output ?? ''),
      citations: [],
      documents: [],
      documentTitle: String(p.document_title ?? ''),
      truncated: Boolean(p.truncated),
      error: null,
      startedAt: Date.now(),
      elapsedMs: null,
    }])
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [restore])

  // Brief-only vault: the document universe is exactly the active
  // brief — there is no full-vault scope or toggle. AppShell passes
  // the resolved active brief (the stored pick, or the default
  // brief when none is chosen); a brief page's Interact tab passes
  // its own brief.
  const effectiveMatterId = matterId
  const docs = useQuery({
    queryKey: ['vault-docs', effectiveMatterId ?? 'none'],
    queryFn: () => vaultApi.listInMatter(effectiveMatterId!),
    enabled: !!effectiveMatterId,
  })

  // Brief-wide derivation: whenever the shown brief (or its document
  // list) changes, the working set re-derives to the brief's first
  // MAX_INTERACT_DOCS documents. Any manual checkbox change leaves
  // brief-wide mode; the "All documents in this brief" row re-enters
  // it. Over-cap briefs are covered partially and the UI says so —
  // the derivation never silently truncates.
  const appliedWideKey = useRef<string | null>(null)
  useEffect(() => {
    // Entering a (new) brief scope resets to the brief-wide default.
    setBriefWide(true)
    appliedWideKey.current = null
  }, [effectiveMatterId])
  const briefWideKey = effectiveMatterId && docs.data
    ? `${effectiveMatterId}|${docs.data.map((d) => d.id).join(',')}`
    : null
  useEffect(() => {
    if (!briefWide || !briefWideKey || !docs.data) return
    if (appliedWideKey.current === briefWideKey) return
    appliedWideKey.current = briefWideKey
    setSelectedIds(docs.data.slice(0, MAX_INTERACT_DOCS).map((d) => d.id))
  }, [briefWide, briefWideKey, docs.data])

  useEffect(() => {
    // Arriving from document search? Select that document directly.
    const fromSearch = sessionStorage.getItem('jurisvault.interactDoc')
    if (!fromSearch) return
    sessionStorage.removeItem('jurisvault.interactDoc')
    setBriefWide(false)
    setSelectedIds([fromSearch])
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
  const [question, setQuestion] = useState('')
  // Prompt refinement (Piece 13) for the chat question.
  const refineCtl = useRefine({ value: question, setValue: setQuestion, surface: 'interact' })

  const run = useMutation({
    mutationFn: async (turn: ITurn) => {
      const r = await createApi.interact(
        turn.source === 'private' ? [] : turn.docIds,
        turn.action,
        turn.action === 'chat' ? turn.question ?? undefined : undefined,
        turn.action === 'chat' ? turn.refineTerms : undefined,
        turn.source,
      )
      return r
    },
    onSuccess: (r: InteractResponse, turn) => {
      setTurns((ts) => ts.map((t) => (t.id === turn.id ? {
        ...t,
        status: 'done',
        output: r.output,
        citations: r.citations ?? [],
        documents: r.documents ?? [],
        documentTitle: r.document_title,
        truncated: r.truncated,
        elapsedMs: Date.now() - t.startedAt,
      } : t)))
    },
    onError: (e, turn) => {
      setTurns((ts) => ts.map((t) => (t.id === turn.id ? {
        ...t,
        status: 'error',
        error: (e as Error).message,
        elapsedMs: Date.now() - t.startedAt,
      } : t)))
    },
  })

  const startTurn = (action: string, q: string | null, src: InteractSource, docIds: string[], refineTerms?: string[]) => {
    if (run.isPending) return
    const turn: ITurn = {
      id: ++turnSeq,
      action,
      question: q,
      source: src,
      docIds,
      refineTerms,
      status: 'pending',
      output: '',
      citations: [],
      documents: [],
      documentTitle: '',
      truncated: false,
      error: null,
      startedAt: Date.now(),
      elapsedMs: null,
    }
    setTurns((ts) => [...ts, turn])
    run.mutate(turn)
  }

  const sendChat = () => {
    const q = question.trim()
    if (!q) return
    // Piece 13: the terms captured by Refine ride along and fold
    // into the instruction server-side as a focus line.
    const terms = refineCtl.consumeTerms()
    startTurn('chat', q, source, [...selectedIds], terms.length ? terms : undefined)
    setQuestion('')
  }

  const toggleDoc = (id: string) => {
    // A manual pick narrows the brief-wide default into a manual set.
    setBriefWide(false)
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
  const chatBlocked = run.isPending
    || !question.trim()
    || (source !== 'private' && !hasSelection)
    || privateLocked

  // ---- Upload (Piece 7): files/folders land in the vault through
  // the normal ingest route and are auto-selected on success. The
  // queue processes sequentially (the box is small and ingest is
  // LLM-backed); each file carries its own status, and one failure
  // never blocks the rest.
  const [uploads, setUploadsState] = useState<UploadItem[]>([])
  const [batchLabel, setBatchLabel] = useState<string | null>(null)
  const [dragging, setDragging] = useState(false)
  const uploadsRef = useRef<UploadItem[]>([])
  const selectedRef = useRef<string[]>([])
  const matterRef = useRef<string | undefined>(undefined)
  const keyRef = useRef(0)
  const processingRef = useRef(false)
  selectedRef.current = selectedIds
  matterRef.current = effectiveMatterId
  const fileInputRef = useRef<HTMLInputElement>(null)
  const folderInputRef = useRef<HTMLInputElement>(null)

  const setUploads = (next: UploadItem[]) => {
    uploadsRef.current = next
    setUploadsState(next)
  }
  const patchUpload = (key: number, patch: Partial<UploadItem>) =>
    setUploads(uploadsRef.current.map((u) => (u.key === key ? { ...u, ...patch } : u)))

  const processQueue = async () => {
    if (processingRef.current) return
    processingRef.current = true
    try {
      for (;;) {
        const item = uploadsRef.current.find((u) => u.status === 'queued')
        if (!item) break
        patchUpload(item.key, { status: 'uploading', progress: 0, startedAt: Date.now() })
        try {
          const title = item.file.name.replace(/\.[^.]+$/, '')
          // Uploading = bytes in flight (progress events). Once the
          // bytes are sent, the stage flips to Indexing and stays
          // there until the server's ingest response lands — Ready
          // means the document is indexed, not just received.
          const report = await vaultApi.uploadFile(title, item.file, matterRef.current, (fraction) => {
            if (fraction !== null && fraction >= 1) {
              patchUpload(item.key, { status: 'indexing', progress: 1 })
            } else {
              patchUpload(item.key, { status: 'uploading', progress: fraction })
            }
          })
          const docId = report.doc_id ?? undefined
          let note: string | undefined
          if (docId) {
            if (selectedRef.current.includes(docId)) {
              note = 'already selected'
            } else if (selectedRef.current.length >= MAX_INTERACT_DOCS) {
              note = `selection full (max ${MAX_INTERACT_DOCS}) — uploaded, not auto-selected`
            } else {
              selectedRef.current = [...selectedRef.current, docId]
              setSelectedIds(selectedRef.current)
              note = 'selected'
            }
          }
          patchUpload(item.key, { status: 'ready', docId, note, progress: 1 })
          qc.invalidateQueries({ queryKey: ['vault-docs'] })
        } catch (e) {
          patchUpload(item.key, { status: 'failed', error: (e as Error).message })
        }
      }
    } finally {
      processingRef.current = false
    }
  }

  const addFiles = (files: { file: File; relPath: string }[]) => {
    if (files.length === 0) return
    const items: UploadItem[] = applyBatchCaps(files).map(({ file, relPath, skipReason }) => ({
      key: ++keyRef.current,
      file,
      relPath,
      status: skipReason ? ('skipped' as const) : ('queued' as const),
      error: skipReason,
    }))
    setBatchLabel(batchLabelFor(items))
    setUploads([...uploadsRef.current, ...items])
    void processQueue()
  }

  const onDrop = async (e: React.DragEvent) => {
    e.preventDefault()
    setDragging(false)
    addFiles(await filesFromDataTransfer(e.dataTransfer))
  }

  return (
    <div
      className={`max-w-5xl mx-auto px-6 py-8 ${dragging ? 'outline-dashed outline-2 outline-stone-400 outline-offset-[-8px]' : ''}`}
      onDragOver={(e) => { e.preventDefault(); setDragging(true) }}
      onDragLeave={() => setDragging(false)}
      onDrop={onDrop}
    >
      <h1 className="font-display text-[2rem] tracking-tight">Interact</h1>
      <p className="text-stone-500 mt-1.5 text-[15px]">
        Chat with your documents the way Ask chats with the vault — but Interact reads only what you point it at:
        this brief's documents, your Confidential Vault, or both. Never the web, never system data.
      </p>

      {/* ---- Piece 16: source selector ---- */}
      <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-6 py-5 mt-6">
        <span className="text-meta uppercase tracking-wider text-stone-500 font-medium">Sources</span>
        <div className="grid sm:grid-cols-3 gap-2.5 mt-3">
          {SOURCES.map((s) => (
            <button
              key={s.id}
              onClick={() => setSource(s.id)}
              className={`rounded-lex border px-3.5 py-3 text-left transition-colors ${
                source === s.id ? 'border-ink bg-stone-50' : 'border-stone-200 hover:border-stone-300'
              }`}
            >
              <span className="flex items-center gap-1.5 text-[13.5px] font-medium">
                {s.id === 'private' ? <Lock size={14} /> : s.id === 'both' ? <Briefcase size={14} /> : <FileSearch size={14} />}
                {s.label}
                {source === s.id && <span className="ml-auto text-[11px] font-semibold uppercase tracking-wide text-stone-400">Active</span>}
              </span>
              <span className="block text-[11.5px] text-stone-400 leading-snug mt-1">{s.hint}</span>
            </button>
          ))}
        </div>
        {source === 'private' && (
          <p className="text-[12.5px] text-stone-500 mt-3">
            Private mode reads only your Confidential Vault — this brief's documents are not used, and the
            answer is never cached or written into history.
          </p>
        )}
        {privateLocked && (
          <div className="mt-3 rounded-lex border border-amber-600/25 bg-[#fdf6e3] px-4 py-3 text-[13px] text-[#7a5b12] flex items-center gap-3 flex-wrap">
            <span>
              Your Confidential Vault is locked — {source === 'private' ? 'Private' : 'Both'} needs it
              unlocked. Unlock it, or switch the source to Vault.
            </span>
            {go && (
              <button
                onClick={() => go('private')}
                className="ml-auto font-medium underline underline-offset-2 whitespace-nowrap"
              >
                Open Private Vault
              </button>
            )}
          </div>
        )}
      </div>

      {source !== 'private' && (
      <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-6 py-5 mt-5">
        <div className="flex items-baseline justify-between gap-3 flex-wrap">
          <span className="text-meta uppercase tracking-wider text-stone-500 font-medium">
            Documents — {selectedIds.length} selected (max {MAX_INTERACT_DOCS})
            {effectiveMatterId && briefWide ? ' · all of this brief' : ''}
          </span>
        </div>
        {effectiveMatterId && docs.data && (
          <div className="mt-3 rounded-lex border border-stone-200 bg-stone-50 px-3 py-2.5">
            <label className="flex items-start gap-2.5 cursor-pointer">
              <input
                type="checkbox"
                checked={briefWide}
                onChange={(e) => {
                  setBriefWide(e.target.checked)
                  if (e.target.checked) appliedWideKey.current = null
                }}
                className="accent-stone-800 mt-0.5"
              />
              <span>
                <span className="text-[13.5px] font-medium">
                  All documents in this brief ({docs.data.length})
                </span>
                {docs.data.length > MAX_INTERACT_DOCS && (
                  <span className="block text-[12px] text-amber-700 mt-0.5">
                    Interact covers the first {MAX_INTERACT_DOCS} of {docs.data.length} —{' '}
                    {docs.data.length - MAX_INTERACT_DOCS} document
                    {docs.data.length - MAX_INTERACT_DOCS === 1 ? ' is' : 's are'} not covered.
                  </span>
                )}
                <span className="block text-[12px] text-stone-400 mt-0.5">
                  {briefWide
                    ? 'Interact works across the whole brief. Untick any document below to narrow the set manually.'
                    : 'Manual selection — tick this to return to the whole brief.'}
                </span>
              </span>
            </label>
          </div>
        )}
        <div className="flex items-center gap-2 mt-4 flex-wrap">
          <button
            onClick={() => fileInputRef.current?.click()}
            className="inline-flex items-center gap-1.5 rounded-lex border border-stone-300 px-3 py-1.5 text-[13px] font-medium hover:bg-stone-100"
          >
            <Upload size={14} /> Upload files
          </button>
          <button
            onClick={() => folderInputRef.current?.click()}
            className="inline-flex items-center gap-1.5 rounded-lex border border-stone-300 px-3 py-1.5 text-[13px] font-medium hover:bg-stone-100"
          >
            <FolderUp size={14} /> Upload folder
          </button>
          <span className="text-[12.5px] text-stone-400">
            or drop files/folders anywhere here — up to {MAX_BATCH_FILES} files and {MAX_BATCH_FOLDERS} folders per batch; uploads file into this brief and are selected automatically · {UPLOAD_FORMATS_HINT}
          </span>
          <input
            ref={fileInputRef}
            type="file"
            multiple
            accept={UPLOAD_ACCEPT}
            className="hidden"
            onChange={(e) => {
              const files = Array.from(e.target.files ?? []).map((file) => ({ file, relPath: '' }))
              e.target.value = ''
              addFiles(files)
            }}
          />
          <input
            ref={folderInputRef}
            type="file"
            accept={UPLOAD_ACCEPT}
            className="hidden"
            {...({ webkitdirectory: '' } as Record<string, string>)}
            onChange={(e) => {
              const files = Array.from(e.target.files ?? []).map((file) => ({
                file,
                relPath: file.webkitRelativePath || '',
              }))
              e.target.value = ''
              addFiles(files)
            }}
          />
        </div>
        {uploads.length > 0 && (
          <UploadProgressList
            headerLabel={batchLabel ?? undefined}
            items={uploads.map((u) => ({
              key: u.key,
              name: u.relPath || u.file.name,
              status: u.status,
              progress: u.progress,
              error: u.error,
              note: u.note,
              startedAt: u.startedAt,
            }))}
            onClear={() => { setUploads([]); setBatchLabel(null) }}
            onRetry={(key) => {
              patchUpload(key, { status: 'queued', error: undefined, progress: null })
              void processQueue()
            }}
            onDismiss={(key) => setUploads(uploadsRef.current.filter((u) => u.key !== key))}
          />
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
          {!effectiveMatterId && (
            <p className="px-3 py-2.5 text-[13.5px] text-stone-500">
              No brief is active yet — pick one in the “Working in” switcher.
            </p>
          )}
          {effectiveMatterId && docs.isPending && <p className="px-3 py-2.5 text-[13.5px] text-stone-400">Loading documents…</p>}
          {docs.data && docs.data.length === 0 && (
            <p className="px-3 py-2.5 text-[13.5px] text-stone-500">
              No documents are filed in this brief yet — upload above, or file documents from the brief page.
            </p>
          )}
        </div>
      </div>
      )}

      {/* ---- Thread (Ask-style) ---- */}
      <div className="mt-8">
        {turns.map((turn) => (
          <TurnView
            key={turn.id}
            turn={turn}
            onRetry={() => {
              setTurns((ts) => ts.map((t) => (t.id === turn.id ? { ...t, status: 'pending', error: null, startedAt: Date.now() } : t)))
              run.mutate({ ...turn, status: 'pending', error: null, startedAt: Date.now() })
            }}
            onCite={(c) => setSourceModal(c)}
            retryDisabled={run.isPending}
          />
        ))}
      </div>

      {/* ---- Document actions ---- */}
      {source === 'private' ? (
        <p className="text-[13px] text-stone-500 rounded-lex border border-stone-200 bg-stone-50 px-4 py-3">
          Actions need Vault documents — switch source to Vault or Both to summarise, build a chronology,
          map clauses or run a risk review. In Both, actions apply to the vault documents only.
        </p>
      ) : (
        <div className="grid sm:grid-cols-2 lg:grid-cols-4 gap-2.5">
          {ACTION_BUTTONS.map((a) => (
            <button
              key={a.id}
              disabled={run.isPending || !hasSelection}
              onClick={() => startTurn(a.id, null, source, [...selectedIds])}
              className="rounded-lex border border-stone-200 bg-white px-3.5 py-3 text-left hover:border-ink disabled:opacity-40 transition-colors"
            >
              <a.icon size={16} className="text-stone-500" />
              <p className="text-[13.5px] font-medium mt-1.5">{a.label}</p>
              <p className="text-[11.5px] text-stone-400 leading-snug">{a.hint}</p>
            </button>
          ))}
        </div>
      )}

      {/* ---- Composer (Ask-style) ---- */}
      <div className="mt-5 rounded-lex-lg border border-stone-300 bg-white shadow-lex-md">
        <textarea
          value={question}
          onChange={(e) => setQuestion(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sendChat() } }}
          placeholder={
            source === 'private'
              ? 'Ask your Confidential Vault… e.g. What was the escrow amount?'
              : 'Ask the documents… e.g. What is the notice period, and who can terminate?'
          }
          rows={3}
          className="w-full resize-none px-4 pt-4 pb-2 text-[15px] outline-none placeholder:text-stone-400 bg-transparent rounded-t-lex-lg"
        />
        <div className="flex items-center justify-between px-3 pb-3">
          <div className="flex items-center gap-2">
            <span className="inline-flex items-center gap-1.5 rounded-full bg-ink text-ivory px-2.5 py-1 text-[12px]">
              {source === 'private' ? <Lock size={13} /> : source === 'both' ? <Briefcase size={13} /> : <FileSearch size={13} />}
              {SOURCES.find((s) => s.id === source)?.label}
            </span>
            <RefineControls ctl={refineCtl} disabled={!question.trim()} />
          </div>
          <button
            onClick={sendChat}
            disabled={chatBlocked}
            className="w-8 h-8 rounded-full bg-ink text-ivory flex items-center justify-center hover:bg-ink-900 disabled:opacity-40 transition-colors"
            title="Ask (Enter)"
          >
            <ArrowUp size={16} />
          </button>
        </div>
        <div className="border-t border-stone-200 bg-stone-100/60 px-3 py-2 flex rounded-b-lex-lg">
          <span className="text-[11px] text-stone-400 self-center">
            {source === 'private'
              ? 'Private answers are never cached or saved to history'
              : source === 'both'
                ? 'Citations are labelled [doc:N] for documents and [private:N] for your Confidential Vault'
                : 'Answers cite the selected documents as [doc:N]'}
          </span>
          <span className="ml-auto hidden sm:block text-[11px] text-stone-400 self-center">Enter to ask · Shift+Enter for a new line</span>
        </div>
      </div>

      {turns.length === 0 && source !== 'private' && !hasSelection && (
        <div className="mt-5"><EmptyState title="Pick a document to begin" hint="Interact reads the documents themselves — not the whole corpus." /></div>
      )}
      {turns.length === 0 && source === 'private' && (
        <div className="mt-5"><EmptyState title="Ask your Confidential Vault" hint="Only your private documents are read — nothing from this brief, nothing cached, nothing saved to history." /></div>
      )}

      {sourceModal && <SourceModal citation={sourceModal} onClose={() => setSourceModal(null)} go={go} />}
    </div>
  )
}

function TurnView({ turn, onRetry, onCite, retryDisabled }: {
  turn: ITurn
  onRetry: () => void
  onCite: (c: Citation) => void
  retryDisabled: boolean
}) {
  const toast = useSession((s) => s.toast)
  const [tick, setTick] = useState(0)
  useEffect(() => {
    if (turn.status !== 'pending') return
    const h = setInterval(() => setTick((v) => v + 1), 500)
    return () => clearInterval(h)
  }, [turn.status])
  void tick
  const secs = (((turn.elapsedMs ?? Date.now() - turn.startedAt)) / 1000).toFixed(1)
  const actionLabel = ACTIONS.find((a) => a.id === turn.action)?.label ?? turn.action
  const heading = turn.action === 'chat'
    ? turn.question ?? ''
    : `${actionLabel} — ${turn.documentTitle || 'documents'}${turn.documents.length > 1 ? ` +${turn.documents.length - 1} more` : ''}`

  return (
    <div className="mb-10">
      <h2 className="font-display text-[1.6rem] leading-snug tracking-tight">{heading}</h2>

      {turn.status === 'pending' && (
        <div className="mt-3 flex items-center gap-2.5 text-stone-500 text-[13px]">
          <AssistantAvatar size={22} playing float={false} />
          <span className="font-medium text-ink">Clause</span>
          {turn.source === 'private' ? 'Reading your Confidential Vault…' : 'Reading the documents…'}
          <span className="text-stone-300">·</span>
          <span className="tabular-nums">{secs}s</span>
        </div>
      )}

      {turn.status === 'error' ? (
        <div className="mt-4 rounded-lex border border-brand-red/30 bg-[#feeaea] px-4 py-3 text-[13px] text-[#a02c2c] flex items-center justify-between">
          <span>{turn.error}</span>
          <button onClick={onRetry} disabled={retryDisabled} className="flex items-center gap-1.5 font-medium underline underline-offset-2 disabled:opacity-40">
            <RotateCcw size={13} /> Retry
          </button>
        </div>
      ) : turn.status === 'done' ? (
        <>
          {turn.documents.length > 0 && (
            <div className="flex gap-1.5 flex-wrap mt-3">
              {turn.documents.map((d) => (
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
              {turn.truncated && <span className="text-[12px] text-stone-400 self-center">Long document set — analysed within the shared budget</span>}
            </div>
          )}
          <div className="prose-lex mt-3 text-[14.5px] leading-relaxed">
            <ReactMarkdown
              remarkPlugins={[remarkGfm]}
              components={turn.citations.length > 0 ? citeMarkdownComponents(turn.citations, onCite) : undefined}
            >
              {turn.citations.length > 0 ? linkifyCitations(turn.output, turn.citations) : turn.output}
            </ReactMarkdown>
          </div>

          {turn.citations.length > 0 && (
            <div className="mt-4 grid sm:grid-cols-2 gap-2.5">
              {turn.citations.map((c) => {
                const isPrivate = c.doc_id.startsWith('private:')
                return (
                  <button
                    key={c.chunk_id + c.doc_id}
                    onClick={() => onCite(c)}
                    title="View source"
                    className="text-left rounded-lex-lg border border-stone-200 bg-white shadow-lex px-4 py-3 hover:border-stone-400 transition-colors"
                  >
                    <span className="flex items-center gap-2">
                      <span className="rounded bg-ink text-ivory px-1.5 py-0.5 text-[11px] font-mono font-semibold">{c.doc_id}</span>
                      {isPrivate && (
                        <span className="rounded-full bg-ink text-ivory px-1.5 py-px text-[10px] font-semibold uppercase tracking-wide">Private</span>
                      )}
                      <span className="text-[13px] font-medium truncate">{c.doc_title ?? c.doc_id}</span>
                    </span>
                    <span className="block text-[12.5px] text-stone-500 leading-relaxed mt-1.5 line-clamp-3">{c.snippet}</span>
                  </button>
                )
              })}
            </div>
          )}

          <div className="mt-4 flex items-center gap-2">
            <button
              onClick={() => { navigator.clipboard.writeText(turn.output).catch(() => {}); toast('success', 'Answer copied') }}
              className="flex items-center gap-1.5 rounded-lex border border-stone-200 bg-white px-2.5 py-1.5 text-[12px] font-medium text-stone-600 hover:text-ink hover:border-stone-300 transition-colors"
            >
              <Copy size={13} /> Copy
            </button>
            <ResponseDownload
              question={
                turn.action === 'chat'
                  ? turn.question ?? ''
                  : `${actionLabel} — ${turn.documents.length > 0 ? turn.documents.map((d) => d.document_title).join(', ') : turn.documentTitle || 'documents'}`
              }
              answerMarkdown={turn.output}
              citations={exportCitations(turn.citations)}
            />
            {turn.elapsedMs != null && (
              <span className="text-[12px] text-stone-400">answered in {(turn.elapsedMs / 1000).toFixed(1)}s</span>
            )}
          </div>
        </>
      ) : null}
    </div>
  )
}
