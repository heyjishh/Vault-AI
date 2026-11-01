import { useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  LayoutTemplate, Eye, FolderOpen, Pencil, Plus, Search, Share2, Star, Trash2, X,
} from 'lucide-react'
import { templatesApi, type TemplateItem } from '../api/client'
import { useSession } from '../stores/session'
import { ConfirmDialog } from './ui'
import { templateInGroup } from './TemplatePicker'

/** Piece 68: the Template Library page — the drafting twin of the
 *  Prompt Library, same pattern at JISHAN's direction: a curated
 *  Juris Vault Collection by category, plus My Templates (Created
 *  By Me / Starred / Shared With Me), search, Preview and Use.
 *  Templates exist for Draft only: Use seeds the Draft composer. */
export default function TemplatesPage({ go }: { go: (view: string) => void }) {
  const qc = useQueryClient()
  const toast = useSession((s) => s.toast)
  const { data, isLoading } = useQuery({ queryKey: ['templates'], queryFn: templatesApi.list })
  const [group, setGroup] = useState('Suggested Templates')
  const [q, setQ] = useState('')
  const [previewId, setPreviewId] = useState<string | null>(null)
  const [editing, setEditing] = useState<TemplateItem | 'new' | null>(null)
  const [deleting, setDeleting] = useState<TemplateItem | null>(null)

  const templates = data?.templates ?? []
  const categories = data?.categories ?? ['Suggested Templates']
  const counts = useMemo(() => {
    const map = new Map<string, number>()
    for (const p of templates) {
      if (p.suggested) map.set('Suggested Templates', (map.get('Suggested Templates') ?? 0) + 1)
      map.set(p.category, (map.get(p.category) ?? 0) + 1)
      if (p.mine) map.set('Created By Me', (map.get('Created By Me') ?? 0) + 1)
      if (p.starred) map.set('Starred', (map.get('Starred') ?? 0) + 1)
      if (!p.mine && p.owner_email) map.set('Shared With Me', (map.get('Shared With Me') ?? 0) + 1)
    }
    return map
  }, [templates])

  const groupKey = group === 'Suggested Templates' ? 'Suggested' : group
  const filtered = useMemo(() => {
    const needle = q.trim().toLowerCase()
    return templates.filter(
      (p) => templateInGroup(p, groupKey) && (!needle || p.title.toLowerCase().includes(needle) || p.body.toLowerCase().includes(needle)),
    )
  }, [templates, groupKey, q])

  const starMut = useMutation({
    mutationFn: (id: string) => templatesApi.star(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['templates'] }),
  })
  const shareMut = useMutation({
    mutationFn: (id: string) => templatesApi.share(id),
    onSuccess: (active) => {
      qc.invalidateQueries({ queryKey: ['templates'] })
      toast('success', active ? 'Template shared with your workspace.' : 'Template is private again.')
    },
    onError: (e) => toast('error', e instanceof Error ? e.message : 'Could not update sharing.'),
  })
  const deleteMut = useMutation({
    mutationFn: (id: string) => templatesApi.remove(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['templates'] })
      setDeleting(null)
      toast('success', 'Template deleted.')
    },
    onError: (e) => toast('error', e instanceof Error ? e.message : 'Could not delete the template.'),
  })

  function useTemplate(p: TemplateItem) {
    sessionStorage.setItem('jurisvault.templateSeed', p.body)
    go('draft')
  }

  const railItem = (label: string, icon?: React.ReactNode) => (
    <button
      key={label}
      onClick={() => setGroup(label)}
      className={`w-full flex items-center gap-2 rounded-lex px-3 py-1.5 text-[13.5px] text-left transition-colors ${
        group === label
          ? 'bg-stone-100 dark:bg-[#122419] font-medium shadow-[inset_2px_0_0_var(--bronze)]'
          : 'text-stone-600 dark:text-stone-300 hover:bg-stone-100/70 dark:hover:bg-white/5'
      }`}
    >
      {icon ?? <FolderOpen size={13} className="text-stone-400 shrink-0" />}
      <span className="truncate">{label}</span>
      <span className="ml-auto text-[11px] text-stone-400 dark:text-stone-500 tabular-nums">{counts.get(label) ?? 0}</span>
    </button>
  )

  return (
    <div className="w-full px-6 py-8">
      <div className="flex items-start justify-between gap-4 flex-wrap">
        <div>
          <h1 className="font-display text-[2rem] tracking-tight flex items-center gap-2.5">
            <LayoutTemplate size={24} /> Template Library
          </h1>
          <p className="text-stone-500 dark:text-stone-400 mt-1.5 text-[15px]">
            Explore Juris Vault curated drafting templates or create your own. “Use” opens Draft with the template loaded into the instructions, ready to generate.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <div className="relative">
            <Search size={14} className="absolute left-3 top-1/2 -translate-y-1/2 text-stone-400" />
            <input
              value={q} onChange={(e) => setQ(e.target.value)}
              placeholder="Search all templates"
              className="w-60 rounded-lex-lg border border-stone-300 dark:border-white/10 bg-white dark:bg-[#0a1a12] pl-8 pr-3 py-2 text-[13.5px] outline-none focus:border-ink placeholder:text-stone-400"
            />
          </div>
          <button
            onClick={() => setEditing('new')}
            className="btn-brand flex items-center gap-1.5 rounded-lex-lg text-ivory px-4 py-2 text-[13.5px] font-medium"
          >
            <Plus size={15} /> New Template
          </button>
        </div>
      </div>

      <div className="flex gap-6 mt-6 items-start">
        {/* Category rail */}
        <aside className="w-60 shrink-0 rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] p-3 hidden md:block">
          <p className="px-3 pt-1 pb-1.5 text-[11.5px] font-medium uppercase tracking-wide text-stone-400 dark:text-stone-500">Juris Vault Collection</p>
          <div className="space-y-0.5">
            {railItem('Suggested Templates', <Star size={13} className="text-stone-400 shrink-0" />)}
            {categories.filter((c) => c !== 'Suggested Templates').map((c) => railItem(c))}
          </div>
          <p className="px-3 pt-4 pb-1.5 text-[11.5px] font-medium uppercase tracking-wide text-stone-400 dark:text-stone-500">My Templates</p>
          <div className="space-y-0.5">
            {railItem('Created By Me', <Pencil size={13} className="text-stone-400 shrink-0" />)}
            {railItem('Starred', <Star size={13} className="text-stone-400 shrink-0" />)}
            {railItem('Shared With Me', <Share2 size={13} className="text-stone-400 shrink-0" />)}
          </div>
        </aside>

        {/* Template cards */}
        <div className="flex-1 min-w-0 rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] divide-y divide-stone-100 dark:divide-white/5">
          <div className="px-5 py-3.5 flex items-center gap-2">
            <h2 className="font-display text-[17px]">{group}</h2>
            <span className="text-[12px] text-stone-400 dark:text-stone-500">{filtered.length} template{filtered.length === 1 ? '' : 's'}</span>
            {/* Mobile group switch (the rail is hidden on small screens) */}
            <select
              value={group} onChange={(e) => setGroup(e.target.value)}
              className="md:hidden ml-auto rounded-lex border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] px-2 py-1.5 text-[12.5px] outline-none"
            >
              {['Suggested Templates', ...categories.filter((c) => c !== 'Suggested Templates'), 'Created By Me', 'Starred', 'Shared With Me'].map((g) => (
                <option key={g} value={g}>{g}</option>
              ))}
            </select>
          </div>
          {isLoading && <p className="px-5 py-8 text-[13.5px] text-stone-400">Loading templates…</p>}
          {!isLoading && filtered.length === 0 && (
            <div className="px-5 py-10 text-center">
              <p className="text-[14px] font-medium">Nothing here yet</p>
              <p className="text-[13px] text-stone-500 dark:text-stone-400 mt-1">
                {group === 'Created By Me'
                  ? 'Create your first template — it will live here and in the + menu inside Draft.'
                  : group === 'Shared With Me'
                    ? 'When a colleague shares a template, it appears here.'
                    : group === 'Starred'
                      ? 'Star any template and it will be pinned here.'
                      : 'No templates match this view.'}
              </p>
            </div>
          )}
          {filtered.map((p) => (
            <div key={p.id} className="px-5 py-4">
              <div className="flex items-start gap-3">
                <div className="min-w-0 flex-1">
                  <p className="text-[15px] font-medium">{p.title}</p>
                  <p className={`text-[13px] text-stone-500 dark:text-stone-400 leading-relaxed mt-1 ${previewId === p.id ? '' : 'line-clamp-2'}`}>
                    {p.body}
                  </p>
                  <p className="text-[11.5px] text-stone-400 dark:text-stone-500 mt-1.5">
                    {p.category}
                    {p.mine ? ' · Yours' : p.owner_email ? ` · Shared by ${p.owner_email}` : ' · Juris Vault collection'}
                    {p.mine && p.shared ? ' · Shared' : ''}
                  </p>
                </div>
                <button
                  onClick={() => starMut.mutate(p.id)}
                  title={p.starred ? 'Unstar' : 'Star'}
                  className={`p-1.5 shrink-0 ${p.starred ? 'text-[#b8860b] dark:text-[#e3c878]' : 'text-stone-300 dark:text-stone-600 hover:text-stone-500'}`}
                >
                  <Star size={16} fill={p.starred ? 'currentColor' : 'none'} />
                </button>
              </div>
              <div className="flex items-center gap-2 mt-3 flex-wrap">
                <button
                  onClick={() => setPreviewId(previewId === p.id ? null : p.id)}
                  className="flex items-center gap-1.5 rounded-lex border border-stone-200 dark:border-white/10 px-3 py-1.5 text-[12.5px] text-stone-600 dark:text-stone-300 hover:text-ink hover:border-stone-300 transition-colors"
                >
                  <Eye size={13} /> {previewId === p.id ? 'Hide' : 'Preview'}
                </button>
                <button
                  onClick={() => useTemplate(p)}
                  className="btn-brand rounded-lex text-ivory px-3.5 py-1.5 text-[12.5px] font-medium"
                >
                  Use
                </button>
                {p.mine && (
                  <>
                    <button
                      onClick={() => setEditing(p)}
                      className="flex items-center gap-1.5 rounded-lex border border-stone-200 dark:border-white/10 px-3 py-1.5 text-[12.5px] text-stone-600 dark:text-stone-300 hover:text-ink hover:border-stone-300 transition-colors"
                    >
                      <Pencil size={13} /> Edit
                    </button>
                    <button
                      onClick={() => shareMut.mutate(p.id)}
                      title={p.shared ? 'Stop sharing' : 'Share with everyone in this workspace'}
                      className={`flex items-center gap-1.5 rounded-lex border px-3 py-1.5 text-[12.5px] transition-colors ${p.shared ? 'bg-ink text-ivory border-ink' : 'border-stone-200 dark:border-white/10 text-stone-600 dark:text-stone-300 hover:text-ink hover:border-stone-300'}`}
                    >
                      <Share2 size={13} /> {p.shared ? 'Shared' : 'Share'}
                    </button>
                    <button
                      onClick={() => setDeleting(p)}
                      className="ml-auto flex items-center gap-1.5 rounded-lex px-2.5 py-1.5 text-[12.5px] text-stone-400 dark:text-stone-500 hover:text-[#a02c2c] transition-colors"
                    >
                      <Trash2 size={13} /> Delete
                    </button>
                  </>
                )}
              </div>
            </div>
          ))}
        </div>
      </div>

      {editing && (
        <TemplateForm
          initial={editing === 'new' ? null : editing}
          categories={categories.filter((c) => c !== 'Suggested Templates')}
          onClose={() => setEditing(null)}
          onSaved={() => { setEditing(null); qc.invalidateQueries({ queryKey: ['templates'] }) }}
        />
      )}
      {deleting && (
        <ConfirmDialog
          title={`Delete “${deleting.title}”?`}
          body="This template will be permanently removed from your library. Starred copies in other views go with it."
          confirmLabel="Delete permanently"
          onConfirm={() => deleteMut.mutate(deleting.id)}
          onCancel={() => setDeleting(null)}
        />
      )}
    </div>
  )
}

