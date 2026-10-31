import { useEffect, useRef, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { AnimatePresence, motion } from 'framer-motion'
import {
  MessageSquare, FlaskConical, Bot, FolderOpen, Network, LogOut, LayoutDashboard,
  PenSquare, Trash2, CreditCard, FileSearch, PenLine, Briefcase, Lock, ExternalLink, PanelTop, Search, X,
} from 'lucide-react'
import { useAuth } from '../stores/auth'
import { useSession } from '../stores/session'
import { useMatter } from '../stores/matter'
import { dashboardApi, mattersApi, publicSettingsApi } from '../api/client'
import { ConfirmDialog, DualToneBackdrop, ThemeToggle } from './ui'
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
  // Piece 33 consent: sidebar history removals (single + clear-all)
  // ask first through the shared dialog, like every permanent delete.
  const [historyRemoval, setHistoryRemoval] = useState<string | 'all' | null>(null)
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
  const removeHistory = useSession((s) => s.removeHistory)
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
    <div className="h-screen flex relative app-wash text-ink dark:text-[#f4f2ee] overflow-hidden">
      {/* Piece 36: one glow layer fixed behind the whole app, over the
          page wash — the taste-skill page's background system. */}
      <DualToneBackdrop fixed />
      {/* Sidebar — transparent over the wash, hairline right border */}
      <aside className="relative z-10 w-64 flex-shrink-0 border-r border-stone-200 dark:border-white/10 flex flex-col">
        <div className="h-[4.5rem] flex items-center px-5 border-b border-stone-200 dark:border-white/10">
          <span className="font-display text-xl tracking-tight">{brand.product_name}</span>
        </div>

        <div className="px-3 pt-4">
          <button
            onClick={newChat}
            className="w-full flex items-center gap-2.5 rounded-lex bg-ink text-ivory px-3 py-2.5 text-[14px] font-medium hover:bg-ink-900 transition-colors"
          >
            <PenSquare size={15} />
            New research
            <span className="ml-auto text-[10px] text-ivory/50 font-normal">⌘⇧N</span>
          </button>

          {/* Matter switcher: scopes Ask, Draft, Interact, Vault and
              uploads to one brief. Brief-only vault — the switcher
              only chooses WHICH brief; there is no full-vault view. */}
          <div className="mt-3">
            <p className="text-[12.5px] text-stone-400 dark:text-stone-500 font-medium px-1">Working in</p>
            <select
              value={effectiveMatterId ?? ''}
              onChange={(e) => setCurrentMatter(e.target.value)}
              className="mt-1.5 w-full rounded-lex border border-stone-200 dark:border-white/10 bg-white dark:bg-[#1d1f19] px-2.5 py-2 text-[13px] outline-none focus:border-stone-400"
            >
              {(mattersList ?? []).map((m) => (
                <option key={m.id} value={m.id}>{m.title}</option>
              ))}
            </select>
            {effectiveMatterId && (
              <button
                onClick={() => openMatter(effectiveMatterId)}
                className="mt-1.5 px-1 text-[12px] text-stone-500 dark:text-stone-400 hover:text-ink font-medium"
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
                  ? 'bg-stone-100 dark:bg-[#26281f] text-ink dark:text-[#f4f2ee] font-medium'
                  : 'text-stone-600 dark:text-stone-300 hover:bg-stone-100/70 dark:hover:bg-white/5 hover:text-ink'
              }`}
            >
              <Icon size={16} strokeWidth={1.8} />
              {label}
            </button>
          ))}

          {user?.role === 'admin' && (
            <a
              href="/admin/"
              className="w-full flex items-center gap-3 rounded-lex px-3 py-2 text-[14px] text-stone-600 dark:text-stone-300 hover:bg-stone-100/70 dark:hover:bg-white/5 hover:text-ink transition-colors"
            >
              <LayoutDashboard size={16} strokeWidth={1.8} />
              Command Center
              <span className="ml-auto text-[12px] text-stone-400 dark:text-stone-500">Admin</span>
            </a>
          )}

          {customTabs.map((t) =>
            t.kind === 'link' ? (
              <a
                key={t.id}
                href={t.url}
                target="_blank"
                rel="noreferrer"
                className="w-full flex items-center gap-3 rounded-lex px-3 py-2 text-[14px] text-stone-600 dark:text-stone-300 hover:bg-stone-100/70 dark:hover:bg-white/5 hover:text-ink transition-colors"
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
                    ? 'bg-stone-100 dark:bg-[#26281f] text-ink dark:text-[#f4f2ee] font-medium'
                    : 'text-stone-600 dark:text-stone-300 hover:bg-stone-100/70 dark:hover:bg-white/5 hover:text-ink'
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
                <p className="text-[12.5px] text-stone-400 dark:text-stone-500 font-medium">Recent research</p>
                <button onClick={() => setHistoryRemoval('all')} title="Clear history" className="text-stone-400 dark:text-stone-500 hover:text-ink p-0.5">
                  <Trash2 size={12} />
                </button>
              </div>
              <div className="mt-1.5 space-y-0.5">
                {history.map((h) => (
                  <div key={h} className="group relative flex items-center">
                    <button
                      onClick={() => reAsk(h)}
                      title={h}
                      className="w-full text-left rounded-lex px-3 py-1.5 pr-7 text-[13px] text-stone-600 dark:text-stone-300 hover:bg-stone-100/70 dark:hover:bg-white/5 hover:text-ink truncate transition-colors"
                    >
                      {h}
                    </button>
                    <button
                      onClick={() => setHistoryRemoval(h)}
                      title="Remove from history"
                      className="absolute right-1.5 p-0.5 rounded text-stone-400 dark:text-stone-500 hover:text-ink opacity-0 group-hover:opacity-100 focus:opacity-100 transition-opacity"
                    >
                      <X size={12} />
                    </button>
                  </div>
                ))}
              </div>
              {historyRemoval === 'all' ? (
                <ConfirmDialog
                  title="Clear your recent research history?"
                  body="Every entry in the sidebar's Recent research list will be removed permanently. Your Dashboard history and saved work are not affected."
                  confirmLabel="Clear history"
                  onConfirm={() => { clearHistory(); setHistoryRemoval(null) }}
                  onCancel={() => setHistoryRemoval(null)}
                />
              ) : historyRemoval ? (
                <ConfirmDialog
                  title="Remove this from your history?"
                  body={`"${historyRemoval}" will be removed from your Recent research list permanently.`}
                  confirmLabel="Remove permanently"
                  onConfirm={() => { removeHistory(historyRemoval); setHistoryRemoval(null) }}
                  onCancel={() => setHistoryRemoval(null)}
                />
              ) : null}
            </div>
          )}

          {dash && (
            <button
              onClick={() => { setActiveTabId(null); setView('plans') }}
              className="mt-6 mx-3 block text-left border-t border-stone-200 dark:border-white/10 px-1 pt-3 pb-1"
            >
              <span className="flex items-center justify-between">
                <span className="text-[12.5px] text-stone-400 dark:text-stone-500 font-medium">Your plan</span>
                <span className="text-[12px] text-stone-500 dark:text-stone-400">{dash.plan_name}</span>
              </span>
              <span className="block text-[19px] font-medium tabular-nums leading-tight mt-1.5">
                {Math.max(0, dash.token_quota - dash.tokens_used).toLocaleString('en-IN')}
                <span className="text-[12.5px] text-stone-500 dark:text-stone-400 font-normal"> tokens left</span>
              </span>
              <span className="block h-1.5 rounded-full bg-stone-100 dark:bg-[#26281f] mt-2 overflow-hidden">
                <span
                  className={`block h-full rounded-full ${Math.round((dash.tokens_used / Math.max(1, dash.token_quota)) * 100) >= 90 ? 'bg-[#a02c2c]' : 'dual-tone-bar'}`}
                  style={{ width: `${Math.min(100, Math.round((dash.tokens_used / Math.max(1, dash.token_quota)) * 100))}%` }}
                />
              </span>
              <span className="block text-[11.5px] text-stone-400 dark:text-stone-500 mt-1.5">
                {dash.tokens_used.toLocaleString('en-IN')} of {dash.token_quota.toLocaleString('en-IN')} used this period
              </span>
            </button>
          )}
        </nav>

        <div className="border-t border-stone-200 dark:border-white/10 px-4 py-2">
          <ClausePetToggle />
        </div>

        <div className="border-t border-stone-200 dark:border-white/10 px-4 py-3.5 flex items-center justify-between">
          <div className="min-w-0">
            <p className="text-[13px] font-medium truncate">{user?.email ?? 'Signed in'}</p>
            <p className="text-[11px] text-stone-400 dark:text-stone-500">{user?.role === 'admin' ? 'Administrator' : `${brand.product_name} workspace`}</p>
          </div>
          <span className="flex items-center gap-0.5 shrink-0">
            <ThemeToggle />
            <button onClick={logout} title="Sign out" className="p-2 rounded-lex text-stone-500 dark:text-stone-400 hover:bg-stone-100 dark:hover:bg-white/10 hover:text-ink transition-colors">
              <LogOut size={16} />
            </button>
          </span>
        </div>
      </aside>

      {/* Main */}
      <main className="flex-1 min-w-0 overflow-y-auto relative z-10">
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
