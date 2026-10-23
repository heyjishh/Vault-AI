import { useState } from 'react'
import { adminApi, errMsg, type Me } from './api'

export default function AuthGate({ onAuthed }: { onAuthed: (u: Me) => void }) {
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)

  async function submit(e: React.FormEvent) {
    e.preventDefault()
    setLoading(true)
    setError(null)
    try {
      const user = await adminApi.login(email, password)
      onAuthed(user)
    } catch (err) {
      setError(errMsg(err))
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="min-h-screen bg-ink flex items-center justify-center p-6">
      <div className="w-full max-w-sm fade-up">
        <div className="flex items-center gap-2.5 mb-2">
          <div className="w-9 h-9 rounded-lg bg-ivory flex items-center justify-center">
            <span className="font-display text-ink text-xl leading-none pt-0.5">L</span>
          </div>
          <span className="font-display text-2xl tracking-tight text-ivory">Juris Vault AI</span>
        </div>
        <p className="text-ivory/50 text-meta mb-8 uppercase tracking-widest">Command Center · Admin sign-in</p>
        <form onSubmit={submit} className="space-y-4">
          <div>
            <label className="block text-meta font-medium text-ivory/60 mb-1.5">Admin email</label>
            <input
              type="email" required value={email} onChange={(e) => setEmail(e.target.value)}
              autoComplete="email" placeholder="admin@firm.com"
              className="w-full rounded-lex border border-ivory/25 bg-ink-900 text-ivory px-3.5 py-2.5 text-[15px] outline-none focus:border-ivory placeholder:text-ivory/30"
            />
          </div>
          <div>
            <label className="block text-meta font-medium text-ivory/60 mb-1.5">Password</label>
            <input
              type="password" required value={password} onChange={(e) => setPassword(e.target.value)}
              autoComplete="current-password" placeholder="••••••••"
              className="w-full rounded-lex border border-ivory/25 bg-ink-900 text-ivory px-3.5 py-2.5 text-[15px] outline-none focus:border-ivory placeholder:text-ivory/30"
            />
          </div>
          {error && (
            <div className="rounded-lex border border-brand-red/40 bg-[#3d1518] px-3.5 py-2.5 text-meta2 text-red-200">{error}</div>
          )}
          <button
            type="submit" disabled={loading}
            className="w-full rounded-lex bg-ivory text-ink py-2.5 text-[15px] font-medium hover:bg-white transition-colors disabled:opacity-50"
          >
            {loading ? 'Signing in…' : 'Sign in to Command Center'}
          </button>
        </form>
        <p className="mt-6 text-center text-meta text-ivory/40">
          Not an admin? <a href="/" className="text-ivory/70 underline underline-offset-2">Open the Juris Vault AI app</a>
        </p>
      </div>
    </div>
  )
}
