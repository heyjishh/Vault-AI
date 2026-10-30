import { useEffect, useRef, useState } from 'react'
import { ChevronDown, Download } from 'lucide-react'
import { buildPdf, buildRtf, exportFileBase, type ExportCitation } from '../lib/exportResponse'
import { useSession } from '../stores/session'

/**
 * Piece 28: download one completed response as a formatted document.
 * The button opens a small menu — PDF or RTF, the user's choice —
 * and each file carries the exact question first, then the response
 * with its structure, then its sources. Generation is entirely
 * client-side (lib/exportResponse), so it works for restored history
 * turns exactly as for fresh ones.
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
  }, [open ])

  const save = (format: 'pdf' | 'rtf') => {
    setOpen(false)
    try {
      const input = { question, title, answerMarkdown, citations, generatedAt: new Date() }
      const blob = format === 'pdf' ? buildPdf(input) : buildRtf(input)
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      a.download = `${exportFileBase(input)}.${format}`
      a.click()
      URL.revokeObjectURL(url)
      toast('success', `Downloading ${format.toUpperCase()}`)
    } catch (e) {
      toast('error', `Could not build the ${format.toUpperCase()} file: ${(e as Error).message}`)
    }
  }

  return (
    <div className="relative" ref={rootRef}>
      <button
        onClick={() => setOpen((v) => !v)}
        title="Download this response as a document"
        className="flex items-center gap-1.5 rounded-lex border border-stone-200 bg-white px-2.5 py-1.5 text-[12px] font-medium text-stone-600 hover:text-ink hover:border-stone-300 transition-colors"
      >
        <Download size={13} /> Download <ChevronDown size={12} />
      </button>
      {open && (
        <div className="absolute left-0 top-full z-20 mt-1.5 w-44 overflow-hidden rounded-lex-lg border border-stone-200 bg-white py-1 shadow-lex-md">
          <button
            onClick={() => save('pdf')}
            className="block w-full px-3 py-2 text-left text-[13px] hover:bg-stone-100"
          >
            Download PDF
            <span className="block text-[11px] text-stone-400">Formatted, ready to share or print</span>
          </button>
          <button
            onClick={() => save('rtf')}
            className="block w-full px-3 py-2 text-left text-[13px] hover:bg-stone-100"
          >
            Download RTF
            <span className="block text-[11px] text-stone-400">Opens in Word and other editors</span>
          </button>
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
