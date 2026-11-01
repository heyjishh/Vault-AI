import { useMemo, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { BookOpen, LayoutTemplate, Search, Star, X } from 'lucide-react'
import { promptsApi, type PromptItem } from '../api/client'
import { TemplatePicker } from './TemplatePicker'

/** The Prompt library modal + the composers' library pills.
 *  Piece 63 shipped the sidebar page + picker; Piece 65 wired
 *  the picker into every chat input (then via a + button);
 *  Piece 66 put the + behind a menu; Piece 71 (below) replaced
 *  the + with explicit Refine-style pills, per JISHAN. */
/** Piece 71: NO plus button (JISHAN: "remove the plus button,
 *  and put prompt and template should be look like refine
 *  button"). The composers now carry explicit library pills
 *  styled exactly like the Refine pill — a "Prompts" pill in
 *  every chat input, plus a "Templates" pill in Draft only —
 *  each opening its library modal directly. */
export function ComposerLibraryButtons({ onPick, withTemplates = false }: {
  onPick: (body: string) => void
  /** Draft only — Templates exist in Draft and nowhere else. */
  withTemplates?: boolean
}) {
  const [pickerOpen, setPickerOpen] = useState(false)
  const [tplOpen, setTplOpen] = useState(false)
  const pill = 'flex items-center gap-1.5 rounded-lex border border-stone-200 dark:border-white/10 px-2.5 py-1 text-[12px] text-stone-500 dark:text-stone-400 transition-colors hover:border-stone-300 hover:text-ink'
  return (
    <span className="inline-flex items-center gap-2">
      <button
        type="button"
        onClick={() => setPickerOpen(true)}
        title="Insert from the prompt library"
        className={pill}
      >
        <BookOpen size={12} /> Prompts
      </button>
      {withTemplates && (
        <button
          type="button"
          onClick={() => setTplOpen(true)}
          title="Use a drafting template"
          className={pill}
        >
          <LayoutTemplate size={12} /> Templates
        </button>
      )}
      {pickerOpen && <PromptPicker onUse={onPick} onClose={() => setPickerOpen(false)} />}
      {tplOpen && <TemplatePicker onUse={onPick} onClose={() => setTplOpen(false)} />}
    </span>
  )
}

/** Build the pick handler for a composer: appends the prompt to the
 *  input's current text (or fills an empty input), then focuses the
 *  field and HIGHLIGHTS the inserted prompt text in the input, as
 *  JISHAN asked — the selection shows exactly what was inserted and
 *  typing replaces it. */
export function promptInsert(
  current: string,
  setValue: (v: string) => void,
  elementId: string,
): (body: string) => void {
  return (body: string) => {
    const prefix = current.trim() ? `${current.replace(/\s+$/, '')}\n` : ''
    const next = prefix + body
    setValue(next)
    window.setTimeout(() => {
      const el = document.getElementById(elementId) as HTMLTextAreaElement | HTMLInputElement | null
      if (!el) return
      el.focus()
      try { el.setSelectionRange(prefix.length, next.length) } catch { /* input type rejects selection */ }
    }, 40)
  }
}

export function promptInGroup(p: PromptItem, group: string): boolean {
  switch (group) {
    case 'All': return true
    case 'Suggested': return p.suggested
    case 'Created By Me': return p.mine
    case 'Starred': return p.starred
    case 'Shared With Me': return !p.mine && p.owner_email != null
    default: return p.category === group
  }
}

function PromptPicker({ onClose, onUse }: { onClose: () => void; onUse: (body: string) => void }) {
  const { data, isLoading } = useQuery({ queryKey: ['prompts'], queryFn: promptsApi.list })
  const qc = useQueryClient()
  const [q, setQ] = useState('')
  const [group, setGroup] = useState('All')
  const groups = useMemo(() => {
    const cats = (data?.categories ?? []).filter((c) => c !== 'Suggested Prompts')
    return ['All', 'Suggested', 'Created By Me', 'Starred', 'Shared With Me', ...cats]
  }, [data])
  const filtered = useMemo(() => {
    const needle = q.trim().toLowerCase()
    return (data?.prompts ?? []).filter(
      (p) => promptInGroup(p, group) && (!needle || p.title.toLowerCase().includes(needle) || p.body.toLowerCase().includes(needle)),
    )
  }, [data, q, group])

  async function toggleStar(p: PromptItem) {
    await promptsApi.star(p.id)
    qc.invalidateQueries({ queryKey: ['prompts'] })
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/45 p-4" onClick={onClose}>
      <div
        className="pop-in w-full max-w-2xl max-h-[80vh] flex flex-col rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] shadow-lex-md"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-3 px-5 pt-4 pb-3 border-b border-stone-200 dark:border-white/10">
          <h2 className="font-display text-lg flex items-center gap-2"><BookOpen size={17} /> Prompt Library</h2>
          <div className="ml-auto relative">
            <Search size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-stone-400" />
            <input
              value={q} onChange={(e) => setQ(e.target.value)} autoFocus
              placeholder="Search all prompts"
              className="w-52 rounded-lex border border-stone-300 dark:border-white/10 bg-white dark:bg-[#0a1a12] pl-7 pr-3 py-1.5 text-[13px] outline-none focus:border-ink placeholder:text-stone-400"
            />
          </div>
          <button onClick={onClose} className="p-1.5 rounded-lex text-stone-500 dark:text-stone-400 hover:text-ink hover:bg-stone-100 dark:hover:bg-white/10" title="Close">
            <X size={16} />
          </button>
        </div>
        <div className="flex gap-1.5 flex-wrap px-5 py-3 border-b border-stone-200 dark:border-white/10">
          {groups.map((g) => (
            <button
              key={g}
              onClick={() => setGroup(g)}
              className={`rounded-lex px-2.5 py-1 text-[12px] border transition-colors ${group === g ? 'bg-ink text-ivory border-ink' : 'border-stone-200 dark:border-white/10 text-stone-500 dark:text-stone-400 hover:text-ink hover:border-stone-300'}`}
            >
              {g}
            </button>
          ))}
        </div>
        <div className="overflow-y-auto px-3 py-2">
          {isLoading && <p className="px-2 py-6 text-[13px] text-stone-400">Loading prompts…</p>}
          {!isLoading && filtered.length === 0 && (
            <p className="px-2 py-6 text-[13px] text-stone-400">No prompts here yet. Create your own from the Prompts page in the sidebar.</p>
          )}
          {filtered.map((p) => (
            <div key={p.id} className="group flex items-start gap-3 rounded-lex px-2.5 py-3 hover:bg-stone-100/70 dark:hover:bg-white/5">
              <div className="min-w-0 flex-1">
                <p className="text-[14px] font-medium truncate">{p.title}</p>
                <p className="text-[12.5px] text-stone-500 dark:text-stone-400 leading-relaxed line-clamp-2 mt-0.5">{p.body}</p>
                <p className="text-[11px] text-stone-400 dark:text-stone-500 mt-1">
                  {p.category}{p.mine ? ' · Yours' : p.owner_email ? ` · Shared by ${p.owner_email}` : ' · Juris Vault'}
                </p>
              </div>
              <button
                onClick={() => void toggleStar(p)}
                title={p.starred ? 'Unstar' : 'Star'}
                className={`p-1.5 shrink-0 ${p.starred ? 'text-[#b8860b] dark:text-[#e3c878]' : 'text-stone-300 dark:text-stone-600 hover:text-stone-500'}`}
              >
                <Star size={15} fill={p.starred ? 'currentColor' : 'none'} />
              </button>
              <button
                onClick={() => onUse(p.body)}
                className="btn-brand shrink-0 rounded-lex text-ivory px-3.5 py-1.5 text-[12.5px] font-medium"
              >
                Use
              </button>
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}
