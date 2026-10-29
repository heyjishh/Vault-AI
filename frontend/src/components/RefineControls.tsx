import { useCallback, useRef, useState } from 'react'
import { Sparkles, Undo2 } from 'lucide-react'
import { ragApi } from '../api/client'
import { useSession } from '../stores/session'

/**
 * Piece 13 — the shared "Refine prompt" affordance for every chat
 * surface (Ask, Interact, Research, Draft, Agents). Hybrid by
 * design: one explicit click asks the refine endpoint for a VISIBLE
 * rewrite plus retrieval terms. The rewrite replaces the input's
 * text in place — still editable, never auto-sent — and the exact
 * original is kept for a byte-identical Undo. The terms are held
 * until the caller sends: the caller passes them as `refine_terms`
 * (folded server-side into the retrieval expansion pool) and calls
 * `consumeTerms()`. A refine failure leaves the input untouched and
 * only toasts: sending the original is never blocked by refining.
 */
export function useRefine(opts: {
  value: string
  setValue: (v: string) => void
  surface: string
}) {
  const toast = useSession((s) => s.toast)
  const [busy, setBusy] = useState(false)
  const [original, setOriginal] = useState<string | null>(null)
  const [terms, setTerms] = useState<string[]>([])
  const valueRef = useRef(opts.value)
  valueRef.current = opts.value
  const setValueRef = useRef(opts.setValue)
  setValueRef.current = opts.setValue

  const refine = useCallback(async () => {
    const text = valueRef.current
    if (!text.trim() || busy) return
    setBusy(true)
    try {
      const out = await ragApi.refine(text, opts.surface)
      setOriginal(text)
      setValueRef.current(out.refined)
      setTerms(out.terms)
    } catch (e) {
      toast(
        'error',
        `Could not refine — your text is unchanged and can be sent as-is. ${e instanceof Error ? e.message : ''}`.trim(),
      )
    } finally {
      setBusy(false)
    }
  }, [busy, opts.surface, toast])

  const undo = useCallback(() => {
    if (original !== null) {
      setValueRef.current(original)
      setOriginal(null)
      setTerms([])
    }
  }, [original])

  /** The terms to attach to the next send; clears the refine state
   * (the sent turn keeps its own copy for display). */
  const consumeTerms = useCallback(() => {
    const t = terms
    setTerms([])
    setOriginal(null)
    return t
  }, [terms])

  return { busy, refined: original !== null, terms, refine, undo, consumeTerms }
}

export type RefineCtl = ReturnType<typeof useRefine>

/**
 * The control row: a Refine pill (or "Refined — Undo" after a
 * successful refine) plus the subtle "Searching also for: …" line
 * once terms exist. Styled like the other composer pills.
 */
export function RefineControls({ ctl, disabled }: { ctl: RefineCtl; disabled?: boolean }) {
  return (
    <span className="inline-flex flex-wrap items-center gap-2">
      {ctl.refined ? (
        <button
          type="button"
          onClick={ctl.undo}
          title="Restore the exact text you wrote before refining"
          className="flex items-center gap-1.5 rounded-full border border-ink bg-ink px-2.5 py-1 text-[12px] text-ivory transition-colors"
        >
          <Undo2 size={12} /> Refined — Undo
        </button>
      ) : (
        <button
          type="button"
          onClick={ctl.refine}
          disabled={disabled || ctl.busy}
          title="Rewrite your prompt so the system understands it better, and add retrieval terms. You review the rewrite before sending — nothing is sent automatically."
          className="flex items-center gap-1.5 rounded-full border border-stone-200 px-2.5 py-1 text-[12px] text-stone-500 transition-colors hover:border-stone-300 hover:text-ink disabled:opacity-40"
        >
          <Sparkles size={12} /> {ctl.busy ? 'Refining…' : 'Refine'}
        </button>
      )}
      {ctl.terms.length > 0 && (
        <span className="text-[11px] text-stone-400">
          Searching also for: {ctl.terms.join(' · ')}
        </span>
      )}
    </span>
  )
}
