import { useEffect, useRef, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { AnimatePresence, motion } from 'framer-motion'
import {
  MessageSquare, FlaskConical, Bot, FolderOpen, Network, LogOut, LayoutDashboard,
  PenSquare, Trash2, CreditCard, FileSearch, PenLine, Briefcase, Scale, Lock, ExternalLink, PanelTop, Search,
} from 'lucide-react'
import { useAuth } from '../stores/auth'
import { useSession } from '../stores/session'
import { useMatter } from '../stores/matter'
import { dashboardApi, mattersApi, publicSettingsApi } from '../api/client'
import AssistantPage from './AssistantPage'
import ResearchPage from './ResearchPage'
import AgentsPage from './AgentsPage'
import VaultPage from './VaultPage'
import GraphPage from './GraphPage'
import PlansPage from './PlansPage'
import DashboardPage from './DashboardPage'
import InteractPage from './InteractPage'
import DraftPage from './DraftPage'
import MattersPage from './MattersPage'
import PrivateVaultPage from './PrivateVaultPage'
import CustomTabPage from './CustomTabPage'
import SearchPage from './SearchPage'
import MatterPage from './MatterPage'
import ErrorBoundary from './ErrorBoundary'
import Toaster from './Toaster'
import ClausePet, { ClausePetToggle } from './ClausePet'

export type View = 'dashboard' | 'matters' | 'matter' | 'assistant' | 'interact' | 'draft' | 'research' | 'agents' | 'vault' | 'search' | 'private' | 'graph' | 'plans' | 'tab'

const NAV: { id: View; label: string; icon: React.ElementType }[] = [
  { id: 'dashboard', label: 'Dashboard', icon: LayoutDashboard },
  { id: 'matters', label: 'Briefs', icon: Briefcase },
  { id: 'assistant', label: 'Ask', icon: MessageSquare },
  { id: 'interact', label: 'Interact', icon: FileSearch },
  { id: 'draft', label: 'Draft', icon: PenLine },
  { id: 'research', label: 'Research', icon: FlaskConical },
  { id: 'agents', label: 'Agents', icon: Bot },
  { id: 'vault', label: 'Vault', icon: FolderOpen },
  { id: 'search', label: 'Search documents', icon: Search },
  { id: 'private', label: 'Private Vault', icon: Lock },
  { id: 'graph', label: 'Knowledge', icon: Network },
  { id: 'plans', label: 'Plans & Billing', icon: CreditCard },
]

export default function AppShell() {
  const [view, setView] = useState<View>('dashboard')
  const [chatNonce, setChatNonce] = useState(0)
  const [openMatterId, setOpenMatterId] = useState<string | null>(null)
  const currentMatterId = useMatter((s) => s.currentMatterId)
  const setCurrentMatter = useMatter((s) => s.setCurrentMatter)
  const { data: mattersList } = useQuery({ queryKey: ['matters'], queryFn: mattersApi.list })
  // Brief-only vault: an active brief always exists. When nothing
  // is chosen (or the stored pick no longer exists), the active
  // brief is the default brief — the same fallback uploads use
  // server-side. The resolution is written back to the store once
  // the list first loads; surfaces that must never be unscoped
  // (Vault, Interact) resolve the same fallback locally, so no
  // surface ever renders a "full vault".
  const defaultMatterId = mattersList?.find((m) => m.is_default)?.id ?? mattersList?.[0]?.id
  const currentIsValid = !!currentMatterId && !!mattersList?.some((m) => m.id === currentMatterId)
  const effectiveMatterId = currentIsValid
    ? currentMatterId
    : mattersList
      ? (defaultMatterId ?? null)
      : currentMatterId
  const resolvedOnce = useRef(false)
  useEffect(() => {
    if (!mattersList || resolvedOnce.current) return
    resolvedOnce.current = true
    if (effectiveMatterId && effectiveMatterId !== currentMatterId) {
      setCurrentMatter(effectiveMatterId)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mattersList])
  // A stored pick pointing at a brief that no longer exists (e.g.
  // deleted elsewhere) falls back to the default brief. A null pick
  // after load is left alone — that is Ask's in-session widening,
  // and Ask's behaviour is unchanged by the brief-only model.
  useEffect(() => {
    if (mattersList && currentMatterId && !currentIsValid && defaultMatterId) {
      setCurrentMatter(defaultMatterId)
    }
  }, [mattersList, currentMatterId, currentIsValid, defaultMatterId, setCurrentMatter])
  const { data: pubSettings } = useQuery({ queryKey: ['public-settings'], queryFn: publicSettingsApi.get })
  const [activeTabId, setActiveTabId] = useState<string | null>(null)
  const brand = pubSettings?.brand ?? { product_name: 'Juris Vault AI', tagline: 'Legal Research, With Memory' }
  const customTabs = pubSettings?.tabs ?? []
  const activeTab = customTabs.find((t) => t.id === activeTabId && t.kind === 'page')

  useEffect(() => {
    document.title = `${brand.product_name} — ${brand.tagline}`
  }, [brand.product_name, brand.tagline])

  function openMatter(id: string) {
    setOpenMatterId(id)
    setCurrentMatter(id)
    setView('matter')
  }
  const user = useAuth((s) => s.user)
  const logout = useAuth((s) => s.logout)
  const history = useSession((s) => s.history)
  const ask = useSession((s) => s.ask)
  const clearHistory = useSession((s) => s.clearHistory)
  const { data: dash } = useQuery({ queryKey: ['dashboard'], queryFn: dashboardApi.me, refetchInterval: 60_000 })

  function newChat() {
    setView('assistant')
    setChatNonce((n) => n + 1)
  }

  function reAsk(q: string) {
    setView('assistant')
    setChatNonce((n) => n + 1)
    // Let the fresh Assistant mount, then hand it the question.
    setTimeout(() => ask(q), 60)
  }

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'n' && e.shiftKey) {
        e.preventDefault()
        newChat()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  return (
    <div className="h-screen flex bg-ivory text-ink overflow-hidden">
      {/* Sidebar — ivory, hairline right border (Harvey) */}
      <aside className="w-64 flex-shrink-0 border-r border-stone-200 flex flex-col">
        <div className="h-[4.5rem] flex items-center gap-2.5 px-5 border-b border-stone-200">
          <div className="w-8 h-8 rounded-lg bg-ink flex items-center justify-center">
            <Scale size={16} className="text-ivory" />
          </div>
          <span className="font-display text-xl tracking-tight">{brand.product_name}</span>
        </div>

        <div className="px-3 pt-4">
          <button
            onClick={newChat}
            className="w-full flex items-center gap-2.5 rounded-lex bg-ink text-ivory px-3 py-2.5 text-[14px] font-medium hover:bg-ink-900 transition-colors shadow-lex"
          >
            <PenSquare size={15} />
            New research
            <span className="ml-auto text-[10px] text-ivory/50 font-normal">⌘⇧N</span>
          </button>

          {/* Matter switcher: scopes Ask, Draft, Interact, Vault and
              uploads to one brief. Brief-only vault — the switcher
              only chooses WHICH brief; there is no full-vault view. */}
          <div className="mt-3">
            <p className="text-[11px] uppercase tracking-wider text-stone-400 font-medium px-1">Working in</p>
            <select
              value={effectiveMatterId ?? ''}
              onChange={(e) => setCurrentMatter(e.target.value)}
              className="mt-1.5 w-full rounded-lex border border-stone-200 bg-white px-2.5 py-2 text-[13px] outline-none focus:border-stone-400"
            >
              {(mattersList ?? []).map((m) => (
                <option key={m.id} value={m.id}>{m.title}</option>
              ))}
            </select>
            {effectiveMatterId && (
              <button
                onClick={() => openMatter(effectiveMatterId)}
                className="mt-1.5 px-1 text-[12px] text-stone-500 hover:text-ink font-medium"
              >
                Open this brief →
              </button>
            )}
          </div>
        </div>

        <nav className="flex-1 px-3 py-4 space-y-1 overflow-y-auto">
          {NAV.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              onClick={() => { setActiveTabId(null); setView(id) }}
              className={`w-full flex items-center gap-3 rounded-lex px-3 py-2 text-[14px] transition-colors ${
                view === id || (id === 'matters' && view === 'matter')
                  ? 'bg-stone-100 text-ink font-medium shadow-[inset_0_0_0_1px_#e5e5e3]'
                  : 'text-stone-600 hover:bg-stone-100/70 hover:text-ink'
              }`}
            >
              <Icon size={16} strokeWidth={1.8} />
              {label}
            </button>
          ))}

          {user?.role === 'admin' && (
            <a
              href="/admin/"
              className="w-full flex items-center gap-3 rounded-lex px-3 py-2 text-[14px] text-stone-600 hover:bg-stone-100/70 hover:text-ink transition-colors"
            >
              <LayoutDashboard size={16} strokeWidth={1.8} />
              Command Center
              <span className="ml-auto text-[10px] font-semibold uppercase tracking-wide bg-ink text-ivory rounded px-1.5 py-0.5">Admin</span>
            </a>
          )}

          {customTabs.map((t) =>
            t.kind === 'link' ? (
              <a
                key={t.id}
                href={t.url}
                target="_blank"
                rel="noreferrer"
                className="w-full flex items-center gap-3 rounded-lex px-3 py-2 text-[14px] text-stone-600 hover:bg-stone-100/70 hover:text-ink transition-colors"
              >
                <ExternalLink size={16} strokeWidth={1.8} />
                {t.label}
              </a>
            ) : (
              <button
                key={t.id}
                onClick={() => { setActiveTabId(t.id); setView('tab') }}
                className={`w-full flex items-center gap-3 rounded-lex px-3 py-2 text-[14px] transition-colors ${
                  view === 'tab' && activeTabId === t.id
                    ? 'bg-stone-100 text-ink font-medium shadow-[inset_0_0_0_1px_#e5e5e3]'
                    : 'text-stone-600 hover:bg-stone-100/70 hover:text-ink'
                }`}
              >
                <PanelTop size={16} strokeWidth={1.8} />
                {t.label}
              </button>
            ),
          )}

          {history.length > 0 && (
            <div className="mt-6">
              <div className="flex items-center justify-between px-3">
                <p className="text-[11px] uppercase tracking-wider text-stone-400 font-medium">Recent research</p>
                <button onClick={clearHistory} title="Clear history" className="text-stone-400 hover:text-ink p-0.5">
                  <Trash2 size={12} />
                </button>
              </div>
              <div className="mt-1.5 space-y-0.5">
                {history.map((h) => (
                  <button
                    key={h}
                    onClick={() => reAsk(h)}
                    title={h}
                    className="w-full text-left rounded-lex px-3 py-1.5 text-[13px] text-stone-600 hover:bg-stone-100/70 hover:text-ink truncate transition-colors"
                  >
                    {h}
                  </button>
                ))}
              </div>
            </div>
          )}

          {dash && (
            <button
              onClick={() => { setActiveTabId(null); setView('plans') }}
              className="mt-6 mx-3 block text-left rounded-lex-lg border border-stone-200 bg-white shadow-lex-sm px-3.5 py-3 hover:border-stone-300 transition-colors"
            >
              <span className="flex items-center justify-between">
                <span className="text-[11px] uppercase tracking-wider text-stone-400 font-medium">Your plan</span>
                <span className="text-[10px] font-semibold uppercase tracking-wide bg-ink text-ivory rounded px-1.5 py-0.5">{dash.plan_name}</span>
              </span>
              <span className="block font-display text-[19px] leading-tight mt-1.5">
                {Math.max(0, dash.token_quota - dash.tokens_used).toLocaleString('en-IN')}
                <span className="text-[12.5px] text-stone-500 font-sans"> tokens left</span>
              </span>
              <span className="block h-1.5 rounded-full bg-stone-100 mt-2 overflow-hidden">
                <span
                  className={`block h-full rounded-full ${Math.round((dash.tokens_used / Math.max(1, dash.token_quota)) * 100) >= 90 ? 'bg-[#a02c2c]' : Math.round((dash.tokens_used / Math.max(1, dash.token_quota)) * 100) >= 70 ? 'bg-[#b7791f]' : 'bg-ink'}`}
                  style={{ width: `${Math.min(100, Math.round((dash.tokens_used / Math.max(1, dash.token_quota)) * 100))}%` }}
                />
              </span>
              <span className="block text-[11.5px] text-stone-400 mt-1.5">
                {dash.tokens_used.toLocaleString('en-IN')} of {dash.token_quota.toLocaleString('en-IN')} used this period
              </span>
            </button>
          )}
        </nav>

        <div className="border-t border-stone-200 px-4 py-2">
          <ClausePetToggle />
        </div>

        <div className="border-t border-stone-200 px-4 py-3.5 flex items-center justify-between">
          <div className="min-w-0">
            <p className="text-[13px] font-medium truncate">{user?.email ?? 'Signed in'}</p>
            <p className="text-[11px] text-stone-400">{user?.role === 'admin' ? 'Administrator' : `${brand.product_name} workspace`}</p>
          </div>
          <button onClick={logout} title="Sign out" className="p-2 rounded-lex text-stone-500 hover:bg-stone-100 hover:text-ink transition-colors">
            <LogOut size={16} />
          </button>
        </div>
      </aside>

      {/* Main */}
      <main className="flex-1 min-w-0 overflow-y-auto relative">
        <ErrorBoundary>
          <AnimatePresence mode="wait">
            <motion.div
              key={view + (view === 'assistant' ? `:${chatNonce}` : '')}
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -6 }}
              transition={{ duration: 0.22, ease: 'easeOut' }}
            >
              {view === 'dashboard' && <DashboardPage go={setView} openMatter={openMatter} />}
              {view === 'matters' && <MattersPage open={openMatter} />}
              {view === 'matter' && openMatterId && (
                <MatterPage matterId={openMatterId} back={() => setView('matters')} go={(v) => setView(v as View)} />
              )}
              {view === 'assistant' && (
                <AssistantPage
                  key={chatNonce}
                  go={(v) => { setActiveTabId(null); setView(v as View) }}
                />
              )}
              {view === 'interact' && (
                <InteractPage matterId={effectiveMatterId ?? undefined} go={(v) => { setActiveTabId(null); setView(v as View) }} />
              )}
              {view === 'draft' && <DraftPage go={(v) => { setActiveTabId(null); setView(v as View) }} />}
              {view === 'research' && <ResearchPage go={(v) => { setActiveTabId(null); setView(v as View) }} />}
              {view === 'agents' && <AgentsPage go={(v) => { setActiveTabId(null); setView(v as View) }} />}
              {view === 'vault' && <VaultPage />}
              {view === 'search' && <SearchPage go={(v) => { setActiveTabId(null); setView(v) }} />}
              {view === 'private' && <PrivateVaultPage />}
              {view === 'tab' && activeTab && <CustomTabPage tab={activeTab} />}
              {view === 'graph' && <GraphPage />}
              {view === 'plans' && <PlansPage />}
            </motion.div>
          </AnimatePresence>
        </ErrorBoundary>
      </main>
      <ClausePet
        view={view}
        go={(v) => { setActiveTabId(null); setView(v as View) }}
        askQuestion={reAsk}
        startNewChat={newChat}
      />
      <Toaster />
    </div>
  )
}
