import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Briefcase, Plus, Trash2, Users, X } from 'lucide-react'
import { authApi, mattersApi, spacesApi } from '../api/client'
import type { MatterSummary, Space } from '../api/types'
import { useSession } from '../stores/session'
import { EmptyState, Pager, Spinner, usePagination } from './ui'

const KINDS = ['Litigation', 'Corporate', 'Tax', 'Advisory', 'Property', 'General']

export function StatusPill({ status }: { status: string }) {
  const cls =
    status === 'Active'
      ? 'bg-emerald-700/10 text-emerald-800'
      : status === 'On hold'
        ? 'bg-amber-600/10 text-amber-800'
        : 'bg-stone-500/10 text-stone-500'
  return (
    <span className={`text-[11px] font-medium rounded-full px-2 py-0.5 ${cls}`}>{status}</span>
  )
}

export default function MattersPage({ open }: { open: (id: string) => void }) {
  const toast = useSession((s) => s.toast)
  const qc = useQueryClient()
  const { data: matters, isLoading } = useQuery({ queryKey: ['matters'], queryFn: mattersApi.list })
  const { data: spaces } = useQuery({ queryKey: ['spaces'], queryFn: spacesApi.list })
  const { data: me } = useQuery({ queryKey: ['me'], queryFn: authApi.me })
  const [creating, setCreating] = useState(false)
  const [title, setTitle] = useState('')
  const [client, setClient] = useState('')
  const [kind, setKind] = useState('Litigation')
  const [notes, setNotes] = useState('')
  const [spaceId, setSpaceId] = useState('')
  const [briefQuery, setBriefQuery] = useState('')
  const [spaceFilter, setSpaceFilter] = useState<'all' | 'none' | string>('all')
  const filteredMatters = (matters ?? []).filter((x) => {
    if (!x.title.toLowerCase().includes(briefQuery.trim().toLowerCase())) return false
    if (spaceFilter === 'none') return !x.space_id
    if (spaceFilter !== 'all') return x.space_id === spaceFilter
    return true
  })
  const pagedMatters = usePagination(filteredMatters, 6)
  const spaceName = (id?: string | null) => (spaces ?? []).find((s) => s.id === id)?.name

  const create = useMutation({
    mutationFn: () => mattersApi.create(title.trim(), client.trim(), kind, notes.trim(), spaceId || undefined),
    onSuccess: (m) => {
      qc.invalidateQueries({ queryKey: ['matters'] })
      setCreating(false)
      setTitle(''); setClient(''); setNotes(''); setSpaceId('')
      toast('success', 'Brief opened')
      open(m.id)
    },
    onError: (e) => toast('error', (e as Error).message),
  })

  const remove = useMutation({
    mutationFn: (id: string) => mattersApi.remove(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['matters'] })
      toast('success', 'Brief deleted — its documents stay in the Vault')
    },
    onError: (e) => toast('error', (e as Error).message),
  })

  return (
    <div className="max-w-5xl mx-auto px-6 py-8">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h1 className="font-display text-[2rem] tracking-tight">Briefs</h1>
          <p className="text-stone-500 mt-1.5 text-[15px] max-w-xl">
            Every brief is a living workspace — its documents, the entities they mention, and
            everything asked, drafted, and filed inside it. The brief remembers, so you don't start cold.
          </p>
        </div>
        <button
          onClick={() => setCreating((c) => !c)}
          className="flex items-center gap-2 rounded-lex bg-ink text-ivory px-3.5 py-2.5 text-[14px] font-medium hover:bg-ink-900 transition-colors shadow-lex whitespace-nowrap"
        >
          {creating ? <X size={15} /> : <Plus size={15} />}
          {creating ? 'Cancel' : 'New brief'}
        </button>
      </div>

      {creating && (
        <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-6 py-5 mt-6">
          <div className="grid sm:grid-cols-2 gap-4">
            <label className="block">
              <span className="text-[12px] uppercase tracking-wider text-stone-400 font-medium">Brief title</span>
              <input
                value={title} onChange={(e) => setTitle(e.target.value)}
                placeholder="e.g. Verma v. Kanpur Development Authority"
                className="mt-1.5 w-full rounded-lex border border-stone-200 px-3 py-2 text-[14px] outline-none focus:border-stone-400"
              />
            </label>
            <label className="block">
              <span className="text-[12px] uppercase tracking-wider text-stone-400 font-medium">Client</span>
              <input
                value={client} onChange={(e) => setClient(e.target.value)}
                placeholder="Client name"
                className="mt-1.5 w-full rounded-lex border border-stone-200 px-3 py-2 text-[14px] outline-none focus:border-stone-400"
              />
            </label>
            <label className="block">
              <span className="text-[12px] uppercase tracking-wider text-stone-400 font-medium">Type</span>
              <select
                value={kind} onChange={(e) => setKind(e.target.value)}
                className="mt-1.5 w-full rounded-lex border border-stone-200 px-3 py-2 text-[14px] bg-white outline-none focus:border-stone-400"
              >
                {KINDS.map((k) => <option key={k}>{k}</option>)}
              </select>
            </label>
            <label className="block">
              <span className="text-[12px] uppercase tracking-wider text-stone-400 font-medium">Notes</span>
              <input
                value={notes} onChange={(e) => setNotes(e.target.value)}
                placeholder="Optional — key facts, forum, next date…"
                className="mt-1.5 w-full rounded-lex border border-stone-200 px-3 py-2 text-[14px] outline-none focus:border-stone-400"
              />
            </label>
            <label className="block">
              <span className="text-[12px] uppercase tracking-wider text-stone-400 font-medium">Space</span>
              <select
                value={spaceId} onChange={(e) => setSpaceId(e.target.value)}
                className="mt-1.5 w-full rounded-lex border border-stone-200 px-3 py-2 text-[14px] bg-white outline-none focus:border-stone-400"
              >
                <option value="">No space — visible to everyone signed in</option>
                {(spaces ?? []).map((s) => <option key={s.id} value={s.id}>{s.name} — members only</option>)}
              </select>
            </label>
          </div>
          <button
            disabled={!title.trim() || create.isPending}
            onClick={() => create.mutate()}
            className="mt-4 rounded-lex bg-ink text-ivory px-4 py-2 text-[14px] font-medium disabled:opacity-40 hover:bg-ink-900 transition-colors"
          >
            {create.isPending ? 'Opening…' : 'Open brief'}
          </button>
        </div>
      )}

      <div className="mt-8">
        {matters && matters.length > 0 && (
          <div className="flex items-center justify-between gap-3 mb-3 flex-wrap">
            <h2 className="font-display text-xl tracking-tight">Your briefs</h2>
            <input
              value={briefQuery}
              onChange={(e) => setBriefQuery(e.target.value)}
              placeholder="Search briefs…"
              className="rounded-full border border-stone-200 bg-white px-3.5 py-1.5 text-[13px] outline-none focus:border-stone-400 w-56"
            />
          </div>
        )}
        {spaces && spaces.length > 0 && (
          <div className="flex items-center gap-1.5 mb-4 flex-wrap">
            {(['all', 'none'] as const).map((f) => (
              <button
                key={f}
                onClick={() => setSpaceFilter(f)}
                className={`rounded-full border px-2.5 py-1 text-[12px] transition-colors ${
                  spaceFilter === f ? 'bg-ink text-ivory border-ink' : 'border-stone-200 text-stone-500 hover:text-ink hover:border-stone-300'
                }`}
              >
                {f === 'all' ? 'All briefs' : 'No space'}
              </button>
            ))}
            {spaces.map((s) => (
              <button
                key={s.id}
                onClick={() => setSpaceFilter(s.id)}
                className={`flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-[12px] transition-colors ${
                  spaceFilter === s.id ? 'bg-ink text-ivory border-ink' : 'border-stone-200 text-stone-500 hover:text-ink hover:border-stone-300'
                }`}
              >
                <Users size={11} /> {s.name}
              </button>
            ))}
          </div>
        )}
        {isLoading ? (
          <Spinner label="Loading briefs…" />
        ) : !matters || matters.length === 0 ? (
          <EmptyState title="No briefs yet" hint="Open your first brief to start filing documents and work inside it." />
        ) : filteredMatters.length === 0 ? (
          <EmptyState title="No briefs match" hint="Try a different search term." />
        ) : (
          <>
          <div className="grid sm:grid-cols-2 gap-3">
            {pagedMatters.pageItems.map((m) => (
              <MatterCard key={m.id} matter={m} spaceName={spaceName(m.space_id)} onOpen={() => open(m.id)} onDelete={() => {
                if (window.confirm(`Delete brief “${m.title}”? Its documents stay in the Vault.`)) remove.mutate(m.id)
              }} />
            ))}
          </div>
          <Pager page={pagedMatters.page} pages={pagedMatters.pages} from={pagedMatters.from} to={pagedMatters.to} total={pagedMatters.total} onPage={pagedMatters.setPage} />
          </>
        )}
      </div>

      <SpacesPanel spaces={spaces ?? []} myId={me?.id} />
    </div>
  )
}

