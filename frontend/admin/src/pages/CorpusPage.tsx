import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Database, FileText, Trash2 } from 'lucide-react'
import { adminApi, errMsg } from '../api'
import { ErrorBanner, PageHead, Spinner } from '../ui'

export default function CorpusPage() {
  const qc = useQueryClient()
  const [prefix, setPrefix] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const { data: docs, isLoading } = useQuery({ queryKey: ['docs'], queryFn: adminApi.docs })
  const { data: stats } = useQuery({ queryKey: ['stats'], queryFn: adminApi.stats })

  const ingest = useMutation({
    mutationFn: () => adminApi.ingestS3(prefix || undefined),
    onSuccess: (r) => {
      setNote(`Ingested ${r.documents} document(s) — ${r.chunks} chunks, ${r.entities} entities.`)
      setError(null)
      qc.invalidateQueries({ queryKey: ['docs'] })
      qc.invalidateQueries({ queryKey: ['stats'] })
    },
    onError: (e) => setError(errMsg(e)),
  })
  const del = useMutation({
    mutationFn: (id: string) => adminApi.deleteDoc(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['docs'] })
      qc.invalidateQueries({ queryKey: ['stats'] })
    },
    onError: (e) => setError(errMsg(e)),
  })

  return (
    <div className="max-w-5xl mx-auto px-6 py-10">
      <PageHead title="Corpus & Vault" sub="Everything the assistant can cite. Manage indexed documents and run bucket ingestion." />
      {error && <div className="mt-5"><ErrorBanner message={error} /></div>}
      {note && (
        <div className="mt-5 rounded-lex border border-green-200 bg-green-50 px-4 py-3 text-[13px] text-green-800">{note}</div>
      )}

      <div className="grid grid-cols-3 gap-3 mt-6">
        {[
          ['Documents', stats?.documents],
          ['Chunks', stats?.chunks],
          ['Entities', stats?.entities],
        ].map(([k, v]) => (
          <div key={k as string} className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-5 py-4">
            <p className="text-[12px] uppercase tracking-wider text-stone-500 font-medium">{k}</p>
            <p className="font-display text-3xl mt-1 tracking-tight">{v ?? '—'}</p>
          </div>
        ))}
      </div>

      <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex p-5 mt-4">
        <h3 className="flex items-center gap-2 font-medium text-[15px]"><Database size={16} /> S3 / R2 bucket ingest</h3>
        <p className="text-[13px] text-stone-500 mt-2 leading-relaxed">
          Pulls every supported file under the prefix from the server-configured bucket
          (AWS S3 or Cloudflare R2 via endpoint URL). Parsing includes OCR for scanned pages.
        </p>
        <div className="flex gap-2 mt-3">
          <input
            value={prefix} onChange={(e) => setPrefix(e.target.value)}
            placeholder="Prefix (e.g. acts/income-tax/)"
            className="flex-1 rounded-lex border border-stone-300 px-3 py-2 text-[13px] outline-none focus:border-ink placeholder:text-stone-400"
          />
          <button
            onClick={() => ingest.mutate()} disabled={ingest.isPending}
            className="rounded-lex bg-ink text-ivory px-4 py-2 text-[13px] font-medium hover:bg-ink-900 disabled:opacity-50"
          >
            {ingest.isPending ? 'Ingesting…' : 'Run ingest'}
          </button>
        </div>
      </div>

      <h2 className="font-display text-xl tracking-tight mt-10 mb-3">Indexed documents</h2>
      {isLoading ? (
        <Spinner />
      ) : !docs || docs.length === 0 ? (
        <p className="text-[15px] text-stone-500">No documents indexed yet.</p>
      ) : (
        <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex overflow-hidden">
          {docs.map((d, i) => (
            <div key={d.id} className={`flex items-center gap-4 px-5 py-3.5 ${i > 0 ? 'border-t border-stone-200' : ''}`}>
              <FileText size={17} className="text-stone-400 flex-shrink-0" />
              <div className="min-w-0 flex-1">
                <p className="text-[14px] font-medium truncate">{d.title}</p>
                <p className="text-[12px] text-stone-500 truncate">{d.source_uri}</p>
              </div>
              <span className="text-[12px] text-stone-500 flex-shrink-0">{d.chunks} chunks · {d.chars.toLocaleString()} chars</span>
              <button
                onClick={() => { if (confirm(`Remove "${d.title}" from the index?`)) del.mutate(d.id) }}
                className="p-2 rounded-lex text-stone-400 hover:text-[#a02c2c] hover:bg-[#feeaea] transition-colors"
                title="Remove from index"
              >
                <Trash2 size={15} />
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
