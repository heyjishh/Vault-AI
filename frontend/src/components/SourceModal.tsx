import { useEffect, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { ExternalLink, Loader2, Lock, PanelRight, X } from 'lucide-react'
import { openOriginalDocument, ragApi } from '../api/client'
import type { Citation, SourceDocument } from '../api/types'

/** Collapse whitespace runs to single spaces, keeping a map from each
 *  normalized index back to its original index — a passage found in
 *  normalized space can then be highlighted in the original text. */
function normalizeWithMap(text: string): { norm: string; map: number[] } {
  let norm = ''
  const map: number[] = []
  let lastWasSpace = true
  for (let i = 0; i < text.length; i++) {
    if (/\s/.test(text[i])) {
      if (!lastWasSpace) {
        norm += ' '
        map.push(i)
        lastWasSpace = true
      }
    } else {
      norm += text[i]
      map.push(i)
      lastWasSpace = false
    }
  }
  return { norm, map }
}

/** Locate the cited passage inside the full document text; returns
 *  [start, end) offsets in the ORIGINAL text, or null. Tries an exact
 *  match on progressively shorter probes of the snippet first, then a
 *  best-overlap window fallback. */
function findPassage(text: string, snippet: string): [number, number] | null {
  const { norm, map } = normalizeWithMap(text)
  const sn = snippet.replace(/\s+/g, ' ').trim()
  if (!sn || map.length === 0) return null
  const toOrig = (nStart: number, nEnd: number): [number, number] => [
    map[Math.max(0, Math.min(nStart, map.length - 1))],
    map[Math.max(0, Math.min(nEnd - 1, map.length - 1))] + 1,
  ]
  for (const len of [sn.length, 400, 240, 160, 100, 64]) {
    if (len < 48) break
    const probe = sn.slice(0, Math.min(len, sn.length))
    const i = norm.indexOf(probe)
    if (i >= 0) return toOrig(i, i + Math.min(sn.length, 1600))
  }
  const words = new Set(sn.toLowerCase().split(/[^a-z0-9]+/).filter((w) => w.length > 3))
  if (words.size === 0) return null
  const CH = 600
  let best: [number, number] | null = null
  let bestScore = 0
  for (let s = 0; s < norm.length; s += CH) {
    const chunkWords = new Set(
      norm.slice(s, s + CH).toLowerCase().split(/[^a-z0-9]+/).filter((w) => w.length > 3),
    )
    let score = 0
    words.forEach((w) => { if (chunkWords.has(w)) score++ })
    if (score > bestScore) {
      bestScore = score
      best = [s, Math.min(norm.length, s + CH)]
    }
  }
  if (best && bestScore >= 4) return toOrig(best[0], best[1])
  return null
}

/** Full-document source viewer: fetches the complete text behind a
 *  citation and highlights the passage the answer relied on, scrolled
 *  into view. Confidential citations never fetch — their text stays in
 *  the Private Vault. Rendered in a portal so it can be hosted inside
 *  a clickable card without click-bubbling surprises. */
export function SourceModal({ citation: c, onClose, go }: {
  citation: Citation
  onClose: () => void
  go?: (view: string) => void
}) {
  const isWeb = !!c.source_url
  const isPrivate = c.doc_id.startsWith('private:')
  const isDoc = c.doc_id.startsWith('doc:')
  const [doc, setDoc] = useState<SourceDocument | null>(null)
  const [error, setError] = useState<string | null>(null)
  // Piece 29: set when the document predates retained originals.
  const [originalMissing, setOriginalMissing] = useState(false)
  const markRef = useRef<HTMLElement | null>(null)

  const openOriginal = async () => {
    const target = doc?.document_id
    if (!target) return
    const res = await openOriginalDocument(target)
    if (res === 'not-retained') setOriginalMissing(true)
    else if (res === 'error') setError('Could not open the original document')
  }

  useEffect(() => {
    if (isPrivate) return
    let live = true
    ragApi
      .sourceDocument(isWeb ? { source_url: c.source_url as string } : { chunk_id: c.chunk_id })
      .then((d) => { if (live) setDoc(d) })
      .catch((e) => { if (live) setError(e instanceof Error ? e.message : 'Could not load this document') })
    return () => { live = false }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [c.chunk_id])

  useEffect(() => {
    const h = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [onClose])

  useEffect(() => {
    if (doc) markRef.current?.scrollIntoView({ block: 'center' })
  }, [doc])

  const domain = c.source_url
    ? (() => { try { return new URL(c.source_url).hostname.replace(/^www\./, '') } catch { return null } })()
    : null
  const pinpoint = c.sentence ?? c.snippet
  const range = doc ? (findPassage(doc.text, pinpoint) ?? findPassage(doc.text, c.snippet)) : null

  return createPortal(
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-ink/45 p-4"
      onClick={(e) => { e.stopPropagation(); onClose() }}
    >
      <div
        className="flex max-h-[86vh] w-full max-w-3xl flex-col rounded-lex-lg bg-white shadow-lex-md"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-start gap-3 border-b border-stone-200 px-6 py-4">
          <span className="mt-0.5 rounded bg-ink px-1.5 py-0.5 font-mono text-[11px] font-semibold text-ivory">{c.doc_id}</span>
          <div className="min-w-0 flex-1">
            <p className="truncate text-[15px] font-semibold leading-snug">{doc?.title ?? c.doc_title ?? c.doc_id}</p>
            <p className="mt-0.5 text-[12.5px] text-stone-500">
              {isWeb && <>{domain}{c.source_date && <> · {c.source_date}</>} · full document</>}
              {isDoc && <>Vault document · full text</>}
              {isPrivate && <>Confidential vault</>}
            </p>
          </div>
          <button
            onClick={onClose}
            className="rounded-full p-1.5 text-stone-500 transition-colors hover:bg-stone-100 hover:text-ink"
            title="Close"
          >
            <X size={17} />
          </button>
        </div>

        <div className="overflow-y-auto px-6 py-5">
          {isPrivate ? (
            <div>
              <p className="text-[13.5px] leading-relaxed text-stone-600">
                This source lives in your Confidential Vault. Its full text is only shown inside the vault, never in the Ask view.
              </p>
              <div className="mt-4 rounded-lex border border-stone-200 bg-[#fafaf9] px-4 py-3 text-[13px] leading-relaxed text-stone-600">
                <span className="font-medium text-ink">Cited passage — </span>
                {pinpoint}
              </div>
              {go && (
                <button
                  onClick={() => go('private')}
                  className="mt-4 inline-flex items-center gap-1.5 text-[13px] font-medium text-ink underline underline-offset-2"
                >
                  <Lock size={13} /> Open in Private Vault
                </button>
              )}
            </div>
          ) : error ? (
            <div>
              <p className="text-[13.5px] text-stone-600">Couldn’t fetch the full document: {error}</p>
              {isWeb && c.source_url && (
                <a
                  href={c.source_url}
                  target="_blank"
                  rel="noreferrer"
                  className="mt-3 inline-flex items-center gap-1.5 text-[13px] font-medium text-ink underline underline-offset-2"
                >
                  <ExternalLink size={13} /> Open the original instead
                </a>
              )}
            </div>
          ) : !doc ? (
            <div className="flex items-center gap-2.5 py-8 text-[13.5px] text-stone-500">
              <Loader2 size={16} className="animate-spin" />
              Fetching the full document…{isWeb ? ' Large judgments can take a few seconds.' : ''}
            </div>
          ) : (
            <div>
              {!range && (
                <p className="mb-3 rounded-lex border border-amber-600/25 bg-[#fdf6e3] px-3 py-2 text-[12.5px] text-[#7a5b12]">
                  The exact cited passage couldn’t be located in the fetched text — showing the full document.
                </p>
              )}
              <div className="whitespace-pre-wrap text-[13.5px] leading-[1.8] text-stone-700">
                {range ? (
                  <>
                    {doc.text.slice(0, range[0])}
                    <mark ref={markRef} className="rounded bg-[#fdf3d7] px-0.5 text-ink">
                      {doc.text.slice(range[0], range[1])}
                    </mark>
                    {doc.text.slice(range[1])}
                  </>
                ) : (
                  doc.text
                )}
              </div>
            </div>
          )}
        </div>

        {originalMissing && (
          <div className="border-t border-stone-200 bg-[#fdf6e3] px-6 py-2.5 text-[12.5px] text-[#7a5b12]">
            The original file wasn't retained for this document — it was uploaded before originals were
            kept. The full extracted text above is what exists.
          </div>
        )}

        <div className="flex items-center justify-between border-t border-stone-200 px-6 py-3">
          <span className="inline-flex items-center gap-1.5 text-[12px] text-stone-500">
            <span className="inline-block h-3 w-4 rounded-sm bg-[#fdf3d7]" /> Cited passage
          </span>
          {isWeb && c.source_url ? (
            <a
              href={c.source_url}
              target="_blank"
              rel="noreferrer"
              className="inline-flex items-center gap-1.5 text-[12.5px] font-medium text-ink hover:underline underline-offset-2"
            >
              <ExternalLink size={12} /> Open original{domain && <> · {domain}</>}
            </a>
          ) : isDoc ? (
            <span className="inline-flex items-center gap-4">
              <button
                onClick={() => void openOriginal()}
                disabled={!doc?.document_id}
                className="inline-flex items-center gap-1.5 text-[12.5px] font-medium text-ink hover:underline underline-offset-2 disabled:opacity-40"
              >
                <ExternalLink size={12} /> Open original document
              </button>
              {go && (
                <button
                  onClick={() => {
                    const target = doc?.document_id
                    if (!target) return
                    try {
                      sessionStorage.setItem('jurisvault.interactDoc', target)
                    } catch { /* ignore */ }
                    go('interact')
                  }}
                  className="inline-flex items-center gap-1.5 text-[12.5px] font-medium text-ink hover:underline underline-offset-2"
                >
                  <PanelRight size={12} /> Open in Interact
                </button>
              )}
            </span>
          ) : (
            <span />
          )}
        </div>
      </div>
    </div>,
    document.body,
  )
}

/** Convert [doc:N] / [web:N] / [private:N] markers in markdown text
 *  into links the cite components turn into source chips. */
export function linkifyCitations(text: string, citations: Citation[]): string {
  return text.replace(/\[(doc|web|private):(\d+)\]/g, (_m, kind: string, n: string) => {
    const c = citations.find((x) => x.doc_id === `${kind}:${n}`)
    return c ? `[${kind}:${n}](#cite-${c.chunk_id})` : `[${kind}:${n}]`
  })
}

/** ReactMarkdown `components` override: `#cite-…` links become chips
 *  that open the source modal; other links open externally. */
export function citeMarkdownComponents(citations: Citation[], onOpen: (c: Citation) => void) {
  return {
    a: ({ href, children }: { href?: string; children?: ReactNode }) => {
      if (href?.startsWith('#cite-')) {
        const chunkId = href.slice('#cite-'.length)
        const c =
          citations.find((x) => x.chunk_id === chunkId) ??
          citations.find((x) => x.chunk_id === decodeURIComponent(chunkId))
        if (c) {
          return (
            <a
              href={href}
              onClick={(e) => { e.preventDefault(); onOpen(c) }}
              className="cite-chip"
              title="Open the full source document"
            >
              {children}
            </a>
          )
        }
      }
      return (
        <a href={href} target="_blank" rel="noreferrer">
          {children}
        </a>
      )
    },
  }
}

/** Plain-text renderer that turns citation markers into chips —
 *  for surfaces that don't render markdown (research conclusions,
 *  agent step findings). */
export function CitedText({ text, citations, onOpen }: {
  text: string
  citations: Citation[]
  onOpen: (c: Citation) => void
}) {
  const marker = /\[(doc|web|private):(\d+)\]/g
  const parts: ReactNode[] = []
  let last = 0
  let k = 0
  let m: RegExpExecArray | null
  while ((m = marker.exec(text)) !== null) {
    const match = m
    const c = citations.find((x) => x.doc_id === `${match[1]}:${match[2]}`)
    if (!c) continue
    if (match.index > last) parts.push(text.slice(last, match.index))
    parts.push(
      <a
        key={k++}
        href={`#cite-${c.chunk_id}`}
        onClick={(e) => { e.preventDefault(); onOpen(c) }}
        className="cite-chip"
        title="Open the full source document"
      >
        {match[0].slice(1, -1)}
      </a>,
    )
    last = match.index + match[0].length
  }
  if (last < text.length) parts.push(text.slice(last))
  return <>{parts}</>
}
