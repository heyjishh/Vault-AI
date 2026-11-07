import type { PassageDiff, ToolTraceEntry } from '../api/types'

/** Piece 84: the tool-call trace of a tool-agent run — every call the
 *  model made, what it cost, and whether it succeeded. Shared by the
 *  Agents page (Piece 89) and its run-history detail view. Extracted
 *  from AgentsPage in Piece 89 so both agent surfaces render the same
 *  trace without a second implementation. */
export function ToolTraceList({ trace }: { trace: ToolTraceEntry[] }) {
  if (trace.length === 0) return null
  return (
    <div className="space-y-1.5">
      {trace.map((t) => (
        <div key={t.n} className="flex items-start gap-2.5 text-meta2">
          <span className="shrink-0 text-meta font-medium text-stone-600 dark:text-stone-300">
            {t.n}. {t.tool.replace('_', ' ')} · {t.status}
          </span>
          <span className="text-stone-600 dark:text-stone-300 min-w-0">
            {t.args_digest && <span className="break-words">{t.args_digest}</span>}
            {t.error && <span className="text-[#a02c2c]"> — {t.error}</span>}
            {t.child && (
              <span className="text-stone-400 dark:text-stone-500">
                {' '}· sub-agent “{t.child.goal}” stopped: {t.child.stopped}, {t.child.steps} trace entries
              </span>
            )}
            <span className="text-stone-400 dark:text-stone-500">
              {' '}· {t.result_chars.toLocaleString()} chars · ~{t.tokens_estimate.toLocaleString()} tok · {t.duration_ms} ms
            </span>
          </span>
        </div>
      ))}
    </div>
  )
}

/** Piece 85: a passage diff an approver can actually read.
 *  Shared by the agent approval cards (Piece 86/89). */
export function DiffView({ diff }: { diff: PassageDiff }) {
  if (diff.added.length === 0 && diff.removed.length === 0) {
    return <p className="text-meta2 text-stone-500 dark:text-stone-400">No textual change.</p>
  }
  const cap = (xs: string[]) => xs.slice(0, 8)
  return (
    <div className="space-y-1.5">
      {cap(diff.removed).map((p, i) => (
        <p key={`r${i}`} className="rounded-lex bg-[#feeaea] dark:bg-[#2a1212] px-3 py-2 text-meta2 text-[#7a2020] dark:text-[#e8a0a0] whitespace-pre-wrap">− {p}</p>
      ))}
      {diff.removed.length > 8 && <p className="text-meta text-stone-400">…and {diff.removed.length - 8} more removed passage(s)</p>}
      {cap(diff.added).map((p, i) => (
        <p key={`a${i}`} className="rounded-lex bg-[#e9f7ec] dark:bg-[#0f2416] px-3 py-2 text-meta2 text-[#205c2a] dark:text-[#9fd3a8] whitespace-pre-wrap">+ {p}</p>
      ))}
      {diff.added.length > 8 && <p className="text-meta text-stone-400">…and {diff.added.length - 8} more added passage(s)</p>}
    </div>
  )
}
