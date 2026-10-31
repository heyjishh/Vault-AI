import { useState } from 'react'
import { adminApi, errMsg, type Me } from './api'
import { AuthScene, BrandMark, ThemeToggle } from './ui'

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
    <div className="min-h-screen dark auth-scene bg-[#04100b] relative overflow-hidden flex items-center justify-center p-6">
      {/* Piece 42: the jurisvault.ai auth scene runs behind the
          admin sign-in — always dark (local `dark` class), whatever
          the app theme — plus the theme toggle */}
      <AuthScene />
      <div className="absolute top-4 right-4 z-20">
        <ThemeToggle />
      </div>
      <div className="relative z-10 w-full max-w-sm fade-up">
        <div>
          <div className="flex items-center gap-3">
            <BrandMark size={40} />
            <span className="font-display text-2xl tracking-tight">Juris Vault AI</span>
          </div>
          <div className="mt-3 h-[2px] w-[44px] bg-bronze" />
        </div>
        <p className="text-stone-500 dark:text-stone-400 text-meta mt-4 mb-8">Command Center · Admin sign-in</p>
        <form onSubmit={submit} className="space-y-4">
          <div>
            <label className="block text-meta font-medium text-stone-600 dark:text-stone-300 mb-1.5">Admin email</label>
            <input
              type="email" required value={email} onChange={(e) => setEmail(e.target.value)}
              autoComplete="email" placeholder="admin@firm.com"
              className="w-full rounded-lex border border-stone-300 dark:border-white/10 bg-white dark:bg-[#0a1a12] px-3.5 py-2.5 text-[15px] outline-none focus:border-ink focus:ring-1 focus:ring-ink placeholder:text-stone-400"
            />
          </div>
          <div>
            <label className="block text-meta font-medium text-stone-600 dark:text-stone-300 mb-1.5">Password</label>
            <input
              type="password" required value={password} onChange={(e) => setPassword(e.target.value)}
              autoComplete="current-password" placeholder="••••••••"
              className="w-full rounded-lex border border-stone-300 dark:border-white/10 bg-white dark:bg-[#0a1a12] px-3.5 py-2.5 text-[15px] outline-none focus:border-ink focus:ring-1 focus:ring-ink placeholder:text-stone-400"
            />
          </div>
          {error && (
            <div className="rounded-lex border border-brand-red/30 bg-[#feeaea] px-3.5 py-2.5 text-meta2 text-[#a02c2c]">{error}</div>
          )}
          <button
            type="submit" disabled={loading}
            className="btn-brand w-full rounded-lex text-ivory py-2.5 text-[15px] font-medium transition-colors disabled:opacity-50"
          >
            {loading ? 'Signing in…' : 'Sign in to Command Center'}
          </button>
        </form>
        <p className="mt-6 text-center text-meta text-stone-500 dark:text-stone-400">
          Not an admin? <a href="/" className="text-ink dark:text-[#f4f2ee] font-medium underline underline-offset-2">Open the Juris Vault AI app</a>
        </p>
      </div>
    </div>
  )
}
