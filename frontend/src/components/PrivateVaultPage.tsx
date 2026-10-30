import { useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Fingerprint, FolderUp, KeyRound, Lock, LockOpen, ShieldCheck, Trash2 } from 'lucide-react'
import { privateVaultApi, type PrivateDocMeta } from '../api/client'
import { useSession } from '../stores/session'
import { EmptyState, Spinner } from './ui'
import {
  UploadProgressList,
  applyBatchCaps, batchLabelFor,
  MAX_BATCH_FILES, MAX_BATCH_FOLDERS,
  type ProgressItemView,
} from './UploadProgress'

/**
 * The Confidential Vault: documents encrypted under the user's own vault
 * password. Titles and text exist only as ciphertext on the server; the AI
 * can search them only while the vault is unlocked, and only for the owner.
 */
export default function PrivateVaultPage() {
  const queryClient = useQueryClient()
  const toast = useSession((s) => s.toast)
  const status = useQuery({ queryKey: ['private-vault-status'], queryFn: privateVaultApi.status })
  const docs = useQuery({
    queryKey: ['private-vault-docs'],
    queryFn: privateVaultApi.list,
    enabled: status.data?.unlocked === true,
  })
  const [password, setPassword] = useState('')
  const [newPassword, setNewPassword] = useState('')
  const [showChange, setShowChange] = useState(false)
  const [title, setTitle] = useState('')
  const [text, setText] = useState('')
  const fileRef = useRef<HTMLInputElement>(null)
  const folderRef = useRef<HTMLInputElement>(null)

  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: ['private-vault-status'] })
    void queryClient.invalidateQueries({ queryKey: ['private-vault-docs'] })
  }
  const onErr = (e: unknown) => toast('error', (e as Error).message)

  const setPw = useMutation({
    mutationFn: () => privateVaultApi.setPassword(password),
    onSuccess: () => { setPassword(''); refresh(); toast('success', 'Vault password set — vault unlocked') },
    onError: onErr,
  })
  const unlock = useMutation({
    mutationFn: () => privateVaultApi.unlock(password),
    onSuccess: () => { setPassword(''); refresh(); toast('success', 'Vault unlocked') },
    onError: onErr,
  })
  const lock = useMutation({
    mutationFn: () => privateVaultApi.lock(),
    onSuccess: () => { refresh(); toast('success', 'Vault locked — the key is gone from memory') },
    onError: onErr,
  })
  const change = useMutation({
    mutationFn: () => privateVaultApi.changePassword(password, newPassword),
    onSuccess: () => { setPassword(''); setNewPassword(''); setShowChange(false); toast('success', 'Vault password changed — everything re-encrypted') },
    onError: onErr,
  })
  // ---- Universal intake (Piece 19): the seal pickers take many
  // files at once (`multiple`) and whole folders
  // (`webkitdirectory`), like every other upload surface — this
  // page was the last single-pick holdout. Everything picked feeds
  // ONE sequential seal queue rendered by the shared
  // UploadProgressList: per item, Uploading (the file is read into
  // memory) → Sealing (the per-document seal POST — the server
  // parses, encrypts and embeds; Ready only when it confirms,
  // noted "N chunks sealed") → Ready / Failed with the server's
  // actual error plus Retry/Dismiss. Pasted text joins the same
  // queue as a one-item job (its payload is already in memory, so
  // it starts at Sealing — the Piece 17 shape). Batch caps are the
  // shared 20 files / 10 folders; overflow stays in the list as
  // skipped-with-reason rows, never silently dropped.
  //
  // Locked gating is unchanged and exact: this queue can only be
  // started from the seal section, which renders only while the
  // vault is unlocked. If the vault is locked MID-queue, the
  // processor keeps going — each remaining item's POST fails with
  // the server's locked error and lands as a Failed row; the queue
  // never stops silently.
  type SealPayload =
    | { kind: 'file'; file: File; docTitle: string | undefined }
    | { kind: 'text'; docTitle: string | undefined; text: string }
  interface SealItem extends ProgressItemView { payload: SealPayload }
  const [sealItems, setSealItemsState] = useState<SealItem[]>([])
  const sealItemsRef = useRef<SealItem[]>([])
  const sealKeyRef = useRef(0)
  const sealProcessingRef = useRef(false)
  const [sealBatchLabel, setSealBatchLabel] = useState<string | null>(null)
  const setSealItems = (next: SealItem[]) => {
    sealItemsRef.current = next
    setSealItemsState(next)
  }
  const patchSealItem = (key: number, patch: Partial<SealItem>) =>
    setSealItems(sealItemsRef.current.map((i) => (i.key === key ? { ...i, ...patch } : i)))
  const processSealQueue = async () => {
    if (sealProcessingRef.current) return
    sealProcessingRef.current = true
    try {
      for (;;) {
        const item = sealItemsRef.current.find((i) => i.status === 'queued')
        if (!item) break
        const p = item.payload
        try {
          let meta: PrivateDocMeta
          if (p.kind === 'file') {
            patchSealItem(item.key, { status: 'uploading', progress: null, startedAt: Date.now() })
            meta = await privateVaultApi.uploadFile(
              p.docTitle,
              p.file,
              (fraction) => patchSealItem(item.key, { status: 'uploading', progress: fraction }),
              () => patchSealItem(item.key, { status: 'sealing', progress: null }),
            )
          } else {
            patchSealItem(item.key, { status: 'sealing', progress: null, startedAt: Date.now() })
            meta = await privateVaultApi.upload({
              title: p.docTitle,
              file_name: 'note.txt',
              text: p.text,
            })
          }
          patchSealItem(item.key, {
            status: 'ready', progress: 1, note: `${meta.chunks} chunks sealed`,
          })
          refresh()
          toast('success', `Sealed into your vault — ${meta.chunks} chunks, fingerprint ${meta.sha256.slice(0, 12)}…`)
        } catch (e) {
          patchSealItem(item.key, { status: 'failed', error: (e as Error).message })
          toast('error', (e as Error).message)
        }
      }
    } finally {
      sealProcessingRef.current = false
    }
  }
  // Title rule: a batch file's document title is its own name
  // (minus extension) — one Title field cannot name many documents.
  // A LONE file picked while the Title field is filled uses that
  // title instead (the field's long-standing role from the
  // single-pick flow), and the field is consumed. Pasted text uses
  // the Title field, or stays untitled (named by file name).
  const enqueueSealFiles = (files: { file: File; relPath: string }[]) => {
    if (files.length === 0) return
    const loneTitle = files.length === 1 && title.trim() ? title.trim() : undefined
    if (loneTitle) setTitle('')
    const items: SealItem[] = applyBatchCaps(files).map(({ file, relPath, skipReason }) => ({
      key: ++sealKeyRef.current,
      name: relPath || file.name,
      status: skipReason ? ('skipped' as const) : ('queued' as const),
      error: skipReason,
      payload: {
        kind: 'file' as const,
        file,
        docTitle: loneTitle ?? file.name.replace(/\.[^.]+$/, ''),
      },
    }))
    setSealBatchLabel(batchLabelFor(files))
    setSealItems([...sealItemsRef.current, ...items])
    void processSealQueue()
  }
  const enqueueSealText = () => {
    if (!text.trim()) return
    const item: SealItem = {
      key: ++sealKeyRef.current,
      name: title.trim() || 'Pasted document',
      status: 'queued',
      payload: { kind: 'text', docTitle: title.trim() || undefined, text },
    }
    setTitle('')
    setText('')
    setSealItems([...sealItemsRef.current, item])
    void processSealQueue()
  }
  const remove = useMutation({
    mutationFn: (id: string) => privateVaultApi.remove(id),
    onSuccess: () => { refresh(); toast('success', 'Private document destroyed') },
    onError: onErr,
  })

  const st = status.data

  return (
    <div className="max-w-3xl mx-auto px-6 py-10">
      <div className="mb-8">
        <h1 className="font-display text-[2rem] tracking-tight">Confidential Vault</h1>
        <p className="text-stone-500 mt-1.5 text-[15px] max-w-xl">
          Your most sensitive documents, sealed under your own password. Stored only as
          ciphertext — invisible in the database, the corpus, and every admin view. The AI can
          work on them only while you unlock the vault, and only for you.
        </p>
        {st?.routing && (
          <p className="text-meta2 text-stone-500 mt-3">
            Private traffic: embeddings via{' '}
            <span className="font-medium text-stone-700">{st.routing.embeddings === 'local' ? 'a local endpoint' : 'the main provider'}</span>
            {' · '}answers via{' '}
            <span className="font-medium text-stone-700">{st.routing.llm === 'local' ? 'a local endpoint' : 'the main provider'}</span>
            {st.routing.embeddings !== 'local' || st.routing.llm !== 'local'
              ? ' — while you ask about sealed documents, the question and the decrypted passages are processed by those providers.'
              : ' — sealed content never leaves this deployment.'}
          </p>
        )}
      </div>

      {status.isLoading || !st ? (
        <Spinner label="Checking your vault…" />
      ) : !st.has_password ? (
        <section className="rounded-lex-lg border border-stone-200 bg-white shadow-lex-sm p-6">
          <div className="flex items-center gap-2.5">
            <ShieldCheck size={18} className="text-stone-500" />
            <h2 className="font-display text-lg">Set your vault password</h2>
          </div>
          <p className="text-[13.5px] text-stone-500 mt-2 leading-relaxed">
            This password is separate from your login. It derives the encryption key for everything
            you seal here — it is never stored, and we cannot recover it for you. Lose it, and the
            sealed documents cannot be opened again.
          </p>
          <div className="flex gap-2 mt-4">
            <input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="At least 8 characters"
              className="flex-1 rounded-lex border border-stone-200 bg-ivory px-3 py-2 text-sm outline-none focus:border-stone-400"
            />
            <button
              onClick={() => setPw.mutate()}
              disabled={password.length < 8 || setPw.isPending}
              className="px-4 py-2 rounded-full bg-ink text-ivory text-sm font-medium hover:bg-ink-900 disabled:opacity-40 transition-colors"
            >
              {setPw.isPending ? 'Setting…' : 'Set password & open vault'}
            </button>
          </div>
        </section>
      ) : !st.unlocked ? (
        <section className="rounded-lex-lg border border-stone-200 bg-white shadow-lex-sm p-6">
          <div className="flex items-center gap-2.5">
            <Lock size={18} className="text-stone-500" />
            <h2 className="font-display text-lg">Vault locked</h2>
          </div>
          <p className="text-[13.5px] text-stone-500 mt-2">
            {st.documents} sealed document{st.documents === 1 ? '' : 's'} inside. While locked, the key
            exists nowhere — not in memory, not on disk — and the AI cannot read them.
          </p>
          <div className="flex gap-2 mt-4">
            <input
              type="password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              onKeyDown={(e) => { if (e.key === 'Enter') unlock.mutate() }}
              placeholder="Vault password"
              className="flex-1 rounded-lex border border-stone-200 bg-ivory px-3 py-2 text-sm outline-none focus:border-stone-400"
            />
            <button
              onClick={() => unlock.mutate()}
              disabled={!password || unlock.isPending}
              className="px-4 py-2 rounded-full bg-ink text-ivory text-sm font-medium hover:bg-ink-900 disabled:opacity-40 transition-colors"
            >
              {unlock.isPending ? 'Unlocking…' : 'Unlock'}
            </button>
          </div>
        </section>
      ) : (
        <>
          <div className="flex items-center justify-between mb-5">
            <p className="inline-flex items-center gap-2 text-[13px] text-stone-500">
              <LockOpen size={14} className="text-emerald-700" /> Unlocked for this session —{' '}
              {st.documents} sealed document{st.documents === 1 ? '' : 's'}. In Ask, switch on{' '}
              <span className="font-medium text-ink">Private</span> to include them in answers.
            </p>
            <div className="flex gap-2 shrink-0">
              <button
                onClick={() => setShowChange((v) => !v)}
                className="inline-flex items-center gap-1.5 rounded-full border border-stone-200 px-3 py-1.5 text-[12.5px] text-stone-600 hover:text-ink hover:border-stone-300 transition-colors"
              >
                <KeyRound size={13} /> Change password
              </button>
              <button
                onClick={() => lock.mutate()}
                className="inline-flex items-center gap-1.5 rounded-full border border-stone-200 px-3 py-1.5 text-[12.5px] text-stone-600 hover:text-ink hover:border-stone-300 transition-colors"
              >
                <Lock size={13} /> Lock now
              </button>
            </div>
          </div>

          {showChange && (
            <section className="rounded-lex-lg border border-stone-200 bg-white shadow-lex-sm p-5 mb-6">
              <h3 className="font-display text-[15px]">Change vault password</h3>
              <p className="text-[13px] text-stone-500 mt-1">Every sealed document is re-encrypted under the new key.</p>
              <div className="flex flex-col sm:flex-row gap-2 mt-3">
                <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} placeholder="Current password" className="flex-1 rounded-lex border border-stone-200 bg-ivory px-3 py-2 text-sm outline-none focus:border-stone-400" />
                <input type="password" value={newPassword} onChange={(e) => setNewPassword(e.target.value)} placeholder="New password (8+ characters)" className="flex-1 rounded-lex border border-stone-200 bg-ivory px-3 py-2 text-sm outline-none focus:border-stone-400" />
                <button onClick={() => change.mutate()} disabled={!password || newPassword.length < 8 || change.isPending} className="px-4 py-2 rounded-full bg-ink text-ivory text-sm font-medium hover:bg-ink-900 disabled:opacity-40 transition-colors">
                  {change.isPending ? 'Re-encrypting…' : 'Change'}
                </button>
              </div>
            </section>
          )}

          <section className="rounded-lex-lg border border-stone-200 bg-white shadow-lex-sm p-5 mb-6">
            <h3 className="font-display text-[15px]">Seal a document</h3>
            <div className="flex items-center gap-3 mt-3 flex-wrap">
              <input
                value={title}
                onChange={(e) => setTitle(e.target.value)}
                placeholder="Title (only you ever see this)"
                className="flex-1 min-w-[220px] rounded-lex border border-stone-200 bg-ivory px-3 py-2 text-sm outline-none focus:border-stone-400"
              />
              <label className="rounded-full border border-stone-200 px-3.5 py-1.5 text-[13px] text-stone-600 hover:text-ink hover:border-stone-300 transition-colors cursor-pointer">
                Choose files…
                <input
                  ref={fileRef} type="file" multiple accept=".pdf,.txt,.md,.docx" className="hidden"
                  onChange={(e) => {
                    const files = Array.from(e.target.files ?? []).map((file) => ({ file, relPath: '' }))
                    e.target.value = ''
                    enqueueSealFiles(files)
                  }}
                />
              </label>
              <label className="rounded-full border border-stone-200 px-3.5 py-1.5 text-[13px] text-stone-600 hover:text-ink hover:border-stone-300 transition-colors cursor-pointer inline-flex items-center gap-1.5">
                <FolderUp size={13} /> Choose folder…
                <input
                  ref={folderRef} type="file" accept=".pdf,.txt,.md,.docx" className="hidden"
                  {...({ webkitdirectory: '' } as Record<string, string>)}
                  onChange={(e) => {
                    const files = Array.from(e.target.files ?? []).map((file) => ({
                      file,
                      relPath: file.webkitRelativePath || '',
                    }))
                    e.target.value = ''
                    enqueueSealFiles(files)
                  }}
                />
              </label>
              <span className="text-[12.5px] text-stone-400">
                up to {MAX_BATCH_FILES} files and {MAX_BATCH_FOLDERS} folders per batch
              </span>
            </div>
            <textarea
              value={text}
              onChange={(e) => setText(e.target.value)}
              placeholder="…or paste confidential text"
              rows={5}
              className="mt-3 w-full rounded-lex border border-stone-200 bg-ivory px-3 py-2 text-sm outline-none focus:border-stone-400 resize-y"
            />
            <div className="mt-3 flex justify-end">
              <button
                onClick={enqueueSealText}
                disabled={!text.trim()}
                className="px-4 py-2 rounded-full bg-ink text-ivory text-sm font-medium hover:bg-ink-900 disabled:opacity-40 transition-colors"
              >
                Seal pasted text
              </button>
            </div>
            {sealItems.length > 0 && (
              <UploadProgressList
                items={sealItems}
                headerLabel={sealBatchLabel ?? undefined}
                onClear={() => { setSealItems([]); setSealBatchLabel(null) }}
                onRetry={(key) => {
                  patchSealItem(key, { status: 'queued', error: undefined, progress: null })
                  void processSealQueue()
                }}
                onDismiss={(key) => setSealItems(sealItemsRef.current.filter((i) => i.key !== key))}
              />
            )}
          </section>

          {docs.isLoading ? (
            <Spinner label="Opening your vault…" />
          ) : (docs.data ?? []).length === 0 ? (
            <EmptyState title="Nothing sealed yet" hint="Documents you seal appear here — and nowhere else in the product." />
          ) : (
            <div className="space-y-2.5">
              {(docs.data ?? []).map((d: PrivateDocMeta) => (
                <div key={d.id} className="rounded-lex-lg border border-stone-200 bg-white px-4 py-3 shadow-lex-sm flex items-center gap-3">
                  <Lock size={15} className="text-stone-400 shrink-0" />
                  <div className="flex-1 min-w-0">
                    <p className="font-medium text-[14px] truncate">{d.title}</p>
                    <p className="text-[12px] text-stone-400 inline-flex items-center gap-1.5 mt-0.5">
                      <Fingerprint size={11} /> {d.sha256.slice(0, 16)}… · {d.chunks} chunks · sealed {new Date(d.created_at).toLocaleDateString('en-IN', { day: 'numeric', month: 'short', year: 'numeric' })}
                    </p>
                  </div>
                  <button
                    onClick={() => { if (window.confirm(`Destroy “${d.title}” permanently? This cannot be undone.`)) remove.mutate(d.id) }}
                    className="p-1.5 text-stone-400 hover:text-[#a02c2c] transition-colors"
                    title="Destroy document"
                  >
                    <Trash2 size={15} />
                  </button>
                </div>
              ))}
            </div>
          )}
        </>
      )}
    </div>
  )
}
