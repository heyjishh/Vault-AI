import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Download, Plug, RefreshCw, Search, Unplug } from 'lucide-react'
import { connectorsApi, type ConnectorStatus, type RemoteFile } from '../api/client'
import { useSession } from '../stores/session'
import { Spinner } from './ui'

/** Piece 70: the Connectors hub — connect Google Drive, OneDrive
 *  or Dropbox once, then browse and import documents straight
 *  into the Vault. SharePoint rides the Microsoft connection;
 *  NetDocuments is partner-only and says so. */
export default function ConnectorsPage() {
  const { data, isLoading } = useQuery({ queryKey: ['connectors'], queryFn: connectorsApi.list })
  return (
    <div className="w-full px-6 py-8">
      <h1 className="font-display text-[2rem] tracking-tight flex items-center gap-2.5">
        <Plug size={24} /> Connectors
      </h1>
      <p className="text-stone-500 dark:text-stone-400 mt-1.5 text-[15px] max-w-2xl">
        Connected to everything that matters. Link a document source once, then import files
        straight into your Vault — indexed and citable like any upload.
      </p>
      {isLoading && <Spinner label="Loading connectors…" />}
      <div className="grid md:grid-cols-2 xl:grid-cols-3 gap-3 mt-6">
        {(data ?? []).map((c) => <ConnectorCard key={c.id} c={c} />)}
      </div>
    </div>
  )
}

function ConnectorCard({ c }: { c: ConnectorStatus }) {
  const qc = useQueryClient()
  const toast = useSession((s) => s.toast)
  const [connecting, setConnecting] = useState(false)

  const disconnectMut = useMutation({
    mutationFn: () => connectorsApi.disconnect(c.id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['connectors'] })
      qc.invalidateQueries({ queryKey: ['connector-files', c.id] })
      toast('success', `${c.name} disconnected.`)
    },
    onError: (e) => toast('error', e instanceof Error ? e.message : 'Could not disconnect.'),
  })

  async function connect() {
    setConnecting(true)
    try {
      const url = await connectorsApi.authUrl(c.id)
      window.location.href = url
    } catch (e) {
      toast('error', e instanceof Error ? e.message : 'Could not start the connection.')
      setConnecting(false)
    }
  }

  return (
    <div className="rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] px-5 py-4 flex flex-col">
      <div className="flex items-center gap-3">
        <span className="w-9 h-9 rounded-lex bg-ink text-ivory flex items-center justify-center font-display text-[15px] shrink-0">
          {c.name.charAt(0)}
        </span>
        <p className="font-display text-[17px]">{c.name}</p>
        <span className={`ml-auto text-[11px] font-medium rounded-full px-2 py-0.5 ${
          c.connected
            ? 'bg-[#e6f4ea] text-[#2f6b2f] dark:bg-[#12351c] dark:text-[#9fd3a8]'
            : 'bg-stone-100 dark:bg-white/10 text-stone-500 dark:text-stone-400'
        }`}>
          {c.connected ? 'Connected' : c.note ? 'Partner' : 'Not connected'}
        </span>
      </div>
      <p className="text-[13px] text-stone-500 dark:text-stone-400 mt-2 leading-relaxed flex-1">
        {c.blurb}
        {c.connected && c.account_email && <> Connected as <span className="text-ink dark:text-[#f4f2ee] font-medium">{c.account_email}</span>.</>}
      </p>
      {c.note && (
        <p className="text-[12px] text-stone-400 dark:text-stone-500 mt-2 leading-relaxed">{c.note}</p>
      )}
      {!c.connected && !c.note && !c.configured && (
        <p className="text-[12px] text-[#7a5b12] dark:text-[#e3c878] mt-2 leading-relaxed">
          Setup required: the operator must register an OAuth app for {c.name} and set its client
          id and secret on the server before anyone can connect.
        </p>
      )}
      <div className="flex gap-2 mt-4">
        {!c.connected && !c.note && (
          <button
            onClick={() => void connect()}
            disabled={connecting}
            className="btn-brand flex items-center gap-1.5 rounded-lex text-ivory px-3.5 py-2 text-[13px] font-medium disabled:opacity-40"
          >
            <Plug size={13} /> {connecting ? 'Opening…' : `Connect ${c.name}`}
          </button>
        )}
        {c.connected && (
          <button
            onClick={() => disconnectMut.mutate()}
            disabled={disconnectMut.isPending}
            className="flex items-center gap-1.5 rounded-lex border border-stone-200 dark:border-white/10 px-3.5 py-2 text-[13px] font-medium text-stone-600 dark:text-stone-300 hover:text-[#a02c2c] hover:border-stone-300 transition-colors disabled:opacity-40"
          >
            <Unplug size={13} /> Disconnect
          </button>
        )}
      </div>
      {c.connected && <FileBrowser provider={c.id} />}
    </div>
  )
}

