import { useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Fingerprint, KeyRound, Lock, LockOpen, ShieldCheck, Trash2 } from 'lucide-react'
import { privateVaultApi, type PrivateDocMeta } from '../api/client'
import { useSession } from '../stores/session'
import { EmptyState, Spinner } from './ui'
import { UploadProgressList, type ProgressItemView } from './UploadProgress'

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
  const [fileName, setFileName] = useState('')
  const [fileB64, setFileB64] = useState<string | null>(null)
  const fileRef = useRef<HTMLInputElement>(null)

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
  // Piece 17: sealing gets the shared staged progress row (elapsed
  // + inline failure with Retry/Dismiss) instead of a bare
  // "Sealing…" button label. The payload is already in memory, so
  // the single POST is the Indexing stage — the server parsing,
  // encrypting and embedding — and Ready means it confirmed.
  const [sealItem, setSealItem] = useState<ProgressItemView | null>(null)
  const upload = useMutation({
    mutationFn: () =>
      privateVaultApi.upload({
        title: title.trim() || undefined,
        file_name: fileName || 'note.txt',
        text: fileB64 ? undefined : text,
        content_base64: fileB64 ?? undefined,
      }),
    onMutate: () => {
      setSealItem({
        key: Date.now(),
        name: title.trim() || fileName || 'Pasted document',
        status: 'indexing',
        progress: null,
        startedAt: Date.now(),
      })
    },
    onSuccess: (m) => {
      setTitle(''); setText(''); setFileName(''); setFileB64(null)
      if (fileRef.current) fileRef.current.value = ''
      setSealItem((cur) => (cur ? { ...cur, status: 'ready', progress: 1, note: `${m.chunks} chunks sealed` } : cur))
      refresh()
      toast('success', `Sealed into your vault — ${m.chunks} chunks, fingerprint ${m.sha256.slice(0, 12)}…`)
    },
    onError: (e) => {
      onErr(e)
      setSealItem((cur) => (cur ? { ...cur, status: 'failed', error: (e as Error).message } : cur))
    },
  })
  const remove = useMutation({
    mutationFn: (id: string) => privateVaultApi.remove(id),
    onSuccess: () => { refresh(); toast('success', 'Private document destroyed') },
    onError: onErr,
  })

  function pickFile(f: File | undefined) {
    if (!f) return
    setFileName(f.name)
    if (!title) setTitle(f.name.replace(/\.[^.]+$/, ''))
    const reader = new FileReader()
    reader.onload = () => {
      const s = String(reader.result)
      setFileB64(s.slice(s.indexOf(',') + 1))
    }
    reader.readAsDataURL(f)
  }

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
              <input ref={fileRef} type="file" accept=".pdf,.txt,.md,.docx" onChange={(e) => pickFile(e.target.files?.[0])} className="hidden" />
              <button onClick={() => fileRef.current?.click()} className="rounded-full border border-stone-200 px-3.5 py-1.5 text-[13px] text-stone-600 hover:text-ink hover:border-stone-300 transition-colors">
                {fileName ? `File: ${fileName}` : 'Choose file…'}
              </button>
            </div>
            {!fileB64 && (
              <textarea
                value={text}
                onChange={(e) => setText(e.target.value)}
                placeholder="…or paste confidential text"
                rows={5}
                className="mt-3 w-full rounded-lex border border-stone-200 bg-ivory px-3 py-2 text-sm outline-none focus:border-stone-400 resize-y"
              />
            )}
            <div className="mt-3 flex justify-end">
              <button
                onClick={() => upload.mutate()}
                disabled={upload.isPending || (!text.trim() && !fileB64)}
                className="px-4 py-2 rounded-full bg-ink text-ivory text-sm font-medium hover:bg-ink-900 disabled:opacity-40 transition-colors"
              >
                Seal into vault
              </button>
            </div>
            {sealItem && (
              <UploadProgressList
                items={[sealItem]}
                onRetry={() => upload.mutate()}
                onDismiss={() => setSealItem(null)}
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
