import { useState } from 'react'
import {
  LayoutDashboard, Users, FolderOpen, Settings2, LogOut, ArrowLeft, IndianRupee, Activity, Target, Palette, BadgeCheck,
} from 'lucide-react'
import type { Me } from './api'
import { BrandMark, DualToneBackdrop, ThemeToggle } from './ui'
import OverviewPage from './pages/OverviewPage'
import UsersPage from './pages/UsersPage'
import CorpusPage from './pages/CorpusPage'
import SystemSettingsPage from './pages/SystemSettingsPage'
import CustomizePage from './pages/CustomizePage'
import BillingPage from './pages/BillingPage'
import InsightsPage from './pages/InsightsPage'
import GoldenPage from './pages/GoldenPage'
import AnnotationsPage from './pages/AnnotationsPage'

type View = 'overview' | 'users' | 'corpus' | 'system' | 'billing' | 'insights' | 'golden' | 'annotations' | 'customize'

const NAV: { id: View; label: string; icon: React.ElementType }[] = [
  { id: 'overview', label: 'Overview', icon: LayoutDashboard },
  { id: 'users', label: 'Users', icon: Users },
  { id: 'corpus', label: 'Corpus & Vault', icon: FolderOpen },
  { id: 'system', label: 'Retrieval & Models', icon: Settings2 },
  { id: 'billing', label: 'Billing', icon: IndianRupee },
  { id: 'insights', label: 'Insights', icon: Activity },
  { id: 'golden', label: 'Golden Ruleset', icon: Target },
  { id: 'annotations', label: 'Verified Answers', icon: BadgeCheck },
  { id: 'customize', label: 'Customize', icon: Palette },
]

export default function Shell({ user, onLogout }: { user: Me; onLogout: () => void }) {
  const [view, setView] = useState<View>('overview')

  return (
    <div className="h-screen flex relative app-wash text-ink dark:text-[#f4f2ee] overflow-hidden">
      {/* Piece 39: one glow layer fixed behind the whole app, over the
          page wash — the same background system as the user app. */}
      <DualToneBackdrop fixed />
      {/* Sidebar — transparent over the wash, hairline right border */}
      <aside className="sidebar-raise relative z-10 w-60 flex-shrink-0 border-r border-stone-200 dark:border-white/10 flex flex-col">
        <div className="px-5 py-4 border-b border-stone-200 dark:border-white/10">
          <div className="flex items-center gap-3">
            <BrandMark size={36} />
            <div className="min-w-0">
              <p className="font-display text-[21px] leading-tight tracking-tight truncate">Juris Vault AI</p>
              <p className="text-[11.5px] text-stone-500 dark:text-stone-400 leading-tight mt-0.5 truncate">Command Center</p>
            </div>
          </div>
          <div className="mt-3 h-[2px] w-[44px] bg-bronze" />
        </div>

        <nav className="flex-1 px-3 py-4 space-y-1 overflow-y-auto">
          {NAV.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              onClick={() => setView(id)}
              className={`w-full flex items-center gap-3 rounded-lex px-3 py-2 text-[14px] transition-colors active:scale-[.98] ${
                view === id
                  ? 'bg-stone-100 dark:bg-[#122419] text-ink dark:text-[#f4f2ee] font-medium shadow-[inset_2px_0_0_var(--bronze)]'
                  : 'text-stone-600 dark:text-stone-300 hover:bg-stone-100/70 dark:hover:bg-white/5 hover:text-ink'
              }`}
            >
              <Icon size={16} strokeWidth={1.8} />
              {label}
            </button>
          ))}
          <div className="pt-4 mt-2 border-t border-stone-200 dark:border-white/10">
            <a
              href="/"
              className="w-full flex items-center gap-3 rounded-lex px-3 py-2 text-[14px] text-stone-600 dark:text-stone-300 hover:bg-stone-100/70 dark:hover:bg-white/5 hover:text-ink transition-colors active:scale-[.98]"
            >
              <ArrowLeft size={16} strokeWidth={1.8} />
              Back to app
            </a>
          </div>
        </nav>

        <div className="border-t border-stone-200 dark:border-white/10 px-4 py-3.5 flex items-center justify-between">
          <div className="min-w-0">
            <p className="text-[13px] font-medium truncate">{user.email}</p>
            <p className="text-[11px] text-stone-400 dark:text-stone-500">Administrator</p>
          </div>
          <span className="flex items-center gap-0.5 shrink-0">
            <ThemeToggle />
            <button onClick={onLogout} title="Sign out" className="p-2 rounded-lex text-stone-500 dark:text-stone-400 hover:bg-stone-100 dark:hover:bg-white/10 hover:text-ink transition-colors">
              <LogOut size={16} />
            </button>
          </span>
        </div>
      </aside>

      <main className="relative z-10 flex-1 min-w-0 overflow-y-auto">
        {/* Piece 45: CSS view transition — the keyed wrapper remounts
            on every view switch, replaying the view-enter rise,
            mirroring the user app's framer view transitions. */}
        <div key={view} className="view-enter">
          {view === 'overview' && <OverviewPage />}
          {view === 'users' && <UsersPage me={user} />}
          {view === 'corpus' && <CorpusPage />}
          {view === 'system' && <SystemSettingsPage />}
          {view === 'customize' && <CustomizePage />}
          {view === 'billing' && <BillingPage />}
          {view === 'insights' && <InsightsPage />}
          {view === 'golden' && <GoldenPage />}
          {view === 'annotations' && <AnnotationsPage />}
        </div>
      </main>
    </div>
  )
}
