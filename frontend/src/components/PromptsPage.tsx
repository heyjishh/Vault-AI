import { useMemo, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import {
  BookOpen, Eye, FolderOpen, Pencil, Plus, Search, Share2, Star, Trash2, X,
} from 'lucide-react'
import { promptsApi, type PromptItem } from '../api/client'
import { useSession } from '../stores/session'
import { ConfirmDialog } from './ui'
import { promptInGroup } from './PromptPicker'

/** Piece 63: the Prompt Library page — the sidebar entry point JISHAN
 *  asked for, modelled on the reference (Lucio) library: a curated
 *  Juris Vault Collection by category, plus My Prompts (Created By
 *  Me / Starred / Shared With Me), search, Preview and Use. Use seeds
 *  the Ask composer. */
export default function PromptsPage({ go }: { go: (view: string) => void }) {
  const qc = useQueryClient()
  const toast = useSession((s) => s.toast)
  const { data, isLoading } = useQuery({ queryKey: ['prompts'], queryFn: promptsApi.list })
  const [group, setGroup] = useState('Suggested Prompts')
  const [q, setQ] = useState('')
  const [previewId, setPreviewId] = useState<string | null>(null)
  const [editing, setEditing] = useState<PromptItem | 'new' | null>(null)
  const [deleting, setDeleting] = useState<PromptItem | null>(null)

  const prompts = data?.prompts ?? []
  const categories = data?.categories ?? ['Suggested Prompts']
  const counts = useMemo(() => {
    const map = new Map<string, number>()
    for (const p of prompts) {
      if (p.suggested) map.set('Suggested Prompts', (map.get('Suggested Prompts') ?? 0) + 1)
      map.set(p.category, (map.get(p.category) ?? 0) + 1)
      if (p.mine) map.set('Created By Me', (map.get('Created By Me') ?? 0) + 1)
      if (p.starred) map.set('Starred', (map.get('Starred') ?? 0) + 1)
      if (!p.mine && p.owner_email) map.set('Shared With Me', (map.get('Shared With Me') ?? 0) + 1)
    }
    return map
  }, [prompts])

  const groupKey = group === 'Suggested Prompts' ? 'Suggested' : group
  const filtered = useMemo(() => {
    const needle = q.trim().toLowerCase()
    return prompts.filter(
      (p) => promptInGroup(p, groupKey) && (!needle || p.title.toLowerCase().includes(needle) || p.body.toLowerCase().includes(needle)),
    )
  }, [prompts, groupKey, q])

  const starMut = useMutation({
    mutationFn: (id: string) => promptsApi.star(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['prompts'] }),
  })
  const shareMut = useMutation({
    mutationFn: (id: string) => promptsApi.share(id),
    onSuccess: (active) => {
      qc.invalidateQueries({ queryKey: ['prompts'] })
      toast('success', active ? 'Prompt shared with your workspace.' : 'Prompt is private again.')
    },
    onError: (e) => toast('error', e instanceof Error ? e.message : 'Could not update sharing.'),
  })
  const deleteMut = useMutation({
    mutationFn: (id: string) => promptsApi.remove(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['prompts'] })
      setDeleting(null)
      toast('success', 'Prompt deleted.')
    },
    onError: (e) => toast('error', e instanceof Error ? e.message : 'Could not delete the prompt.'),
  })

  function usePrompt(p: PromptItem) {
    sessionStorage.setItem('jurisvault.promptSeed', p.body)
    go('assistant')
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
            <BookOpen size={24} /> Prompt Library
          </h1>
          <p className="text-stone-500 dark:text-stone-400 mt-1.5 text-[15px]">
            Explore Juris Vault curated prompts or create your own. “Use” drops the prompt into Knowledge, ready to send.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <div className="relative">
            <Search size={14} className="absolute left-3 top-1/2 -translate-y-1/2 text-stone-400" />
            <input
              value={q} onChange={(e) => setQ(e.target.value)}
              placeholder="Search all prompts"
              className="w-60 rounded-lex-lg border border-stone-300 dark:border-white/10 bg-white dark:bg-[#0a1a12] pl-8 pr-3 py-2 text-[13.5px] outline-none focus:border-ink placeholder:text-stone-400"
            />
          </div>
          <button
            onClick={() => setEditing('new')}
            className="btn-brand flex items-center gap-1.5 rounded-lex-lg text-ivory px-4 py-2 text-[13.5px] font-medium"
          >
            <Plus size={15} /> New Prompt
          </button>
        </div>
      </div>

      <div className="flex gap-6 mt-6 items-start">
        {/* Category rail */}
        <aside className="w-60 shrink-0 rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] p-3 hidden md:block">
          <p className="px-3 pt-1 pb-1.5 text-[11.5px] font-medium uppercase tracking-wide text-stone-400 dark:text-stone-500">Juris Vault Collection</p>
          <div className="space-y-0.5">
            {railItem('Suggested Prompts', <Star size={13} className="text-stone-400 shrink-0" />)}
            {categories.filter((c) => c !== 'Suggested Prompts').map((c) => railItem(c))}
          </div>
          <p className="px-3 pt-4 pb-1.5 text-[11.5px] font-medium uppercase tracking-wide text-stone-400 dark:text-stone-500">My Prompts</p>
          <div className="space-y-0.5">
            {railItem('Created By Me', <Pencil size={13} className="text-stone-400 shrink-0" />)}
            {railItem('Starred', <Star size={13} className="text-stone-400 shrink-0" />)}
            {railItem('Shared With Me', <Share2 size={13} className="text-stone-400 shrink-0" />)}
          </div>
        </aside>

        {/* Prompt cards */}
        <div className="flex-1 min-w-0 rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] divide-y divide-stone-100 dark:divide-white/5">
          <div className="px-5 py-3.5 flex items-center gap-2">
            <h2 className="font-display text-[17px]">{group}</h2>
            <span className="text-[12px] text-stone-400 dark:text-stone-500">{filtered.length} prompt{filtered.length === 1 ? '' : 's'}</span>
            {/* Mobile group switch (the rail is hidden on small screens) */}
            <select
              value={group} onChange={(e) => setGroup(e.target.value)}
              className="md:hidden ml-auto rounded-lex border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] px-2 py-1.5 text-[12.5px] outline-none"
            >
              {['Suggested Prompts', ...categories.filter((c) => c !== 'Suggested Prompts'), 'Created By Me', 'Starred', 'Shared With Me'].map((g) => (
                <option key={g} value={g}>{g}</option>
              ))}
            </select>
          </div>
          {isLoading && <p className="px-5 py-8 text-[13.5px] text-stone-400">Loading prompts…</p>}
          {!isLoading && filtered.length === 0 && (
            <div className="px-5 py-10 text-center">
              <p className="text-[14px] font-medium">Nothing here yet</p>
              <p className="text-[13px] text-stone-500 dark:text-stone-400 mt-1">
                {group === 'Created By Me'
                  ? 'Create your first prompt — it will live here and in the Prompts button inside Knowledge and Intelligence.'
                  : group === 'Shared With Me'
                    ? 'When a colleague shares a prompt, it appears here.'
                    : group === 'Starred'
                      ? 'Star any prompt and it will be pinned here.'
                      : 'No prompts match this view.'}
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
                  onClick={() => usePrompt(p)}
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
        <PromptForm
          initial={editing === 'new' ? null : editing}
          categories={categories.filter((c) => c !== 'Suggested Prompts')}
          onClose={() => setEditing(null)}
          onSaved={() => { setEditing(null); qc.invalidateQueries({ queryKey: ['prompts'] }) }}
        />
      )}
      {deleting && (
        <ConfirmDialog
          title={`Delete “${deleting.title}”?`}
          body="This prompt will be permanently removed from your library. Starred copies in other views go with it."
          confirmLabel="Delete permanently"
          onConfirm={() => deleteMut.mutate(deleting.id)}
          onCancel={() => setDeleting(null)}
        />
      )}
    </div>
  )
}

