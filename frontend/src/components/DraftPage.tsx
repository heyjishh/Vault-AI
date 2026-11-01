import { useEffect, useState } from 'react'
import { useMutation } from '@tanstack/react-query'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { Copy, Download, PenLine } from 'lucide-react'
import { createApi, type DraftResponse } from '../api/client'
import type { Citation } from '../api/types'
import { RefineControls, useRefine } from './RefineControls'
import { PromptPlusButton, promptInsert } from './PromptPicker'
import { CitationCard, EmptyState, Spinner } from './ui'
import { SourceModal, citeMarkdownComponents, linkifyCitations } from './SourceModal'
import { useSession } from '../stores/session'
import { useMatter } from '../stores/matter'

const KINDS = [
  { id: 'legal_notice', label: 'Legal notice', hint: 'Demand / reply notice in Indian practice' },
  { id: 'agreement', label: 'Agreement', hint: 'Parties, obligations, term, dispute resolution' },
  { id: 'memo', label: 'Legal memo (IRAC)', hint: 'Grounded in your vault’s sources' },
  { id: 'petition', label: 'Petition outline', hint: 'Facts, grounds, interim relief, prayer' },
  { id: 'letter', label: 'Advocate’s letter', hint: 'Formal client-position letter' },
] as const

export default function DraftPage({ matterId: fixedMatterId, go }: { matterId?: string; go?: (view: string) => void } = {}) {
  const toast = useSession((s) => s.toast)
  const storeMatterId = useMatter((s) => s.currentMatterId)
  const matterId = fixedMatterId ?? storeMatterId ?? undefined
  const [kind, setKind] = useState<string>('legal_notice')
  const [instructions, setInstructions] = useState('')
  // Piece 68: "Use" on the Templates page seeds the Draft
  // instructions with the template, highlighted in the input.
  useEffect(() => {
    const seed = sessionStorage.getItem('jurisvault.templateSeed')
    if (!seed) return
    sessionStorage.removeItem('jurisvault.templateSeed')
    setInstructions(seed)
    window.setTimeout(() => {
      const el = document.getElementById('draft-input') as HTMLTextAreaElement | null
      if (el) {
        el.focus()
        try { el.setSelectionRange(0, seed.length) } catch { /* noop */ }
      }
    }, 80)
  }, [])
  const [useCorpus, setUseCorpus] = useState(true)
  const [result, setResult] = useState<DraftResponse | null>(null)
  const [modalCitation, setModalCitation] = useState<Citation | null>(null)
  // History restore: reopen the exact draft.
  const restore = useSession((s) => s.restore)
  const consumeRestore = useSession((s) => s.consumeRestore)
  useEffect(() => {
    if (!restore || restore.kind !== 'draft') return
    const p = consumeRestore('draft')
    if (!p) return
    setKind(String(p.kind ?? 'legal_notice'))
    setInstructions(String(p.instructions ?? ''))
    setResult({
      kind: String(p.kind ?? ''),
      draft: String(p.draft ?? ''),
      citations: (p.citations as DraftResponse['citations']) ?? [],
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [restore])

  // Prompt refinement (Piece 13) for the draft instructions.
  const refineCtl = useRefine({ value: instructions, setValue: setInstructions, surface: 'draft' })

  const run = useMutation({
    mutationFn: () => createApi.draft(kind, instructions.trim(), useCorpus, matterId, refineCtl.consumeTerms()),
    onSuccess: (r) => { setResult(r); toast('success', 'Draft ready — review before use.') },
    onError: (e) => toast('error', (e as Error).message),
  })

  const download = () => {
    if (!result) return
    const blob = new Blob([result.draft], { type: 'text/markdown' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `${kind.replace('_', '-')}-draft.md`
    a.click()
    URL.revokeObjectURL(url)
  }

  return (
    <div className="w-full px-6 py-8">
      <h1 className="font-display text-[2rem] tracking-tight">Draft</h1>
      <p className="text-stone-500 dark:text-stone-400 mt-1.5 text-[15px]">
        Generate a structured first draft. Facts you don't supply become [BRACKETED PLACEHOLDERS] — the drafter never invents citations.
      </p>

      <div className="rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] px-6 py-5 mt-6">
        <div className="flex flex-wrap gap-2">
          {KINDS.map((k) => (
            <button
              key={k.id}
              onClick={() => setKind(k.id)}
              className={`rounded-lex px-3.5 py-1.5 text-[13px] border transition-colors ${kind === k.id ? 'bg-ink text-ivory border-ink' : 'border-stone-300 dark:border-white/10 text-stone-600 dark:text-stone-300 hover:border-ink'}`}
              title={k.hint}
            >
              {k.label}
            </button>
          ))}
        </div>
        <div className="running-border mt-4 rounded-lex-lg border border-stone-300 dark:border-white/10 bg-white dark:bg-[#0a1a12]">
          <textarea
            id="draft-input"
            value={instructions}
            onChange={(e) => setInstructions(e.target.value)}
            rows={5}
            placeholder={'Describe what you need. E.g. "Notice to a tenant at 14 MG Road, Bengaluru for 3 months’ unpaid rent of ₹45,000/month, demanding payment within 15 days and vacation on default."'}
            className="w-full resize-none px-4 pt-4 pb-2 text-[15px] leading-relaxed outline-none placeholder:text-stone-400 bg-transparent"
          />
          <div className="flex items-center justify-between px-3 pb-3 pt-1 flex-wrap gap-3">
          <div className="flex items-center gap-3 flex-wrap">
            <PromptPlusButton withTemplates onPick={promptInsert(instructions, setInstructions, 'draft-input')} />
            <label className="flex items-center gap-2 text-[13.5px] text-stone-600 dark:text-stone-300">
              <input type="checkbox" checked={useCorpus} onChange={(e) => setUseCorpus(e.target.checked)} className="accent-[#0f0e0d]" />
              Ground in the vault (search sources first, cite them in the draft)
            </label>
            <RefineControls ctl={refineCtl} disabled={!instructions.trim()} />
          </div>
          <button
            disabled={!instructions.trim() || run.isPending}
            onClick={() => run.mutate()}
            className="btn-brand rounded-lex text-ivory px-5 py-2.5 text-[13.5px] font-medium disabled:opacity-40 inline-flex items-center gap-2"
          >
            <PenLine size={14} /> {run.isPending ? 'Drafting…' : 'Generate draft'}
          </button>
          </div>
        </div>
      </div>

      {run.isPending && <Spinner label="Drafting…" />}
      {!run.isPending && result && (
        <div className="mt-5">
          <div className="rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] px-6 py-5">
            <div className="flex items-center justify-between gap-3 flex-wrap">
              <h2 className="font-display text-xl">{KINDS.find((k) => k.id === result.kind)?.label} — draft</h2>
              <div className="flex gap-2">
                <button
                  onClick={() => { navigator.clipboard.writeText(result.draft); toast('success', 'Draft copied.') }}
                  className="rounded-lex border border-stone-300 dark:border-white/10 px-3 py-1.5 text-[12.5px] inline-flex items-center gap-1.5"
                >
                  <Copy size={13} /> Copy
                </button>
                <button
                  onClick={download}
                  className="rounded-lex border border-stone-300 dark:border-white/10 px-3 py-1.5 text-[12.5px] inline-flex items-center gap-1.5"
                >
                  <Download size={13} /> Download .md
                </button>
              </div>
            </div>
            <div className="prose-lex mt-3 text-[14.5px] leading-relaxed">
              <ReactMarkdown
                remarkPlugins={[remarkGfm]}
                components={citeMarkdownComponents(result.citations, setModalCitation)}
              >
                {linkifyCitations(result.draft, result.citations)}
              </ReactMarkdown>
            </div>
          </div>
          {result.citations.length > 0 && (
            <div className="mt-4">
              <p className="text-[13px] text-stone-500 dark:text-stone-400 font-medium mb-2">Sources used</p>
              <div className="grid md:grid-cols-2 gap-2.5">
                {result.citations.map((c, i) => <CitationCard key={c.chunk_id + i} citation={c} go={go} />)}
              </div>
            </div>
          )}
          <p className="text-meta text-stone-400 dark:text-stone-500 mt-4">
            AI-generated first draft — a qualified advocate must review it before it is sent, filed, or relied on.
          </p>
        </div>
      )}
      {!run.isPending && !result && (
        <div className="mt-5"><EmptyState title="No draft yet" hint="Choose a document type, describe the facts, and generate." /></div>
      )}
      {modalCitation && (
        <SourceModal citation={modalCitation} onClose={() => setModalCitation(null)} go={go} />
      )}
    </div>
  )
}
