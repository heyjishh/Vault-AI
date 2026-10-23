import { useEffect, useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  MessagesSquare, MonitorSmartphone, Palette, Plus, RotateCcw, SlidersHorizontal, Trash2, Waypoints,
} from 'lucide-react'
import { adminApi, type PanelPrompts, type PanelSettings, type UiTab } from '../api'
import { ErrorBanner, PageHead, Spinner } from '../ui'

const MODES: { id: string; label: string; hint: string }[] = [
  { id: 'mix', label: 'Mix — recommended', hint: 'Hybrid graph + vector retrieval. The best all-rounder for legal research; this is the default.' },
  { id: 'hybrid', label: 'Hybrid', hint: 'Local + global graph search, no plain vector pass.' },
  { id: 'naive', label: 'Naive', hint: 'Plain vector similarity only — fastest, least context.' },
  { id: 'local', label: 'Local', hint: 'Entity-neighborhood search — best for “who/what is connected to X”.' },
  { id: 'global', label: 'Global', hint: 'Community-level graph search — best for themes across the whole corpus.' },
  { id: 'bypass', label: 'Bypass', hint: 'No retrieval — the model answers from its own knowledge. Not grounded.' },
]

const inputCls = 'w-full rounded-lex border border-stone-200 bg-ivory px-3 py-2 text-sm outline-none focus:border-stone-400'
const labelCls = 'text-[12px] uppercase tracking-wider text-stone-400 font-medium'

type Section = 'branding' | 'answering' | 'surfaces' | 'models' | 'tabs'

const SECTIONS: { id: Section; label: string; icon: React.ElementType; blurb: string }[] = [
  { id: 'branding', label: 'Branding', icon: Palette, blurb: 'What the panel is called' },
  { id: 'answering', label: 'Answering', icon: MessagesSquare, blurb: 'Default mode & the Ask prompt' },
  { id: 'surfaces', label: 'AI Surfaces', icon: Waypoints, blurb: 'Interact, Draft, Research, Agents' },
  { id: 'models', label: 'Models', icon: SlidersHorizontal, blurb: 'LLM & embedding models' },
  { id: 'tabs', label: 'Sidebar Tabs', icon: MonitorSmartphone, blurb: 'Custom pages & links for users' },
]

/** Prompt surfaces that have a real built-in default text to prefill. */
const TEXT_PROMPTS = ['ask', 'interact', 'draft', 'research_frame', 'research_adjudicate'] as const
type TextPromptKey = (typeof TEXT_PROMPTS)[number]

interface PromptField {
  key: TextPromptKey | 'agent'
  label: string
  rows: number
  hint: string
}

const SURFACE_PROMPTS: PromptField[] = [
  { key: 'interact', label: 'Interact — document analyst', rows: 7, hint: 'Drives Interact: summary, chronology, key clauses, risks, and document chat.' },
  { key: 'draft', label: 'Draft — drafting assistant', rows: 7, hint: 'Drives Draft: legal notices, agreements, memos, petitions, letters.' },
  { key: 'research_frame', label: 'Research — planner', rows: 6, hint: 'Breaks a research question into testable hypotheses. Caution: keep the JSON output format — change the voice, keep the structure.' },
  { key: 'research_adjudicate', label: 'Research — adjudicator', rows: 7, hint: 'Weighs the gathered evidence and writes the research report. Caution: keep the JSON verdict format — a prompt that drops it will fail adjudication.' },
  { key: 'agent', label: 'Agents — standing instructions', rows: 4, hint: 'Added on top of each workflow’s own built-in instruction (shown below). Steers voice, format, and rules across all agent runs.' },
]