function SpacesPanel({ spaces, myId }: { spaces: Space[]; myId?: string }) {
  const toast = useSession((s) => s.toast)
  const qc = useQueryClient()
  const [name, setName] = useState('')
  const [openId, setOpenId] = useState<string | null>(null)
  const [memberEmail, setMemberEmail] = useState('')
  const [memberRole, setMemberRole] = useState('viewer')
  const refresh = () => {
    qc.invalidateQueries({ queryKey: ['spaces'] })
    qc.invalidateQueries({ queryKey: ['matters'] })
  }
  const createSpace = useMutation({
    mutationFn: () => spacesApi.create(name.trim()),
    onSuccess: () => { refresh(); setName(''); toast('success', 'Space created — you are its owner') },
    onError: (e) => toast('error', (e as Error).message),
  })
  const addMember = useMutation({
    mutationFn: (spaceId: string) => spacesApi.addMember(spaceId, memberEmail.trim(), memberRole),
    onSuccess: () => { refresh(); setMemberEmail(''); toast('success', 'Member added') },
    onError: (e) => toast('error', (e as Error).message),
  })
  const removeMember = useMutation({
    mutationFn: (v: { spaceId: string; userId: string }) => spacesApi.removeMember(v.spaceId, v.userId),
    onSuccess: () => { refresh(); toast('success', 'Member removed') },
    onError: (e) => toast('error', (e as Error).message),
  })
  const deleteSpace = useMutation({
    mutationFn: (id: string) => spacesApi.remove(id),
    onSuccess: () => { refresh(); setOpenId(null); toast('success', 'Space deleted — its briefs moved out and kept') },
    onError: (e) => toast('error', (e as Error).message),
  })

  const inputCls = 'rounded-lex border border-stone-200 px-3 py-2 text-[14px] outline-none focus:border-stone-400 bg-white'
  return (
    <div className="mt-12">
      <h2 className="font-display text-xl tracking-tight">Spaces</h2>
      <p className="text-stone-500 mt-1 text-[13.5px] max-w-xl">
        A space is a private container for a team or client. Briefs filed in a space — and questions
        asked inside them — are visible only to its members.
      </p>
      <div className="mt-4 flex gap-2 max-w-md">
        <input
          value={name} onChange={(e) => setName(e.target.value)}
          placeholder="New space name — e.g. Verma litigation team"
          className={`${inputCls} flex-1`}
        />
        <button
          disabled={!name.trim() || createSpace.isPending}
          onClick={() => createSpace.mutate()}
          className="rounded-lex bg-ink text-ivory px-3.5 py-2 text-[13.5px] font-medium disabled:opacity-40 whitespace-nowrap"
        >
          Create space
        </button>
      </div>
      <div className="mt-4 space-y-2.5">
        {spaces.map((s) => {
          const myRole = s.members.find((m) => m.user_id === myId)?.role
          const iOwn = s.owner_id === myId
          const expanded = openId === s.id
          return (
            <div key={s.id} className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-5 py-4">
              <div className="flex items-center gap-3 flex-wrap">
                <span className="w-8 h-8 rounded-lex bg-stone-100 flex items-center justify-center">
                  <Users size={14} className="text-stone-500" />
                </span>
                <p className="font-medium text-[15px]">{s.name}</p>
                <span className="text-[12px] text-stone-500">
                  {s.members.length} member{s.members.length === 1 ? '' : 's'}{myRole ? ` · you are ${myRole}` : ''}
                </span>
                <span className="ml-auto flex items-center gap-2">
                  <button
                    onClick={() => setOpenId(expanded ? null : s.id)}
                    className="text-[12.5px] text-stone-500 hover:text-ink underline underline-offset-2"
                  >
                    {expanded ? 'Close' : 'Members'}
                  </button>
                  {iOwn && (
                    <button
                      onClick={() => { if (window.confirm(`Delete space “${s.name}”? Its briefs are kept and moved out of the space.`)) deleteSpace.mutate(s.id) }}
                      className="text-[12.5px] text-stone-400 hover:text-[#a02c2c]"
                    >
                      Delete
                    </button>
                  )}
                </span>
              </div>
              {expanded && (
                <div className="mt-3.5 border-t border-stone-100 pt-3.5">
                  <div className="space-y-1.5">
                    {s.members.map((m) => (
                      <div key={m.user_id} className="flex items-center gap-2.5 text-[13px]">
                        <span className="text-ink">{m.email}</span>
                        <span className="rounded-full bg-stone-100 border border-stone-200 px-1.5 py-px text-[10px] font-semibold uppercase tracking-wide text-stone-600">
                          {m.role}
                        </span>
                        {iOwn && m.user_id !== s.owner_id && (
                          <button
                            onClick={() => removeMember.mutate({ spaceId: s.id, userId: m.user_id })}
                            className="ml-auto text-stone-400 hover:text-[#a02c2c] text-[12px]"
                          >
                            Remove
                          </button>
                        )}
                      </div>
                    ))}
                  </div>
                  {iOwn && (
                    <div className="mt-3.5 flex gap-2 flex-wrap">
                      <input
                        value={memberEmail} onChange={(e) => setMemberEmail(e.target.value)}
                        placeholder="Member's account email"
                        className={`${inputCls} flex-1 min-w-48 !py-1.5 text-[13px]`}
                      />
                      <select
                        value={memberRole} onChange={(e) => setMemberRole(e.target.value)}
                        className="rounded-lex border border-stone-200 px-2 py-1.5 text-[13px] bg-white outline-none"
                      >
                        <option value="viewer">Viewer — read + ask</option>
                        <option value="editor">Editor — + upload & edit</option>
                      </select>
                      <button
                        disabled={!memberEmail.trim() || addMember.isPending}
                        onClick={() => addMember.mutate(s.id)}
                        className="rounded-lex bg-ink text-ivory px-3 py-1.5 text-[13px] font-medium disabled:opacity-40"
                      >
                        Add member
                      </button>
                    </div>
                  )}
                  {!iOwn && (
                    <p className="mt-2 text-[12px] text-stone-400">Only the space owner can change membership.</p>
                  )}
                </div>
              )}
            </div>
          )
        })}
        {spaces.length === 0 && (
          <p className="text-[13px] text-stone-400">No spaces yet — create one above to work privately with a team.</p>
        )}
      </div>
    </div>
  )
}