function FileBrowser({ provider }: { provider: string }) {
  const qc = useQueryClient()
  const toast = useSession((s) => s.toast)
  const [q, setQ] = useState('')
  const [search, setSearch] = useState('')
  const files = useQuery({
    queryKey: ['connector-files', provider, search],
    queryFn: () => connectorsApi.files(provider, search),
  })
  const [importingId, setImportingId] = useState<string | null>(null)

  async function doImport(f: RemoteFile) {
    setImportingId(f.id)
    try {
      const r = await connectorsApi.importFile(provider, f)
      qc.invalidateQueries({ queryKey: ['docs'] })
      qc.invalidateQueries({ queryKey: ['dashboard'] })
      toast('success', `Imported “${f.name}” into the Vault (${r.chunks} chunks indexed).`)
    } catch (e) {
      toast('error', e instanceof Error ? e.message : 'Import failed.')
    } finally {
      setImportingId(null)
    }
  }

  return (
    <div className="mt-4 border-t border-stone-100 dark:border-white/10 pt-3">
      <form
        className="flex items-center gap-2"
        onSubmit={(e) => { e.preventDefault(); setSearch(q.trim()) }}
      >
        <div className="relative flex-1">
          <Search size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-stone-400" />
          <input
            value={q} onChange={(e) => setQ(e.target.value)}
            placeholder="Search files to import…"
            className="w-full rounded-lex border border-stone-300 dark:border-white/10 bg-white dark:bg-[#0a1a12] pl-7 pr-3 py-1.5 text-[13px] outline-none focus:border-ink placeholder:text-stone-400"
          />
        </div>
        <button type="submit" className="rounded-lex border border-stone-200 dark:border-white/10 px-3 py-1.5 text-[12.5px] font-medium hover:border-stone-300 transition-colors">
          Search
        </button>
        <button
          type="button"
          onClick={() => files.refetch()}
          title="Refresh"
          className="p-1.5 rounded-lex text-stone-400 hover:text-ink transition-colors"
        >
          <RefreshCw size={13} />
        </button>
      </form>
      <div className="mt-2 max-h-64 overflow-y-auto divide-y divide-stone-100 dark:divide-white/5">
        {files.isLoading && <p className="py-4 text-[12.5px] text-stone-400">Loading files…</p>}
        {files.isError && (
          <p className="py-4 text-[12.5px] text-[#a02c2c]">
            Could not list files: {files.error instanceof Error ? files.error.message : 'unknown error'}
          </p>
        )}
        {files.data && files.data.length === 0 && (
          <p className="py-4 text-[12.5px] text-stone-400">No files found.</p>
        )}
        {(files.data ?? []).map((f) => (
          <div key={f.id} className="flex items-center gap-2.5 py-2">
            <div className="min-w-0 flex-1">
              <p className="text-[13px] font-medium truncate" title={f.name}>{f.name}</p>
              <p className="text-[11px] text-stone-400 dark:text-stone-500">
                {f.size != null ? `${(f.size / 1024).toFixed(0)} KB` : '—'}
                {f.modified ? ` · ${new Date(f.modified).toLocaleDateString('en-IN', { day: 'numeric', month: 'short', year: 'numeric' })}` : ''}
              </p>
            </div>
            <button
              onClick={() => void doImport(f)}
              disabled={importingId === f.id}
              className="shrink-0 flex items-center gap-1.5 rounded-lex border border-stone-200 dark:border-white/10 px-2.5 py-1.5 text-[12px] font-medium text-stone-600 dark:text-stone-300 hover:text-ink hover:border-stone-300 transition-colors disabled:opacity-40"
            >
              <Download size={12} /> {importingId === f.id ? 'Importing…' : 'Import'}
            </button>
          </div>
        ))}
      </div>
    </div>
  )
}
