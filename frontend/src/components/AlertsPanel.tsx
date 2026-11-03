import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { BellPlus, CheckCheck, ExternalLink, RefreshCw, Trash2 } from 'lucide-react'
import { alertsApi } from '../api/client'
import type { WatchView } from '../api/types'
import { EmptyState, Spinner } from './ui'

/**
 * Workspace alerts (Piece 79): case/query watches on this workspace.
 * A watch re-checks the vault and the web every few hours; the first
 * check only sets the baseline, so hits are always material that is
 * new since the watch started doing its job.
 */
export default function AlertsPanel({ matterId }: { matterId: string }) {
  const qc = useQueryClient()
  const [kind, setKind] = useState<'case' | 'query'>('case')
  const [text, setText] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)

  const list = useQuery({
    queryKey: ['alerts', matterId],
    queryFn: () => alertsApi.list(matterId),
  })
  const invalidate = () => qc.invalidateQueries({ queryKey: ['alerts', matterId] })

  const create = useMutation({
    mutationFn: () => alertsApi.create(matterId, kind, text.trim()),
    onSuccess: () => {
      setText('')
      setError(null)
      setNotice('Watch added. Run “Check now” once to set its baseline — everything already in the vault and on the web counts as seen, and only new material after that becomes a hit.')
      invalidate()
    },
    onError: (e) => setError(e instanceof Error ? e.message : 'Could not add the watch'),
  })

  return (
    <div>
      <p className="text-[14px] text-stone-500 dark:text-stone-400 max-w-3xl">
        Watch a case or a legal question inside this workspace. The watch re-checks the vault and the web
        every few hours and collects anything new it finds here — new filings and orders in a case, fresh
        material on a question you are tracking.
      </p>

      <div className="mt-4 flex flex-wrap items-end gap-2">
        <label className="flex flex-col gap-1 text-[12px] text-stone-500 dark:text-stone-400">
          Watch type
          <select
            value={kind}
            onChange={(e) => setKind(e.target.value as 'case' | 'query')}
            className="rounded-lg border border-stone-300 dark:border-white/15 bg-transparent px-2.5 py-2 text-[14px] text-ink dark:text-[#f4f2ee]"
          >
            <option value="case">Case</option>
            <option value="query">Query</option>
          </select>
        </label>
        <label className="flex flex-1 min-w-[240px] flex-col gap-1 text-[12px] text-stone-500 dark:text-stone-400">
          {kind === 'case' ? 'Case name or citation' : 'Question to watch'}
          <input
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder={kind === 'case' ? 'e.g. Sharma v. State of Delhi, CRL.A. 123/2024' : 'e.g. Is a WhatsApp summons valid service under the BNSS?'}
            className="rounded-lg border border-stone-300 dark:border-white/15 bg-transparent px-3 py-2 text-[14px] text-ink dark:text-[#f4f2ee]"
          />
        </label>
        <button
          onClick={() => text.trim() && create.mutate()}
          disabled={!text.trim() || create.isPending}
          className="btn-brand inline-flex items-center gap-1.5 rounded-lg px-3.5 py-2 text-[14px] font-medium disabled:opacity-50"
        >
          <BellPlus size={15} /> {create.isPending ? 'Adding…' : 'Add watch'}
        </button>
      </div>
      {error && <p className="mt-2 text-[13px] text-red-600 dark:text-red-400">{error}</p>}
      {notice && <p className="mt-2 text-[13px] text-stone-500 dark:text-stone-400">{notice}</p>}

      <div className="mt-6">
        {list.isLoading ? (
          <Spinner label="Loading watches…" />
        ) : !list.data || list.data.watches.length === 0 ? (
          <EmptyState title="No watches yet" />
        ) : (
          <div className="flex flex-col gap-4">
            {list.data.watches.map((w) => (
              <WatchCard key={w.id} watch={w} matterId={matterId} onChanged={invalidate} />
            ))}
          </div>
        )}
      </div>
    </div>
  )
}

