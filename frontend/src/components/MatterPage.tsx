import { useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ArrowLeft, FileText, Paperclip, X } from 'lucide-react'
import { mattersApi, vaultApi } from '../api/client'
import { useSession } from '../stores/session'
import { EmptyState, Spinner, StatCard } from './ui'
import { StatusPill } from './MattersPage'
import AssistantPage from './AssistantPage'
import InteractPage from './InteractPage'
import DraftPage from './DraftPage'
import { UploadProgressList, type ProgressItemView } from './UploadProgress'

const TABS = ['Overview', 'Documents', 'Ask', 'Interact', 'Draft', 'History', 'Notes'] as const
type Tab = (typeof TABS)[number]

const KIND_LABEL: Record<string, string> = {
  query: 'Asked', research: 'Researched', workflow: 'Workflow', interact: 'Interact',
  draft: 'Draft', upload: 'Upload', document: 'Document filed', created: 'Created',
}

export default function MatterPage({ matterId, back, go }: { matterId: string; back: () => void; go?: (view: string) => void }) {
  const [tab, setTab] = useState<Tab>('Overview')
  const detail = useQuery({ queryKey: ['matter', matterId], queryFn: () => mattersApi.get(matterId) })
  const m = detail.data

  return (
    <div className="max-w-5xl mx-auto px-6 py-8">
      <button onClick={back} className="flex items-center gap-1.5 text-[13px] text-stone-500 hover:text-ink">
        <ArrowLeft size={14} /> All briefs
      </button>

      {detail.isLoading || !m ? (
        <div className="mt-10"><Spinner label="Opening brief…" /></div>
      ) : (
        <>
          <div className="mt-4 flex flex-wrap items-start gap-3">
            <div className="min-w-0 flex-1">
              <h1 className="font-display text-[2rem] leading-tight tracking-tight">{m.title}</h1>
              <p className="text-stone-500 mt-1 text-[14px]">
                {[m.client, m.kind].filter(Boolean).join(' · ')}
                {' · opened '}
                {new Date(m.created_at).toLocaleDateString('en-IN', { day: 'numeric', month: 'short', year: 'numeric' })}
              </p>
            </div>
            <StatusSelect matterId={matterId} status={m.status} />
            <StatusPill status={m.status} />
          </div>

          <div className="mt-6 flex gap-1 border-b border-stone-200 overflow-x-auto">
            {TABS.map((t) => (
              <button
                key={t}
                onClick={() => setTab(t)}
                className={`px-3.5 py-2 text-[14px] whitespace-nowrap border-b-2 -mb-px transition-colors ${
                  tab === t ? 'border-ink text-ink font-medium' : 'border-transparent text-stone-500 hover:text-ink'
                }`}
              >
                {t}
              </button>
            ))}
          </div>

          <div className="mt-6">
            {tab === 'Overview' && <Overview matterId={matterId} notes={m.notes} />}
            {tab === 'Documents' && <Documents matterId={matterId} />}
            {tab === 'Ask' && <AssistantPage matterId={matterId} />}
            {tab === 'Interact' && <InteractPage matterId={matterId} go={go} />}
            {tab === 'Draft' && <DraftPage matterId={matterId} />}
            {tab === 'History' && <History matterId={matterId} />}
            {tab === 'Notes' && <Notes matterId={matterId} initial={m.notes} />}
          </div>
        </>
      )}
    </div>
  )
}

function StatusSelect({ matterId, status }: { matterId: string; status: string }) {
  const qc = useQueryClient()
  const toast = useSession((s) => s.toast)
  const update = useMutation({
    mutationFn: (s: string) => mattersApi.update(matterId, { status: s }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['matter', matterId] })
      qc.invalidateQueries({ queryKey: ['matters'] })
    },
    onError: (e) => toast('error', (e as Error).message),
  })
  return (
    <select
      value={status}
      onChange={(e) => update.mutate(e.target.value)}
      className="rounded-lex border border-stone-200 bg-white px-2.5 py-1.5 text-[13px] outline-none focus:border-stone-400"
      title="Brief status"
    >
      {['Active', 'On hold', 'Closed'].map((s) => <option key={s}>{s}</option>)}
    </select>
  )
}

