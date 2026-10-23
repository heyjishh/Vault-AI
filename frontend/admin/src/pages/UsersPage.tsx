import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ShieldCheck, Trash2, User as UserIcon } from 'lucide-react'
import { adminApi, errMsg, type Me } from '../api'
import { ErrorBanner, PageHead, Spinner } from '../ui'

export default function UsersPage({ me }: { me: Me }) {
  const qc = useQueryClient()
  const [error, setError] = useState<string | null>(null)
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
      <p className="text-[12px] text-stone-400 mt-4">
        Users self-register in the app. Emails listed in the server's <code className="font-mono">LEGAL_RAG_ADMIN_EMAILS</code> are promoted to admin automatically at sign-in.
      </p>
    </div>
  )
}
