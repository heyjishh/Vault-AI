import { useState } from 'react'
import { Scale, ShieldCheck, BookOpen, Zap } from 'lucide-react'
import { useAuth } from '../stores/auth'
import { AuthScene, BrandMark, ThemeToggle } from './ui'

export default function AuthPage() {
  const login = useAuth((s) => s.login)
  const register = useAuth((s) => s.register)
  const loading = useAuth((s) => s.loading)
  const error = useAuth((s) => s.error)
  const [mode, setMode] = useState<'login' | 'register'>('login')
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')

  async function submit(e: React.FormEvent) {
    e.preventDefault()
    try {
      if (mode === 'login') await login(email, password)
      else await register(email, password)
    } catch { /* error surfaced via store */ }
  }

  return (
    <div className="min-h-screen dark auth-scene bg-[#04100b] flex relative overflow-hidden">
      {/* Piece 42: the jurisvault.ai auth scene runs behind the
          sign-in — always dark (local `dark` class), whatever the
          app theme — plus the theme toggle for the app beyond it */}
      <AuthScene />
      <div className="absolute top-4 right-4 z-20">
        <ThemeToggle />
      </div>
      {/* Left: product panel (Harvey-style serif statement) */}
      <div className="relative hidden lg:flex w-1/2 flex-col justify-between p-14 border-r border-stone-200 dark:border-white/10">
        <div>
          <div className="flex items-center gap-3">
            <BrandMark size={40} />
            <span className="font-display text-2xl tracking-tight">Juris Vault AI</span>
          </div>
          <div className="mt-3 h-[2px] w-[44px] bg-bronze" />
        </div>
        <div>
          <h1 className="font-display text-5xl leading-[1.15] tracking-tight max-w-lg">
            Legal research, <em className="italic">grounded</em> in your documents.
          </h1>
          <p className="mt-6 text-stone-600 dark:text-stone-300 text-lg leading-relaxed max-w-md">
            Ask questions across your corpus and get answers with verifiable citations — every claim traceable to its source.
          </p>
          <div className="mt-10 space-y-4">
            {[
              { icon: BookOpen, text: 'Multimodal RAG over statutes, judgments and contracts' },
              { icon: ShieldCheck, text: 'Grounded answers — no uncited claims' },
              { icon: Zap, text: 'Agentic workflows for reviews, diligence and memos' },
            ].map(({ icon: Icon, text }) => (
              <div key={text} className="flex items-center gap-3 text-stone-700 dark:text-stone-300">
                <Icon className="w-4.5 h-4.5 text-bronze" size={18} />
                <span className="text-[15px]">{text}</span>
              </div>
            ))}
          </div>
        </div>
        <p className="text-meta text-stone-400 dark:text-stone-500 flex items-center gap-1.5">
          <Scale size={12} className="text-bronze" /> Juris Vault AI — agentic legal research &amp; RAG
        </p>
      </div>

      {/* Right: auth card */}
      <div className="relative flex-1 flex items-center justify-center p-6">
        <div className="w-full max-w-sm view-enter">
          <div className="lg:hidden flex flex-col items-center mb-8">
            <div className="flex items-center gap-3">
              <BrandMark size={40} />
              <span className="font-display text-2xl tracking-tight">Juris Vault AI</span>
            </div>
            <div className="mt-3 h-[2px] w-[44px] bg-bronze" />
          </div>
          <h2 className="font-display text-3xl tracking-tight">
            {mode === 'login' ? 'Welcome back' : 'Create your workspace'}
          </h2>
          <p className="text-stone-500 dark:text-stone-400 mt-2 text-[15px]">
            {mode === 'login' ? 'Sign in to continue to your briefs.' : 'Set up your account to start researching.'}
          </p>

          <form onSubmit={submit} className="mt-8 space-y-4">
            <div>
              <label className="block text-meta font-medium text-stone-600 dark:text-stone-300 mb-1.5">Email</label>
              <input
                type="email" required value={email} onChange={(e) => setEmail(e.target.value)}
                placeholder="you@firm.com" autoComplete="email"
                className="w-full rounded-lex border border-stone-300 dark:border-white/10 bg-white dark:bg-[#0a1a12] px-3.5 py-2.5 text-[15px] outline-none focus:border-ink focus:ring-1 focus:ring-ink placeholder:text-stone-400"
              />
            </div>
            <div>
              <label className="block text-meta font-medium text-stone-600 dark:text-stone-300 mb-1.5">Password</label>
              <input
                type="password" required value={password} onChange={(e) => setPassword(e.target.value)}
                placeholder="••••••••" autoComplete={mode === 'login' ? 'current-password' : 'new-password'}
                className="w-full rounded-lex border border-stone-300 dark:border-white/10 bg-white dark:bg-[#0a1a12] px-3.5 py-2.5 text-[15px] outline-none focus:border-ink focus:ring-1 focus:ring-ink placeholder:text-stone-400"
              />
            </div>
            {error && (
              <div className="rounded-lex border border-brand-red/30 bg-[#feeaea] px-3.5 py-2.5 text-meta2 text-[#a02c2c]">
                {error}
              </div>
            )}
            <button
              type="submit" disabled={loading}
              className="btn-brand w-full rounded-lex text-ivory py-2.5 text-[15px] font-medium transition-colors disabled:opacity-50"
            >
              {loading ? 'Please wait…' : mode === 'login' ? 'Sign in' : 'Create account'}
            </button>
          </form>

          <div className="mt-5 text-center">
            <a
              href="/api/v1/auth/oidc/login"
              className="inline-block w-full rounded-lex border border-stone-300 dark:border-white/10 bg-white dark:bg-[#0a1a12] py-2.5 text-[15px] font-medium text-ink dark:text-[#f4f2ee] hover:bg-stone-100 dark:hover:bg-white/10 transition-colors"
            >
              Continue with SSO
            </a>
          </div>

          <p className="mt-6 text-center text-meta2 text-stone-500 dark:text-stone-400">
            {mode === 'login' ? (
              <>New to Juris Vault AI?{' '}<button onClick={() => setMode('register')} className="text-ink dark:text-[#f4f2ee] font-medium underline underline-offset-2">Create an account</button></>
            ) : (
              <>Already have an account?{' '}<button onClick={() => setMode('login')} className="text-ink dark:text-[#f4f2ee] font-medium underline underline-offset-2">Sign in</button></>
            )}
          </p>
        </div>
      </div>
    </div>
  )
}