function Overview({ matterId, notes }: { matterId: string; notes: string }) {
  const memory = useQuery({ queryKey: ['matter-memory', matterId], queryFn: () => mattersApi.memory(matterId) })
  const timeline = useQuery({ queryKey: ['matter-timeline', matterId], queryFn: () => mattersApi.timeline(matterId) })
  const d = memory.data
  return (
    <div>
      <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
        <StatCard label="Documents" value={d?.document_count ?? '—'} />
        <StatCard label="Chunks indexed" value={d?.chunk_count ?? '—'} />
        <StatCard label="Activities" value={d?.activity_count ?? '—'} />
        <StatCard
          label="Last activity"
          value={d?.last_activity_at ? new Date(d.last_activity_at).toLocaleDateString('en-IN', { day: 'numeric', month: 'short' }) : '—'}
        />
      </div>

      {notes && (
        <div className="mt-5 rounded-lex-lg border border-stone-200 bg-white shadow-lex px-5 py-4">
          <p className="text-[11px] uppercase tracking-wider text-stone-400 font-medium">Notes</p>
          <p className="text-[14px] mt-1.5 whitespace-pre-wrap">{notes}</p>
        </div>
      )}

      <div className="mt-6">
        <p className="text-[11px] uppercase tracking-wider text-stone-400 font-medium">
          Brief memory — who &amp; what this brief is about
        </p>
        {memory.isLoading ? (
          <div className="mt-3"><Spinner /></div>
        ) : !d || d.entities.length === 0 ? (
          <p className="text-[13px] text-stone-500 mt-2">No entities yet — file documents into this brief and its memory builds itself.</p>
        ) : (
          <div className="mt-3 flex flex-wrap gap-2">
            {d.entities.map((e) => (
              <span key={e.name} className="rounded-full border border-stone-200 bg-white px-3 py-1.5 text-[13px] shadow-lex" title={`${e.kind} · mentioned in ${e.mentions} chunks`}>
                {e.name}
                <span className="text-stone-400 ml-1.5 text-[11px]">{e.kind} · {e.mentions}</span>
              </span>
            ))}
          </div>
        )}
      </div>

      <div className="mt-8">
        <p className="text-[11px] uppercase tracking-wider text-stone-400 font-medium">Recent history</p>
        <div className="mt-3">
          <TimelineList items={(timeline.data ?? []).slice(0, 6)} loading={timeline.isLoading} />
        </div>
      </div>
    </div>
  )
}

function TimelineList({ items, loading }: { items: import('../api/types').TimelineItem[]; loading?: boolean }) {
  if (loading) return <Spinner />
  if (items.length === 0) return <EmptyState title="Nothing here yet" hint="Work done inside this brief will appear in its history." />
  return (
    <div className="space-y-2">
      {items.map((t, i) => (
        <div key={i} className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-4 py-3 flex gap-3">
          <div className="min-w-0 flex-1">
            <p className="text-[14px] font-medium truncate">{t.title}</p>
            {t.excerpt && <p className="text-[13px] text-stone-500 mt-0.5 line-clamp-2">{t.excerpt}</p>}
            <p className="text-[12px] text-stone-400 mt-1">
              {new Date(t.at).toLocaleString('en-IN', { day: 'numeric', month: 'short', hour: 'numeric', minute: '2-digit' })}
              {t.citations > 0 ? ` · ${t.citations} sources` : ''}
              {t.tokens > 0 ? ` · ${t.tokens.toLocaleString()} tokens` : ''}
              {t.user_email ? ` · ${t.user_email}` : ''}
            </p>
          </div>
          <span className="text-[11px] font-medium rounded-full px-2 py-0.5 bg-stone-500/10 text-stone-500 h-fit whitespace-nowrap">
            {KIND_LABEL[t.kind] ?? t.kind}
          </span>
        </div>
      ))}
    </div>
  )
}

function History({ matterId }: { matterId: string }) {
  const timeline = useQuery({ queryKey: ['matter-timeline', matterId], queryFn: () => mattersApi.timeline(matterId) })
  return <TimelineList items={timeline.data ?? []} loading={timeline.isLoading} />
}