export default function CustomizePage() {
  const queryClient = useQueryClient()
  const { data, isLoading, error } = useQuery({ queryKey: ['panel-settings'], queryFn: adminApi.settings })
  const [form, setForm] = useState<PanelSettings | null>(null)
  const [baseline, setBaseline] = useState<string>('')
  const [section, setSection] = useState<Section>('branding')
  const [saved, setSaved] = useState(false)
  const [err, setErr] = useState<string | null>(null)

  const defaults = useMemo(() => data?.defaults ?? {}, [data])

  /** The payload that would be sent: a prompt identical to its built-in
   *  default is stored as "no override", never as a frozen copy. */
  const normalize = (f: PanelSettings): PanelSettings => {
    const prompts: PanelPrompts = {}
    for (const k of TEXT_PROMPTS) {
      const v = f.prompts?.[k]
      prompts[k] = v && v.trim() && v.trim() !== (defaults[k] ?? '').trim() ? v : null
    }
    prompts.agent = f.prompts?.agent?.trim() ? f.prompts.agent : null
    return { ...f, prompts }
  }

  useEffect(() => {
    if (data && !form) {
      const s = data.settings
      const prompts: PanelPrompts = { agent: s.prompts?.agent ?? '' }
      for (const k of TEXT_PROMPTS) prompts[k] = s.prompts?.[k] ?? data.defaults[k] ?? ''
      const initial: PanelSettings = { ...s, prompts }
      setForm(initial)
      setBaseline(JSON.stringify(normalize(initial)))
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data, form])

  const save = useMutation({
    mutationFn: (body: PanelSettings) => adminApi.updateSettings(body),
    onSuccess: (_r, sent) => {
      setSaved(true)
      setErr(null)
      setBaseline(JSON.stringify(sent))
      void queryClient.invalidateQueries({ queryKey: ['panel-settings'] })
      setTimeout(() => setSaved(false), 3000)
    },
    onError: (e) => setErr((e as Error).message),
  })

  if (isLoading || !data || !form) return <div className="max-w-5xl mx-auto px-6 py-10"><Spinner label="Loading settings…" /></div>
  if (error) return <div className="max-w-5xl mx-auto px-6 py-10"><ErrorBanner message={(error as Error).message} /></div>

  const eff = data.effective
  const set = (patch: Partial<PanelSettings>) => setForm({ ...form, ...patch })
  const setPrompt = (key: keyof PanelPrompts, value: string) =>
    set({ prompts: { ...form.prompts, [key]: value } })
  const setTab = (i: number, patch: Partial<UiTab>) =>
    set({ tabs: form.tabs.map((t, j) => (j === i ? { ...t, ...patch } : t)) })

  const dirty = JSON.stringify(normalize(form)) !== baseline
  const isCustom = (key: TextPromptKey) =>
    (form.prompts?.[key] ?? '').trim() !== '' && (form.prompts?.[key] ?? '').trim() !== (defaults[key] ?? '').trim()
  const sectionDirty: Record<Section, boolean> = {
    branding: false,
    answering: isCustom('ask'),
    surfaces: (['interact', 'draft', 'research_frame', 'research_adjudicate'] as TextPromptKey[]).some(isCustom) || !!(form.prompts?.agent ?? '').trim(),
    models: false,
    tabs: false,
  }

  const promptField = ({ key, label, rows, hint }: PromptField) => {
    const isText = key !== 'agent'
    const custom = isText ? isCustom(key as TextPromptKey) : !!(form.prompts?.agent ?? '').trim()
    return (
      <div key={key} className="mt-6 first:mt-0">
        <div className="flex items-center gap-2 flex-wrap">
          <span className={labelCls}>{label}</span>
          {custom ? (
            <span className="text-[10.5px] font-semibold uppercase tracking-wide bg-emerald-50 text-emerald-700 border border-emerald-200 rounded-full px-2 py-0.5">Customized</span>
          ) : (
            <span className="text-[10.5px] font-semibold uppercase tracking-wide bg-stone-100 text-stone-500 border border-stone-200 rounded-full px-2 py-0.5">{isText ? 'Built-in default' : 'No standing instructions'}</span>
          )}
          <button
            type="button"
            onClick={() => setPrompt(key, isText ? (defaults[key] ?? '') : '')}
            className="ml-auto inline-flex items-center gap-1 text-[11.5px] text-stone-400 hover:text-ink transition-colors"
            title={isText ? 'Restore the built-in default text' : 'Clear standing instructions'}
          >
            <RotateCcw size={11} /> Reset
          </button>
        </div>
        <textarea
          className={`${inputCls} mt-1.5 resize-y font-mono text-[12.5px] leading-relaxed`}
          rows={rows}
          value={form.prompts?.[key] ?? ''}
          placeholder={isText ? '' : 'Example: Always end with a one-line risk rating. Prefer Indian law references. Flag any clause missing a governing-law provision.'}
          onChange={(e) => setPrompt(key, e.target.value)}
        />
        <p className="text-[12.5px] text-stone-500 mt-1">{hint}</p>
        {key === 'agent' && (
          <div className="mt-3 rounded-lex border border-stone-150 border-stone-200 bg-stone-50 p-3.5 space-y-2.5">
            <p className="text-[11px] uppercase tracking-wider text-stone-400 font-medium">Built-in workflow instructions (read-only)</p>
            {data.agent_workflows.map((w) => (
              <div key={w.id}>
                <p className="text-[12.5px] font-medium text-ink">{w.name}</p>
                <p className="text-[12px] text-stone-500 leading-relaxed">{w.report_instruction}</p>
              </div>
            ))}
          </div>
        )}
      </div>
    )
  }

  return (
    <div className="max-w-5xl mx-auto px-6 py-10">
      <PageHead
        title="Customize"
        sub="Shape the user panel from here — branding, how it answers, which models it uses, and which tabs users see. Prompt editors are prefilled with the built-in default: edit the text to customize it."
      />
      {err && <div className="mt-4"><ErrorBanner message={err} /></div>}

      <div className="mt-8 flex gap-6 items-start">
        {/* Section tree */}
        <nav className="w-56 flex-shrink-0 rounded-lex-lg border border-stone-200 bg-white shadow-lex-sm p-2 space-y-1 sticky top-6">
          {SECTIONS.map(({ id, label, icon: Icon, blurb }) => (
            <button
              key={id}
              onClick={() => setSection(id)}
              className={`w-full text-left rounded-lex px-3 py-2.5 transition-colors ${
                section === id
                  ? 'bg-stone-100 shadow-[inset_0_0_0_1px_#e5e5e3]'
                  : 'hover:bg-stone-100/70'
              }`}
            >
              <span className={`flex items-center gap-2 text-[13.5px] ${section === id ? 'text-ink font-medium' : 'text-stone-600'}`}>
                <Icon size={15} strokeWidth={1.8} />
                {label}
                {sectionDirty[id] && <span className="ml-auto w-2 h-2 rounded-full bg-emerald-500" title="Customized" />}
              </span>
              <span className="block text-[11.5px] text-stone-400 mt-0.5 pl-[23px]">{blurb}</span>
            </button>
          ))}
          <div className="border-t border-stone-100 mt-2 pt-3 px-3 pb-2">
            <button
              onClick={() => save.mutate(normalize(form))}
              disabled={save.isPending || !dirty}
              className="w-full px-4 py-2.5 rounded-full bg-ink text-ivory text-[13px] font-medium hover:bg-ink-900 disabled:opacity-40 transition-colors"
            >
              {save.isPending ? 'Saving…' : 'Save & apply'}
            </button>
            {dirty && !saved && <p className="text-[12px] text-amber-600 font-medium mt-2 text-center">Unsaved changes</p>}
            {saved && <p className="text-[12px] text-emerald-700 font-medium mt-2 text-center">Saved — live now.</p>}
          </div>
        </nav>

        {/* Active section panel */}
        <div className="flex-1 min-w-0 rounded-lex-lg border border-stone-200 bg-white shadow-lex-sm p-6">
          {section === 'branding' && (
            <>
              <h2 className="font-display text-lg">Branding</h2>
              <div className="grid sm:grid-cols-2 gap-4 mt-4">
                <label className="block">
                  <span className={labelCls}>Product name</span>
                  <input className={`${inputCls} mt-1.5`} value={form.brand.product_name}
                    onChange={(e) => set({ brand: { ...form.brand, product_name: e.target.value } })} />
                </label>
                <label className="block">
                  <span className={labelCls}>Tagline</span>
                  <input className={`${inputCls} mt-1.5`} value={form.brand.tagline}
                    onChange={(e) => set({ brand: { ...form.brand, tagline: e.target.value } })} />
                </label>
              </div>
              <p className="text-[12.5px] text-stone-500 mt-3">Shown in the user panel sidebar and browser title. Applies on the user's next page load.</p>
            </>
          )}

          {section === 'answering' && (
            <>
              <h2 className="font-display text-lg">How answers work</h2>
              <label className="block mt-4">
                <span className={labelCls}>Default retrieval mode (users never see this choice)</span>
                <select className={`${inputCls} mt-1.5`} value={form.default_mode} onChange={(e) => set({ default_mode: e.target.value })}>
                  {MODES.map((m) => <option key={m.id} value={m.id}>{m.label}</option>)}
                </select>
              </label>
              <p className="text-[12.5px] text-stone-500 mt-2">{MODES.find((m) => m.id === form.default_mode)?.hint}</p>
              <div className="border-t border-stone-100 mt-6 pt-6">
                {promptField({
                  key: 'ask',
                  label: 'Ask — answer prompt',
                  rows: 8,
                  hint: 'The system prompt behind every Ask answer — shown below is the exact text in use. Edit it and save to customize; Reset returns to the built-in default.',
                })}
              </div>
            </>
          )}

          {section === 'surfaces' && (
            <>
              <h2 className="font-display text-lg">Prompts — AI surfaces</h2>
              <p className="text-[12.5px] text-stone-500 mt-1.5">
                Every editor is prefilled with the exact prompt currently in use — the built-in default,
                or your saved customization. Edit and save to change it; Reset restores the built-in text.
              </p>
              <div className="mt-2">{SURFACE_PROMPTS.map(promptField)}</div>
            </>
          )}

          {section === 'models' && (
            <>
              <h2 className="font-display text-lg">Models</h2>
              <div className="grid sm:grid-cols-2 gap-4 mt-4">
                <label className="block">
                  <span className={labelCls}>LLM model (live swap)</span>
                  <input className={`${inputCls} mt-1.5`} value={form.llm_model ?? ''} placeholder={eff.llm_model}
                    onChange={(e) => set({ llm_model: e.target.value || null })} />
                  <span className="block text-[12.5px] text-stone-500 mt-1.5">In effect now: <span className="font-mono text-[12px]">{eff.llm_model}</span>. Saving swaps it immediately — no restart.</span>
                </label>
                <label className="block">
                  <span className={labelCls}>Embedding model (applies on restart)</span>
                  <input className={`${inputCls} mt-1.5`} value={form.embedding_model ?? ''} placeholder={eff.embedding_model}
                    onChange={(e) => set({ embedding_model: e.target.value || null })} />
                  <span className="block text-[12.5px] text-stone-500 mt-1.5">In effect now: <span className="font-mono text-[12px]">{eff.embedding_model}</span>. Changing it requires a restart <em>and</em> re-indexing the corpus — stored vectors are not interchangeable between models.</span>
                </label>
              </div>
            </>
          )}

          {section === 'tabs' && (
            <>
              <div className="flex items-center justify-between">
                <h2 className="font-display text-lg">Sidebar tabs</h2>
                <button
                  onClick={() => set({ tabs: [...form.tabs, { id: `tab-${Date.now()}`, label: 'New tab', kind: 'page', content: '', visible: true }] })}
                  className="inline-flex items-center gap-1.5 rounded-full border border-stone-200 px-3 py-1.5 text-[12.5px] text-stone-600 hover:text-ink hover:border-stone-300 transition-colors"
                >
                  <Plus size={13} /> Add tab
                </button>
              </div>
              <p className="text-[12.5px] text-stone-500 mt-2">Add your own pages (markdown — firm policies, chamber notices, help) or links (your DMS, court websites) to the user panel sidebar.</p>
              <div className="space-y-4 mt-4">
                {form.tabs.map((t, i) => (
                  <div key={t.id} className="rounded-lex border border-stone-200 p-4">
                    <div className="flex gap-3 flex-wrap items-end">
                      <label className="block flex-1 min-w-[160px]">
                        <span className={labelCls}>Label</span>
                        <input className={`${inputCls} mt-1`} value={t.label} onChange={(e) => setTab(i, { label: e.target.value })} />
                      </label>
                      <label className="block">
                        <span className={labelCls}>Type</span>
                        <select className={`${inputCls} mt-1`} value={t.kind} onChange={(e) => setTab(i, { kind: e.target.value as UiTab['kind'] })}>
                          <option value="page">Page (markdown)</option>
                          <option value="link">External link</option>
                        </select>
                      </label>
                      <label className="flex items-center gap-2 pb-2.5 text-[13px] text-stone-600">
                        <input type="checkbox" checked={t.visible} onChange={(e) => setTab(i, { visible: e.target.checked })} /> Visible
                      </label>
                      <button onClick={() => set({ tabs: form.tabs.filter((_, j) => j !== i) })} className="pb-2 text-stone-400 hover:text-[#a02c2c]" title="Remove tab">
                        <Trash2 size={16} />
                      </button>
                    </div>
                    {t.kind === 'link' ? (
                      <input className={`${inputCls} mt-3`} placeholder="https://…" value={t.url ?? ''} onChange={(e) => setTab(i, { url: e.target.value })} />
                    ) : (
                      <textarea className={`${inputCls} mt-3 resize-y`} rows={4} placeholder="# Page title&#10;&#10;Write in markdown…" value={t.content ?? ''} onChange={(e) => setTab(i, { content: e.target.value })} />
                    )}
                  </div>
                ))}
                {form.tabs.length === 0 && <p className="text-[13px] text-stone-400">No custom tabs yet.</p>}
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  )
}
