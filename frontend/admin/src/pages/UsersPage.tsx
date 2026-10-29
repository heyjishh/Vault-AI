import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { Eraser, ShieldCheck, Trash2, User as UserIcon } from 'lucide-react'
import { adminApi, errMsg, type AdminUser, type Me, type UserDataReceipt } from '../api'
import { ErrorBanner, PageHead, Spinner } from '../ui'

/** Permanent user-data purge: shows the pre-delete counts, requires
 *  typing the user's email to confirm, then displays the receipt.
 *  Scope is narrow by design — uploaded documents + history only;
 *  the account, feedback, Private Vault and payment data stay. */
function PurgePanel({ user, onClose }: { user: AdminUser; onClose: () => void }) {
  const [typed, setTyped] = useState('')
  const [receipt, setReceipt] = useState<UserDataReceipt | null>(null)
  const [error, setError] = useState<string | null>(null)
  const summary = useQuery({
    queryKey: ['user-data-summary', user.id],
    queryFn: () => adminApi.userDataSummary(user.id),
  })
  const purge = useMutation({
    mutationFn: () => adminApi.deleteUserData(user.id),
    onSuccess: (r) => { setReceipt(r); setError(null) },
    onError: (e) => setError(errMsg(e)),
  })

  return (
    <div className="mt-6 rounded-lex-lg border border-[#e5c4c4] bg-[#fdf3f3] px-6 py-5">
      <h3 className="font-display text-lg text-[#7a1f1f]">Delete uploaded data of {user.email}</h3>
      {error && <div className="mt-3"><ErrorBanner message={error} /></div>}
      {receipt ? (
        <>
          <p className="text-[14px] mt-2">
            Done. Removed <strong>{receipt.documents_removed}</strong> documents ({receipt.chunks_removed} chunks,{' '}
            {receipt.brief_filings_removed} brief filings) and <strong>{receipt.history_entries_removed}</strong> history
            entries. The account, feedback, Private Vault and payment data were not touched.
          </p>
          <button onClick={onClose} className="mt-4 rounded-lex border border-stone-300 bg-white px-3 py-1.5 text-[13px] font-medium">
            Close
          </button>
        </>
      ) : summary.isLoading ? (
        <Spinner label="Counting this user's data…" />
      ) : summary.data ? (
        <>
          <p className="text-[14px] mt-2 leading-relaxed">
            This permanently deletes <strong>{summary.data.documents} uploaded document(s)</strong> — including the ones
            filed in their Briefs, removed from the corpus, vectors, graph and brief filings — and{' '}
            <strong>{summary.data.history_entries} history entries</strong>. It cannot be undone.
          </p>
          <p className="text-[13px] text-stone-500 mt-1.5">
            Preserved: the account itself, their feedback, Private Vault, payment and usage records, and everyone else's data.
          </p>
          <label className="block mt-4">
            <span className="text-[12px] uppercase tracking-wider text-stone-500 font-medium">
              Type {user.email} to confirm
            </span>
            <input
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
              className="mt-1.5 w-full max-w-md rounded-lex border border-stone-300 bg-white px-3 py-2 text-[14px]"
              placeholder={user.email}
            />
          </label>
          <div className="flex gap-2 mt-4">
            <button
              onClick={() => purge.mutate()}
              disabled={typed.trim() !== user.email || purge.isPending}
              className="rounded-lex bg-[#a02c2c] text-white px-3.5 py-2 text-[13px] font-medium disabled:opacity-40"
            >
              {purge.isPending ? 'Deleting…' : 'Permanently delete this data'}
            </button>
            <button onClick={onClose} className="rounded-lex border border-stone-300 bg-white px-3 py-2 text-[13px] font-medium">
              Cancel
            </button>
          </div>
        </>
      ) : (
        <p className="text-[14px] mt-2">Could not load the counts for this user.</p>
      )}
    </div>
  )
}

