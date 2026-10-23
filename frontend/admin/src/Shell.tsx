import { useState } from 'react'
import {
  LayoutDashboard, Users, FolderOpen, Settings2, LogOut, ArrowLeft, IndianRupee, Activity, Target, Palette, BadgeCheck,
} from 'lucide-react'
import type { Me } from './api'
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
    <div className="h-screen flex bg-ivory text-ink overflow-hidden">
      {/* Dark ink sidebar — visually distinct from the user app */}
      <aside className="w-60 flex-shrink-0 bg-ink text-ivory flex flex-col">
        <div className="h-[4.5rem] flex items-center gap-2.5 px-5 border-b border-ivory/10">
          <div className="w-8 h-8 rounded-lg bg-ivory flex items-center justify-center">
            <span className="font-display text-ink text-lg leading-none pt-0.5">L</span>
          </div>
          <div>
            <p className="font-display text-lg leading-none tracking-tight">Juris Vault AI</p>
            <p className="text-[10px] uppercase tracking-[0.18em] text-ivory/50 mt-1">Command Center</p>
          </div>
        </div>

        <nav className="flex-1 px-3 py-4 space-y-1 overflow-y-auto">
          {NAV.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              onClick={() => setView(id)}
              className={`w-full flex items-center gap-3 rounded-lex px-3 py-2 text-[14px] transition-colors ${
                view === id
                  ? 'bg-ivory/15 text-ivory font-medium'
                  : 'text-ivory/60 hover:bg-ivory/10 hover:text-ivory'
              }`}
            >
              <Icon size={16} strokeWidth={1.8} />
              {label}
            </button>
          ))}
          <div className="pt-4 mt-2 border-t border-ivory/10">
            <a
              href="/"
              className="w-full flex items-center gap-3 rounded-lex px-3 py-2 text-[14px] text-ivory/60 hover:bg-ivory/10 hover:text-ivory transition-colors"
            >
              <ArrowLeft size={16} strokeWidth={1.8} />
              Back to app
            </a>
          </div>
        </nav>

        <div className="border-t border-ivory/10 px-4 py-3.5 flex items-center justify-between">
          <div className="min-w-0">
            <p className="text-meta2 font-medium truncate text-ivory">{user.email}</p>
            <p className="text-[11px] text-ivory/40">Administrator</p>
          </div>
          <button onClick={onLogout} title="Sign out" className="p-2 rounded-lex text-ivory/60 hover:bg-ivory/10 hover:text-ivory transition-colors">
            <LogOut size={16} />
          </button>
        </div>
      </aside>

      <main className="flex-1 min-w-0 overflow-y-auto">
        {view === 'overview' && <OverviewPage />}
        {view === 'users' && <UsersPage me={user} />}
        {view === 'corpus' && <CorpusPage />}
        {view === 'system' && <SystemSettingsPage />}
        {view === 'customize' && <CustomizePage />}
        {view === 'billing' && <BillingPage />}
        {view === 'insights' && <InsightsPage />}
        {view === 'golden' && <GoldenPage />}
        {view === 'annotations' && <AnnotationsPage />}
      </main>
    </div>
  )
}
