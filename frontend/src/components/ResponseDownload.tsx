import { useEffect, useRef, useState } from 'react'
import { ChevronDown, Download } from 'lucide-react'
import { buildPdf, buildRtf, exportFileBase, type ExportCitation } from '../lib/exportResponse'
import { useSession } from '../stores/session'

/**
 * Piece 28: download one completed response as a formatted document.
 * Piece 30: the user also chooses With citations or Without citations.
 *
 * The button opens a small menu grouped by citation choice — each
 * group offers PDF and RTF. With citations keeps the inline
 * [doc:N] / [web:N] / [private:N] markers and the Sources list;
 * Without citations is a clean copy with neither. Generation is
 * entirely client-side (lib/exportResponse), so it works for restored
 * history turns exactly as for fresh ones.
 */
export function ResponseDownload({ question, title, answerMarkdown, citations }: {
  question: string
  /** Context line (Interact: the action's document titles). */
  title?: string
  answerMarkdown: string
  citations?: ExportCitation[]
}) {
  const toast = useSession((s) => s.toast)
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const onDocClick = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setOpen(false) }
    document.addEventListener('mousedown', onDocClick)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDocClick)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  const save = (format: 'pdf' | 'rtf', includeCitations: boolean) => {
    setOpen(false)
    try {
      const input = { question, title, answerMarkdown, citations, includeCitations, generatedAt: new Date() }
      const blob = format === 'pdf' ? buildPdf(input) : buildRtf(input)
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      a.download = `${exportFileBase(input)}.${format}`
      a.click()
      URL.revokeObjectURL(url)
      toast('success', `Downloading ${format.toUpperCase()} ${includeCitations ? 'with citations' : 'without citations'}`)
    } catch (e) {
      toast('error', `Could not build the ${format.toUpperCase()} file: ${(e as Error).message}`)
    }
  }

  const hasCitations = (citations?.length ?? 0) > 0

  return (
    <div className="relative" ref={rootRef}>
      <button
        onClick={() => setOpen((v) => !v)}
        title="Download this response as a document"
        className="flex items-center gap-1.5 rounded-lex border border-stone-200 dark:border-white/10 bg-white dark:bg-[#1e1c19] px-2.5 py-1.5 text-[12px] font-medium text-stone-600 dark:text-stone-300 hover:text-ink hover:border-stone-300 transition-colors"
      >
        <Download size={13} /> Download <ChevronDown size={12} />
      </button>
      {open && (
        <div className="absolute left-0 top-full z-20 mt-1.5 w-60 overflow-hidden rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#1e1c19] py-1 shadow-lex-md">
          <p className="px-3 pt-2 pb-1 text-[12px] font-medium text-stone-400 dark:text-stone-500">
            With citations
          </p>
          <button
            onClick={() => save('pdf', true)}
            className="block w-full px-3 py-2 text-left text-[13px] hover:bg-stone-100 dark:hover:bg-white/10"
          >
            Download PDF
            <span className="block text-[11px] text-stone-400 dark:text-stone-500">Citations in the text + Sources list</span>
          </button>
          <button
            onClick={() => save('rtf', true)}
            className="block w-full px-3 py-2 text-left text-[13px] hover:bg-stone-100 dark:hover:bg-white/10"
          >
            Download RTF
            <span className="block text-[11px] text-stone-400 dark:text-stone-500">Citations in the text + Sources list</span>
          </button>
          <div className="my-1 border-t border-stone-100 dark:border-white/10" />
          <p className="px-3 pt-1 pb-1 text-[12px] font-medium text-stone-400 dark:text-stone-500">
            Without citations
          </p>
          <button
            onClick={() => save('pdf', false)}
            className="block w-full px-3 py-2 text-left text-[13px] hover:bg-stone-100 dark:hover:bg-white/10"
          >
            Download PDF
            <span className="block text-[11px] text-stone-400 dark:text-stone-500">Clean copy — no citation markers, no Sources list</span>
          </button>
          <button
            onClick={() => save('rtf', false)}
            className="block w-full px-3 py-2 text-left text-[13px] hover:bg-stone-100 dark:hover:bg-white/10"
          >
            Download RTF
            <span className="block text-[11px] text-stone-400 dark:text-stone-500">Clean copy — no citation markers, no Sources list</span>
          </button>
          {!hasCitations && (
            <p className="px-3 py-2 text-[11px] text-stone-400 dark:text-stone-500 border-t border-stone-100 dark:border-white/10 mt-1">
              This response has no citations, so both versions are the same content.
            </p>
          )}
        </div>
      )}
    </div>
  )
}

/** Citations as the export wants them: one entry per source document
 *  (Ask/Interact citations are per-chunk, so the same document can
 *  appear several times). */
export function exportCitations(citations: { doc_id: string; doc_title?: string | null }[]): ExportCitation[] {
  const seen = new Set<string>()
  const out: ExportCitation[] = []
  for (const c of citations) {
    const title = c.doc_title ?? c.doc_id
    const key = `${c.doc_id}|${title}`
    if (seen.has(key)) continue
    seen.add(key)
    out.push({ label: c.doc_id, title })
  }
  return out
}
