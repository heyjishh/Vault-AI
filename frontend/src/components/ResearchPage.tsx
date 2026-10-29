import { useEffect, useState } from 'react'
import { useMutation } from '@tanstack/react-query'
import { FlaskConical, Globe, Lightbulb, AlertTriangle } from 'lucide-react'
import { ragApi } from '../api/client'
import { useSession } from '../stores/session'
import { RefineControls, useRefine } from './RefineControls'
import type { ResearchReport } from '../api/types'
import { CitationCard, Spinner, StatusBadge } from './ui'
import { CitedText, SourceModal } from './SourceModal'
import type { Citation } from '../api/types'

export default function ResearchPage({ go }: { go?: (view: string) => void } = {}) {
  const [q, setQ] = useState('')
  const [web, setWeb] = useState(false)
  const [report, setReport] = useState<ResearchReport | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [modalCitation, setModalCitation] = useState<Citation | null>(null)

  // History restore: reopen the exact report, no re-run.
  const restore = useSession((st) => st.restore)
  const consumeRestore = useSession((st) => st.consumeRestore)
  useEffect(() => {
    if (!restore || restore.kind !== 'research') return
    const p = consumeRestore('research')
    if (!p) return
    setReport(p.report as ResearchReport)
    setQ(String(p.question ?? ''))
    setError(null)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [restore])

  // Prompt refinement (Piece 13) for the research question.
  const refineCtl = useRefine({ value: q, setValue: setQ, surface: 'research' })

  const run = useMutation({
    mutationFn: () => ragApi.research(q.trim(), web, refineCtl.consumeTerms()),
    onSuccess: (r) => { setReport(r); setError(null) },
    onError: (e) => setError((e as Error).message),
  })

  return (
    <div className="max-w-3xl mx-auto px-6 py-10">
      <h1 className="font-display text-[2rem] tracking-tight flex items-center gap-3">
        <FlaskConical className="text-stone-500" /> Research
      </h1>
      <p className="text-stone-500 mt-1.5 text-[15px]">
        Deep research runs an epistemic loop: frame hypotheses, gather cited evidence for each, then adjudicate.
      </p>

      <div className="mt-6 rounded-lex-lg border border-stone-300 bg-white shadow-lex-md p-4">
        <textarea
          value={q} onChange={(e) => setQ(e.target.value)} rows={3}
          placeholder="State the legal question to investigate…"
          className="w-full resize-none text-[15px] outline-none placeholder:text-stone-400"
          onKeyDown={(e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); if (q.trim()) run.mutate() } }}
        />
        <div className="flex items-center justify-between mt-2">
          <div className="flex items-center gap-2">
            <button
              onClick={() => setWeb(!web)}
              className={`flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-meta transition-colors ${
                web ? 'bg-ink text-ivory border-ink' : 'border-stone-200 text-stone-500 hover:text-ink'
              }`}
            >
              <Globe size={13} /> Include web evidence
            </button>
            <RefineControls ctl={refineCtl} disabled={!q.trim()} />
          </div>
          <button
            onClick={() => q.trim() && run.mutate()} disabled={run.isPending || !q.trim()}
            className="rounded-lex bg-ink text-ivory px-4 py-2 text-meta2 font-medium hover:bg-ink-900 disabled:opacity-40"
          >
            {run.isPending ? 'Researching…' : 'Run research'}
          </button>
        </div>
      </div>

      {error && (
        <div className="mt-5 rounded-lex border border-brand-red/30 bg-[#feeaea] px-4 py-3 text-meta2 text-[#a02c2c]">{error}</div>
      )}
      {run.isPending && <Spinner label="Framing hypotheses and gathering evidence — this takes a little while…" />}

      {report && !run.isPending && (
        <div className="mt-8 fade-up">
          <h2 className="font-display text-2xl tracking-tight">{report.question}</h2>
          <div className="mt-4 rounded-lex-lg border border-stone-200 bg-white shadow-lex p-5">
            <p className="text-meta uppercase tracking-wider text-stone-400 font-medium">Conclusion</p>
            <p className="text-[15px] leading-relaxed mt-2 whitespace-pre-wrap">
              <CitedText text={report.conclusion} citations={report.citations} onOpen={setModalCitation} />
            </p>
          </div>

          <h3 className="font-display text-xl tracking-tight mt-8 mb-3">Hypotheses</h3>
          <div className="space-y-3">
            {report.hypotheses.map((h, i) => (
              <div key={i} className="rounded-lex-lg border border-stone-200 bg-white shadow-lex p-5">
                <div className="flex items-start justify-between gap-3">
                  <p className="font-medium text-[15px]">{h.question}</p>
                  <StatusBadge status={h.status} />
                </div>
                <p className="text-meta2 text-stone-600 mt-1.5"><span className="font-medium text-ink-800">Prediction:</span> {h.prediction}</p>
                {h.evidence.length > 0 && (
                  <div className="mt-3 space-y-1.5 border-t border-stone-200 pt-3">
                    {h.evidence.map((ev, j) => {
                      const srcId = ev.source.replace(/^\[|\]$/g, '')
                      const srcCite = report.citations.find((x) => x.doc_id === srcId)
                      return (
                        <p key={j} className="text-meta2 text-stone-600 leading-relaxed">
                          <span className="text-stone-400 mr-1.5">—</span>{ev.claim}
                          {srcCite ? (
                            <a
                              href={`#cite-${srcCite.chunk_id}`}
                              onClick={(e) => { e.preventDefault(); setModalCitation(srcCite) }}
                              className="cite-chip ml-1.5"
                              title="Open the full source document"
                            >
                              {srcId}
                            </a>
                          ) : (
                            <span className="text-stone-400 text-meta ml-1.5">[{ev.source}]</span>
                          )}
                        </p>
                      )
                    })}
                  </div>
                )}
              </div>
            ))}
          </div>

          {report.conflicts.length > 0 && (
            <>
              <h3 className="font-display text-xl tracking-tight mt-8 mb-3 flex items-center gap-2">
                <AlertTriangle size={18} className="text-amber-600" /> Conflicts
              </h3>
              <div className="space-y-2">
                {report.conflicts.map((c, i) => (
                  <div key={i} className="rounded-lex border border-amber-200 bg-amber-50 px-4 py-3">
                    <p className="font-medium text-[14px]">{c.topic}</p>
                    <p className="text-meta2 text-stone-700 mt-1">A: {c.position_a}</p>
                    <p className="text-meta2 text-stone-700">B: {c.position_b}</p>
                  </div>
                ))}
              </div>
            </>
          )}

          {report.open_points.length > 0 && (
            <>
              <h3 className="font-display text-xl tracking-tight mt-8 mb-3 flex items-center gap-2">
                <Lightbulb size={18} className="text-stone-500" /> Open points
              </h3>
              <ul className="list-disc pl-5 space-y-1 text-[15px] text-stone-700">
                {report.open_points.map((p, i) => <li key={i}>{p}</li>)}
              </ul>
            </>
          )}

          {report.citations.length > 0 && (
            <div className="mt-8">
              <p className="text-meta uppercase tracking-wider text-stone-400 font-medium mb-2.5">Sources · {report.citations.length}</p>
              <div className="grid gap-2">
                {report.citations.map((c) => <CitationCard key={c.chunk_id} citation={c} go={go} />)}
              </div>
            </div>
          )}
        </div>
      )}
      {modalCitation && (
        <SourceModal citation={modalCitation} onClose={() => setModalCitation(null)} go={go} />
      )}
    </div>
  )
}