function Documents({ matterId }: { matterId: string }) {
  const toast = useSession((s) => s.toast)
  const qc = useQueryClient()
  const detail = useQuery({ queryKey: ['matter', matterId], queryFn: () => mattersApi.get(matterId) })
  const allDocs = useQuery({ queryKey: ['vault-docs', 'all'], queryFn: vaultApi.list })
  const [paste, setPaste] = useState('')
  const [pasteTitle, setPasteTitle] = useState('')
  const [attachId, setAttachId] = useState('')

  const invalidate = () => {
    qc.invalidateQueries({ queryKey: ['matter', matterId] })
    qc.invalidateQueries({ queryKey: ['matter-memory', matterId] })
    qc.invalidateQueries({ queryKey: ['matters'] })
    qc.invalidateQueries({ queryKey: ['docs'] })
  }
  // ---- Add-documents progress (Piece 17): the same staged
  // Uploading → Indexing → Ready/Failed feedback Interact has,
  // rendered by the shared UploadProgressList — this panel used to
  // show a bare "Indexing…" button label for the whole ingest.
  // Items process sequentially (ingest is LLM-backed); Ready means
  // the server's ingest response landed, and a failure stays inline
  // with the server's error plus Retry/Dismiss.
  type AddPayload =
    | { kind: 'file'; file: File; title: string }
    | { kind: 'text'; title: string; text: string }
  interface AddItem extends ProgressItemView { payload: AddPayload }
  const [addItems, setAddItemsState] = useState<AddItem[]>([])
  const addItemsRef = useRef<AddItem[]>([])
  const addKeyRef = useRef(0)
  const addProcessingRef = useRef(false)
  const matterRef = useRef(matterId)
  matterRef.current = matterId
  const setAddItems = (next: AddItem[]) => {
    addItemsRef.current = next
    setAddItemsState(next)
  }
  const patchAddItem = (key: number, patch: Partial<AddItem>) =>
    setAddItems(addItemsRef.current.map((i) => (i.key === key ? { ...i, ...patch } : i)))
  const processAddQueue = async () => {
    if (addProcessingRef.current) return
    addProcessingRef.current = true
    try {
      for (;;) {
        const item = addItemsRef.current.find((i) => i.status === 'queued')
        if (!item) break
        const isFile = item.payload.kind === 'file'
        patchAddItem(item.key, {
          status: isFile ? 'uploading' : 'indexing',
          progress: isFile ? 0 : null,
          startedAt: Date.now(),
        })
        try {
          const p = item.payload
          const r = p.kind === 'file'
            ? await vaultApi.uploadFile(p.title, p.file, matterRef.current, (fraction) => {
                if (fraction !== null && fraction >= 1) {
                  patchAddItem(item.key, { status: 'indexing', progress: 1 })
                } else {
                  patchAddItem(item.key, { status: 'uploading', progress: fraction })
                }
              })
            : await vaultApi.uploadText(p.title, 'pasted.txt', p.text, matterRef.current)
          patchAddItem(item.key, { status: 'ready', progress: 1, note: `${r.chunks} chunks indexed` })
          invalidate()
          toast('success', `Filed — ${r.chunks} chunks indexed`)
        } catch (e) {
          patchAddItem(item.key, { status: 'failed', error: (e as Error).message })
        }
      }
    } finally {
      addProcessingRef.current = false
    }
  }
  const enqueueAdd = (payload: AddPayload, name: string) => {
    const item: AddItem = { key: ++addKeyRef.current, name, status: 'queued', payload }
    setAddItems([...addItemsRef.current, item])
    void processAddQueue()
  }
  const attach = useMutation({
    mutationFn: () => mattersApi.attach(matterId, attachId),
    onSuccess: () => { invalidate(); setAttachId(''); toast('success', 'Document filed in this brief') },
    onError: (e) => toast('error', (e as Error).message),
  })
  const detach = useMutation({
    mutationFn: (docId: string) => mattersApi.detach(matterId, docId),
    onSuccess: invalidate,
    onError: (e) => toast('error', (e as Error).message),
  })

  const docs = detail.data?.documents ?? []
  const filedIds = new Set(detail.data?.document_ids ?? [])
  const unfiled = (allDocs.data ?? []).filter((d) => !filedIds.has(d.id))

  return (
    <div>
      {detail.isLoading ? (
        <Spinner label="Loading documents…" />
      ) : docs.length === 0 ? (
        <EmptyState title="No documents in this brief yet" hint="Upload a file, paste text, or file an existing Vault document below." />
      ) : (
        <div className="space-y-2">
          {docs.map((d) => (
            <div key={d.id} className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-4 py-3 flex items-center gap-3">
              <FileText size={16} className="text-stone-400 flex-shrink-0" />
              <div className="min-w-0 flex-1">
                <p className="text-[14px] font-medium truncate">{d.title}</p>
                <p className="text-[12px] text-stone-400">
                  {d.chunks} chunks · {d.chars.toLocaleString()} chars ·{' '}
                  {new Date(d.ingested_at).toLocaleDateString('en-IN', { day: 'numeric', month: 'short', year: 'numeric' })}
                </p>
              </div>
              <button
                onClick={() => detach.mutate(d.id)}
                title="Remove from this brief (document stays in the Vault)"
                className="text-stone-400 hover:text-ink p-1"
              >
                <X size={15} />
              </button>
            </div>
          ))}
        </div>
      )}

      <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-5 py-4 mt-5">
        <p className="text-[11px] uppercase tracking-wider text-stone-400 font-medium">Add documents</p>
        <div className="mt-3 flex flex-wrap items-center gap-3">
          <label className="rounded-lex bg-ink text-ivory px-3.5 py-2 text-[13px] font-medium cursor-pointer hover:bg-ink-900 transition-colors">
            Upload a file
            <input
              type="file" className="hidden"
              onChange={(e) => {
                const f = e.target.files?.[0]
                if (f) enqueueAdd({ kind: 'file', file: f, title: pasteTitle || f.name.replace(/\.[^.]+$/, '') }, f.name)
                e.target.value = ''
              }}
            />
          </label>
          {unfiled.length > 0 && (
            <div className="flex items-center gap-2">
              <select
                value={attachId} onChange={(e) => setAttachId(e.target.value)}
                className="rounded-lex border border-stone-200 bg-white px-2.5 py-2 text-[13px] outline-none max-w-[240px]"
              >
                <option value="">File an existing Vault document…</option>
                {unfiled.map((d) => <option key={d.id} value={d.id}>{d.title}</option>)}
              </select>
              <button
                disabled={!attachId || attach.isPending}
                onClick={() => attach.mutate()}
                className="flex items-center gap-1.5 rounded-lex border border-stone-200 px-3 py-2 text-[13px] font-medium disabled:opacity-40 hover:border-stone-300"
              >
                <Paperclip size={13} /> File it
              </button>
            </div>
          )}
        </div>
        <input
          value={pasteTitle} onChange={(e) => setPasteTitle(e.target.value)}
          placeholder="Title for pasted text (optional)"
          className="mt-4 w-full rounded-lex border border-stone-200 px-3 py-2 text-[13px] outline-none focus:border-stone-400"
        />
        <textarea
          value={paste} onChange={(e) => setPaste(e.target.value)}
          placeholder="…or paste document text here"
          rows={4}
          className="mt-2 w-full rounded-lex border border-stone-200 px-3 py-2 text-[13px] outline-none focus:border-stone-400"
        />
        <button
          disabled={!paste.trim()}
          onClick={() => {
            enqueueAdd(
              { kind: 'text', title: pasteTitle || 'Pasted document', text: paste },
              pasteTitle || 'Pasted document',
            )
            setPaste('')
          }}
          className="mt-2 rounded-lex border border-stone-200 px-3.5 py-2 text-[13px] font-medium disabled:opacity-40 hover:border-stone-300"
        >
          File pasted text
        </button>
        <UploadProgressList
          items={addItems}
          onRetry={(key) => {
            patchAddItem(key, { status: 'queued', error: undefined, progress: null })
            void processAddQueue()
          }}
          onDismiss={(key) => setAddItems(addItemsRef.current.filter((i) => i.key !== key))}
        />
      </div>
    </div>
  )
}