function TemplateForm({ initial, categories, onClose, onSaved }: {
  initial: TemplateItem | null
  categories: string[]
  onClose: () => void
  onSaved: () => void
}) {
  const toast = useSession((s) => s.toast)
  const [title, setTitle] = useState(initial?.title ?? '')
  const [category, setCategory] = useState(initial?.category ?? 'General')
  const [body, setBody] = useState(initial?.body ?? '')
  const saveMut = useMutation({
    mutationFn: async () => {
      if (initial) return templatesApi.update(initial.id, { title: title.trim(), body: body.trim(), category: category.trim() || 'General' })
      return templatesApi.create(title.trim(), body.trim(), category.trim() || 'General')
    },
    onSuccess: () => { toast('success', initial ? 'Template updated.' : 'Template created.'); onSaved() },
    onError: (e) => toast('error', e instanceof Error ? e.message : 'Could not save the template.'),
  })
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/45 p-4" onClick={onClose}>
      <div
        className="pop-in w-full max-w-xl rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] shadow-lex-md p-5"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between">
          <h2 className="font-display text-lg">{initial ? 'Edit template' : 'New Template'}</h2>
          <button onClick={onClose} className="p-1.5 rounded-lex text-stone-500 dark:text-stone-400 hover:text-ink hover:bg-stone-100 dark:hover:bg-white/10" title="Close">
            <X size={16} />
          </button>
        </div>
        <label className="block mt-4">
          <span className="text-[12.5px] font-medium text-stone-500 dark:text-stone-400">Title</span>
          <input
            value={title} onChange={(e) => setTitle(e.target.value)} autoFocus
            placeholder="e.g. Notice reply — rent arrears"
            className="mt-1.5 w-full rounded-lex border border-stone-300 dark:border-white/10 bg-white dark:bg-[#0a1a12] px-3 py-2 text-[14px] outline-none focus:border-ink placeholder:text-stone-400"
          />
        </label>
        <label className="block mt-3">
          <span className="text-[12.5px] font-medium text-stone-500 dark:text-stone-400">Category</span>
          <input
            value={category} onChange={(e) => setCategory(e.target.value)} list="template-categories"
            placeholder="Notices, Agreements, Pleadings…"
            className="mt-1.5 w-full rounded-lex border border-stone-300 dark:border-white/10 bg-white dark:bg-[#0a1a12] px-3 py-2 text-[14px] outline-none focus:border-ink placeholder:text-stone-400"
          />
          <datalist id="template-categories">
            {categories.map((c) => <option key={c} value={c} />)}
            <option value="General" />
          </datalist>
        </label>
        <label className="block mt-3">
          <span className="text-[12.5px] font-medium text-stone-500 dark:text-stone-400">Template</span>
          <textarea
            value={body} onChange={(e) => setBody(e.target.value)} rows={7}
            placeholder="Write the template text — structure, clauses and wording. Use [brackets] for details you'll fill in each time — e.g. Legal notice to [tenant] for arrears of…"
            className="mt-1.5 w-full rounded-lex border border-stone-300 dark:border-white/10 bg-white dark:bg-[#0a1a12] px-3 py-2 text-[14px] leading-relaxed outline-none focus:border-ink placeholder:text-stone-400 resize-y"
          />
        </label>
        <div className="flex items-center justify-end gap-2 mt-4">
          <button onClick={onClose} className="rounded-lex border border-stone-300 dark:border-white/10 px-4 py-2 text-[13px] font-medium hover:bg-stone-100 dark:hover:bg-white/10">
            Cancel
          </button>
          <button
            onClick={() => saveMut.mutate()}
            disabled={!title.trim() || !body.trim() || saveMut.isPending}
            className="btn-brand rounded-lex text-ivory px-4 py-2 text-[13px] font-medium disabled:opacity-40"
          >
            {saveMut.isPending ? 'Saving…' : initial ? 'Save changes' : 'Create template'}
          </button>
        </div>
      </div>
    </div>
  )
}