function PromptForm({ initial, categories, onClose, onSaved }: {
  initial: PromptItem | null
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
      if (initial) return promptsApi.update(initial.id, { title: title.trim(), body: body.trim(), category: category.trim() || 'General' })
      return promptsApi.create(title.trim(), body.trim(), category.trim() || 'General')
    },
    onSuccess: () => { toast('success', initial ? 'Prompt updated.' : 'Prompt created.'); onSaved() },
    onError: (e) => toast('error', e instanceof Error ? e.message : 'Could not save the prompt.'),
  })
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/45 p-4" onClick={onClose}>
      <div
        className="pop-in w-full max-w-xl rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] shadow-lex-md p-5"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between">
          <h2 className="font-display text-lg">{initial ? 'Edit prompt' : 'New Prompt'}</h2>
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
            value={category} onChange={(e) => setCategory(e.target.value)} list="prompt-categories"
            placeholder="Contracts, Drafting, Research…"
            className="mt-1.5 w-full rounded-lex border border-stone-300 dark:border-white/10 bg-white dark:bg-[#0a1a12] px-3 py-2 text-[14px] outline-none focus:border-ink placeholder:text-stone-400"
          />
          <datalist id="prompt-categories">
            {categories.map((c) => <option key={c} value={c} />)}
            <option value="General" />
          </datalist>
        </label>
        <label className="block mt-3">
          <span className="text-[12.5px] font-medium text-stone-500 dark:text-stone-400">Prompt</span>
          <textarea
            value={body} onChange={(e) => setBody(e.target.value)} rows={7}
            placeholder="Write the instruction. Use [brackets] for details you'll fill in each time — e.g. Review the agreement for [client name] and…"
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
            {saveMut.isPending ? 'Saving…' : initial ? 'Save changes' : 'Create prompt'}
          </button>
        </div>
      </div>
    </div>
  )
}