function Notes({ matterId, initial }: { matterId: string; initial: string }) {
  const toast = useSession((s) => s.toast)
  const qc = useQueryClient()
  const [notes, setNotes] = useState(initial)
  const save = useMutation({
    mutationFn: () => mattersApi.update(matterId, { notes }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['matter', matterId] })
      qc.invalidateQueries({ queryKey: ['matters'] })
      toast('success', 'Notes saved to the brief memory')
    },
    onError: (e) => toast('error', (e as Error).message),
  })
  return (
    <div>
      <p className="text-[13px] text-stone-500">
        Brief notes are part of its living memory — key facts, forum, next dates, strategy.
      </p>
      <textarea
        value={notes} onChange={(e) => setNotes(e.target.value)}
        rows={10}
        placeholder="What should anyone opening this brief know?"
        className="mt-3 w-full rounded-lex-lg border border-stone-200 bg-white shadow-lex px-4 py-3 text-[14px] outline-none focus:border-stone-400"
      />
      <button
        onClick={() => save.mutate()} disabled={save.isPending}
        className="mt-3 rounded-lex bg-ink text-ivory px-4 py-2 text-[14px] font-medium disabled:opacity-40 hover:bg-ink-900 transition-colors"
      >
        {save.isPending ? 'Saving…' : 'Save notes'}
      </button>
    </div>
  )
}