export default function UsersPage({ me }: { me: Me }) {
  const qc = useQueryClient()
  const [error, setError] = useState<string | null>(null)
  const [purgeFor, setPurgeFor] = useState<AdminUser | null>(null)
  const { data: users, isLoading } = useQuery({ queryKey: ['users'], queryFn: adminApi.users })

  const role = useMutation({
    mutationFn: ({ id, role }: { id: string; role: string }) => adminApi.setRole(id, role),
    onSuccess: () => { qc.invalidateQueries({ queryKey: ['users'] }); setError(null) },
    onError: (e) => setError(errMsg(e)),
  })
  const del = useMutation({
    mutationFn: (id: string) => adminApi.deleteUser(id),
    onSuccess: () => { qc.invalidateQueries({ queryKey: ['users'] }); setError(null) },
    onError: (e) => setError(errMsg(e)),
  })

  return (
    <div className="max-w-5xl mx-auto px-6 py-10">
      <PageHead title="Users" sub="Manage workspace membership and roles. Admins can access the Command Center; grant the role sparingly." />
      {error && <div className="mt-5"><ErrorBanner message={error} /></div>}

      {isLoading ? (
        <Spinner label="Loading users…" />
      ) : (
        <div className="mt-6 rounded-lex-lg border border-stone-200 bg-white shadow-lex overflow-hidden">
          <table className="w-full text-left">
            <thead>
              <tr className="border-b border-stone-200 bg-stone-100/60 text-[12px] uppercase tracking-wider text-stone-500">
                <th className="px-5 py-3 font-medium">User</th>
                <th className="px-5 py-3 font-medium">Role</th>
                <th className="px-5 py-3 font-medium">Joined</th>
                <th className="px-5 py-3 font-medium text-right">Actions</th>
              </tr>
            </thead>
            <tbody>
              {(users ?? []).map((u) => (
                <tr key={u.id} className="border-b border-stone-200 last:border-0 hover:bg-stone-100/40">
                  <td className="px-5 py-3.5">
                    <div className="flex items-center gap-3">
                      <span className="w-8 h-8 rounded-full bg-stone-100 border border-stone-200 flex items-center justify-center text-stone-500">
                        {u.role === 'admin' ? <ShieldCheck size={15} /> : <UserIcon size={15} />}
                      </span>
                      <div>
                        <p className="text-[14px] font-medium">{u.email}</p>
                        {u.id === me.id && <p className="text-[11px] text-stone-400">This is you</p>}
                      </div>
                    </div>
                  </td>
                  <td className="px-5 py-3.5">
                    <span className={`inline-flex rounded-full border px-2 py-0.5 text-[11px] font-semibold uppercase tracking-wide ${
                      u.role === 'admin' ? 'bg-ink text-ivory border-ink' : 'bg-stone-100 text-stone-600 border-stone-200'
                    }`}>
                      {u.role}
                    </span>
                  </td>
                  <td className="px-5 py-3.5 text-[13px] text-stone-500">
                    {new Date(u.created_at).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' })}
                  </td>
                  <td className="px-5 py-3.5">
                    <div className="flex justify-end gap-2">
                      <button
                        onClick={() => role.mutate({ id: u.id, role: u.role === 'admin' ? 'user' : 'admin' })}
                        disabled={role.isPending}
                        className="rounded-lex border border-stone-300 px-2.5 py-1.5 text-[12px] font-medium hover:bg-stone-100 disabled:opacity-50"
                      >
                        {u.role === 'admin' ? 'Demote to user' : 'Make admin'}
                      </button>
                      <button
                        onClick={() => setPurgeFor(u)}
                        title="Permanently delete this user's uploaded documents and history (account stays)"
                        className="inline-flex items-center gap-1 rounded-lex border border-stone-300 px-2.5 py-1.5 text-[12px] font-medium text-[#a02c2c] hover:bg-[#feeaea]"
                      >
                        <Eraser size={13} /> Delete data
                      </button>
                      <button
                        onClick={() => { if (confirm(`Delete ${u.email}? This cannot be undone.`)) del.mutate(u.id) }}
                        disabled={u.id === me.id || del.isPending}
                        title={u.id === me.id ? 'You cannot delete yourself' : 'Delete user'}
                        className="p-1.5 rounded-lex text-stone-400 hover:text-[#a02c2c] hover:bg-[#feeaea] disabled:opacity-30 transition-colors"
                      >
                        <Trash2 size={15} />
                      </button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {purgeFor && <PurgePanel user={purgeFor} onClose={() => setPurgeFor(null)} />}
      <p className="text-[12px] text-stone-400 mt-4">
        Users self-register in the app. Emails listed in the server's <code className="font-mono">LEGAL_RAG_ADMIN_EMAILS</code> are promoted to admin automatically at sign-in.
      </p>
    </div>
  )
}