function MatterCard({ matter: m, spaceName, onOpen, onDelete }: { matter: MatterSummary; spaceName?: string; onOpen: () => void; onDelete: () => void }) {
  return (
    <div
      onClick={onOpen}
      className="text-left rounded-lex-lg border border-stone-200 bg-white shadow-lex px-5 py-4 hover:border-stone-300 hover:shadow-lex-md transition-all cursor-pointer group"
    >
      <div className="flex items-start gap-3">
        <div className="w-9 h-9 rounded-lex bg-stone-100 flex items-center justify-center flex-shrink-0">
          <Briefcase size={16} className="text-stone-500" />
        </div>
        <div className="min-w-0 flex-1">
          <p className="font-display text-[17px] leading-snug truncate group-hover:text-ink">{m.title}</p>
          <p className="text-[13px] text-stone-500 truncate">
            {[m.client, m.kind].filter(Boolean).join(' · ') || '—'}
          </p>
        </div>
        <StatusPill status={m.status} />
      </div>
      <div className="mt-3.5 flex items-center gap-4 text-[12px] text-stone-500">
        <span>{m.document_count} documents</span>
        <span>{m.activity_count} activities</span>
        {spaceName && (
          <span className="flex items-center gap-1 rounded-full bg-stone-100 border border-stone-200 px-1.5 py-px text-[10px] font-semibold uppercase tracking-wide text-stone-600">
            <Users size={10} /> {spaceName}
          </span>
        )}
        <span className="ml-auto">
          {new Date(m.updated_at).toLocaleDateString('en-IN', { day: 'numeric', month: 'short', year: 'numeric' })}
        </span>
        {m.is_default && <span className="text-[10px] font-semibold uppercase tracking-wide bg-stone-100 rounded px-1.5 py-0.5">Default</span>}
        {!m.is_default && (
          <button
            onClick={(e) => { e.stopPropagation(); onDelete() }}
            title="Delete brief"
            className="text-stone-400 hover:text-[#a02c2c] p-0.5"
          >
            <Trash2 size={13} />
          </button>
        )}
      </div>
    </div>
  )
}