function WatchCard({ watch: w, onChanged }: { watch: WatchView; matterId: string; onChanged: () => void }) {
  const [confirming, setConfirming] = useState(false)
  const [result, setResult] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const check = useMutation({
    mutationFn: () => alertsApi.check(w.id),
    onSuccess: (r) => {
      setError(null)
      if (r.legs_failed.length === 2) {
        setResult('The check could not run — both the vault and web legs failed. The baseline is unchanged; try again shortly.')
      } else if (r.baseline) {
        setResult('Baseline set. Material that appears from now on will land here as hits.')
      } else if (r.new_hits > 0) {
        setResult(`${r.new_hits} new ${r.new_hits === 1 ? 'hit' : 'hits'} found.`)
      } else {
        setResult('Checked — nothing new.')
      }
      if (r.legs_failed.length === 1) {
        setResult((prev) => `${prev ?? ''} (The ${r.legs_failed[0]} leg failed this run.)`)
      }
      onChanged()
    },
    onError: (e) => setError(e instanceof Error ? e.message : 'Check failed'),
  })
  const markRead = useMutation({
    mutationFn: () => alertsApi.read(w.id),
    onSuccess: onChanged,
  })
  const remove = useMutation({
    mutationFn: () => alertsApi.remove(w.id),
    onSuccess: onChanged,
  })

  return (
    <div className="rounded-xl border border-stone-200 dark:border-white/10 p-4">
      <div className="flex flex-wrap items-start gap-2">
        <span className="rounded-full bg-stone-100 dark:bg-white/10 px-2 py-0.5 text-[11px] font-medium uppercase tracking-wide text-stone-600 dark:text-stone-300">
          {w.kind === 'case' ? 'Case watch' : 'Query watch'}
        </span>
        {w.unread > 0 && (
          <span className="rounded-full bg-emerald-100 dark:bg-emerald-500/15 px-2 py-0.5 text-[11px] font-medium text-emerald-700 dark:text-emerald-300">
            {w.unread} new
          </span>
        )}
        <div className="min-w-0 flex-1">
          <p className="text-[15px] font-medium leading-snug">{w.text}</p>
          <p className="mt-0.5 text-[12px] text-stone-500 dark:text-stone-400">
            {w.last_checked_at
              ? `Last checked ${new Date(w.last_checked_at).toLocaleString('en-IN', { day: 'numeric', month: 'short', hour: 'numeric', minute: '2-digit' })}`
              : 'Not checked yet — the first check sets the baseline'}
          </p>
        </div>
        <div className="flex items-center gap-1.5">
          <button
            onClick={() => check.mutate()}
            disabled={check.isPending}
            className="inline-flex items-center gap-1 rounded-lg border border-stone-300 dark:border-white/15 px-2.5 py-1.5 text-[13px] hover:bg-stone-50 dark:hover:bg-white/5 disabled:opacity-50"
          >
            <RefreshCw size={13} /> {check.isPending ? 'Checking…' : 'Check now'}
          </button>
          {w.unread > 0 && (
            <button
              onClick={() => markRead.mutate()}
              disabled={markRead.isPending}
              className="inline-flex items-center gap-1 rounded-lg border border-stone-300 dark:border-white/15 px-2.5 py-1.5 text-[13px] hover:bg-stone-50 dark:hover:bg-white/5 disabled:opacity-50"
            >
              <CheckCheck size={13} /> Mark read
            </button>
          )}
          {confirming ? (
            <>
              <button
                onClick={() => remove.mutate()}
                disabled={remove.isPending}
                className="rounded-lg bg-red-600 px-2.5 py-1.5 text-[13px] font-medium text-white disabled:opacity-50"
              >
                {remove.isPending ? 'Deleting…' : 'Confirm delete'}
              </button>
              <button
                onClick={() => setConfirming(false)}
                className="rounded-lg border border-stone-300 dark:border-white/15 px-2.5 py-1.5 text-[13px]"
              >
                Keep
              </button>
            </>
          ) : (
            <button
              onClick={() => setConfirming(true)}
              className="inline-flex items-center gap-1 rounded-lg border border-stone-300 dark:border-white/15 px-2.5 py-1.5 text-[13px] text-stone-500 dark:text-stone-400 hover:text-red-600 disabled:opacity-50"
            >
              <Trash2 size={13} /> Delete
            </button>
          )}
        </div>
      </div>

      {result && <p className="mt-2 text-[13px] text-stone-600 dark:text-stone-300">{result}</p>}
      {error && <p className="mt-2 text-[13px] text-red-600 dark:text-red-400">{error}</p>}

      {w.hits.length > 0 && (
        <div className="mt-3 flex flex-col divide-y divide-stone-100 dark:divide-white/5 border-t border-stone-100 dark:border-white/5">
          {w.hits.map((h) => (
            <div key={h.id} className="py-2.5 flex gap-2.5">
              <span className={`mt-1.5 h-2 w-2 shrink-0 rounded-full ${h.read ? 'bg-stone-300 dark:bg-white/15' : 'bg-emerald-500'}`} />
              <div className="min-w-0">
                <p className="text-[14px] font-medium leading-snug">
                  {h.url ? (
                    <a href={h.url} target="_blank" rel="noreferrer" className="inline-flex items-center gap-1 hover:underline">
                      {h.title} <ExternalLink size={12} className="text-stone-400" />
                    </a>
                  ) : (
                    h.title
                  )}
                  <span className="ml-2 rounded-full bg-stone-100 dark:bg-white/10 px-1.5 py-px text-[10px] font-medium uppercase tracking-wide text-stone-500 dark:text-stone-400">
                    {h.source === 'web' ? 'Web' : 'Vault'}
                  </span>
                </p>
                {h.snippet && <p className="mt-0.5 text-[13px] text-stone-500 dark:text-stone-400 leading-snug">{h.snippet}</p>}
                <p className="mt-0.5 text-[11px] text-stone-400">
                  Found {new Date(h.found_at).toLocaleString('en-IN', { day: 'numeric', month: 'short', hour: 'numeric', minute: '2-digit' })}
                </p>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
