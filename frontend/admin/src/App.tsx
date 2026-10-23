import { useEffect, useState } from 'react'
import { adminApi, tokens, type Me } from './api'
import Shell from './Shell'
import AuthGate from './AuthGate'

export default function App() {
  const [user, setUser] = useState<Me | null>(null)
  const [booting, setBooting] = useState(true)

  useEffect(() => {
    if (!tokens.access) { setBooting(false); return }
    adminApi.me()
      .then(setUser)
      .catch(() => tokens.clear())
      .finally(() => setBooting(false))
  }, [])

  if (booting) {
    return <div className="min-h-screen bg-ink flex items-center justify-center text-ivory/60">Loading Command Center…</div>
  }
  if (!user) return <AuthGate onAuthed={setUser} />
  if (user.role !== 'admin') return <NonAdmin user={user} />
  return <Shell user={user} onLogout={() => { tokens.clear(); setUser(null) }} />
}

function NonAdmin({ user }: { user: Me }) {
  return (
    <div className="min-h-screen bg-ivory flex items-center justify-center p-6">
      <div className="max-w-sm text-center">
        <h1 className="font-display text-3xl tracking-tight">Command Center is admin-only</h1>
        <p className="text-stone-500 mt-3 text-[15px]">
          Signed in as <span className="font-medium text-ink">{user.email}</span> (role: {user.role}).
          Ask an administrator to grant you access, or return to the app.
        </p>
        <div className="mt-6 flex gap-3 justify-center">
          <a href="/" className="rounded-lex bg-ink text-ivory px-4 py-2 text-[15px] font-medium">Open LexAgent</a>
          <button
            onClick={() => { tokens.clear(); window.location.reload() }}
            className="rounded-lex border border-stone-300 px-4 py-2 text-[15px] font-medium"
          >
            Sign out
          </button>
        </div>
      </div>
    </div>
  )
}
