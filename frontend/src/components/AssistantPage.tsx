import { useEffect, useRef, useState } from 'react'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { motion } from 'framer-motion'
import {
  ArrowUp, BadgeCheck, Check, ChevronDown, Copy, ExternalLink, Globe, Lock, PanelRight, RotateCcw, ShieldCheck, Square, ThumbsDown, ThumbsUp, X,
} from 'lucide-react'
import { useQuery } from '@tanstack/react-query'
import { mattersApi, privateVaultApi, publicSettingsApi, queryStream, ragApi } from '../api/client'
import AssistantAvatar from './AssistantAvatar'
import { SourceModal, citeMarkdownComponents, linkifyCitations } from './SourceModal'
import type { Citation, CitationAudit, QueryMode } from '../api/types'
import { useSession } from '../stores/session'
import { useMatter } from '../stores/matter'


interface Turn {
  id: number
  question: string
  mode: QueryMode
  status: string | null
  citations: Citation[]
  answer: string
  streaming: boolean
  done: boolean
  error: string | null
  scopeNote: string | null
  verified: boolean
  audit: CitationAudit | null
  expansions: string[]
  startedAt: number
  elapsedMs: number | null
}

const EXAMPLES: { title: string; q: string; tag: string }[] = [
  { tag: 'Statute', title: 'Interpret a provision', q: 'What does Section 86 of the Income-tax Act provide, and who can claim it?' },
  { tag: 'Analysis', title: 'Find the exceptions', q: 'What are the conditions and exceptions for excluding a member\u2019s share income from total income?' },
  { tag: 'Drafting', title: 'Explain for a client', q: 'Explain in plain English how income of an association of persons is taxed in members\u2019 hands.' },
  { tag: 'Research', title: 'Check the sharing rule', q: 'How is the share of a member in an AOP determined when individual shares are indeterminate?' },
]

const FOLLOW_UPS = [
  'Explain this in simpler terms',
  'What conditions or exceptions apply here?',
  'Turn this into a short client-facing summary',
]

let turnId = 0

export default function AssistantPage({ matterId: fixedMatterId, go }: { matterId?: string; go?: (view: string) => void } = {}) {
  const storeMatterId = useMatter((s) => s.currentMatterId)
  const setStoreMatter = useMatter((s) => s.setCurrentMatter)
  const scopeMatterId = fixedMatterId ?? storeMatterId
  const { data: matters } = useQuery({ queryKey: ['matters'], queryFn: mattersApi.list })
  const scopeMatter = matters?.find((m) => m.id === scopeMatterId)
  const [input, setInput] = useState('')
  const { data: pubSettings } = useQuery({ queryKey: ['public-settings'], queryFn: publicSettingsApi.get })
  const mode = (pubSettings?.default_mode as QueryMode | undefined) ?? 'mix'
  const [sources, setSources] = useState(5)
  const [web, setWeb] = useState(false)
  const [priv, setPriv] = useState(false)
  const [asOf, setAsOf] = useState('')
  const [jurisdiction, setJurisdiction] = useState('')
  const { data: vaultStatus } = useQuery({ queryKey: ['private-vault-status'], queryFn: privateVaultApi.status })
  const privAvailable = vaultStatus?.unlocked === true
  const [turns, setTurns] = useState<Turn[]>([])
  const abortRef = useRef<AbortController | null>(null)
  const bottomRef = useRef<HTMLDivElement>(null)
  const addHistory = useSession((s) => s.addHistory)
  const pendingQuestion = useSession((s) => s.pendingQuestion)
  const consumePending = useSession((s) => s.consumePending)
  const toast = useSession((s) => s.toast)

  const busy = turns.some((t) => t.streaming)
  const [sourcesOpen, setSourcesOpen] = useState(true)
  const [focusCite, setFocusCite] = useState<{ turnId: number; chunkId: string } | null>(null)
  const [sourceModal, setSourceModal] = useState<Citation | null>(null)
  const focusTurn = (focusCite ? turns.find((t) => t.id === focusCite.turnId) : undefined)
    ?? [...turns].reverse().find((t) => t.citations.length > 0)

  async function ask(questionRaw: string) {
    const question = questionRaw.trim()
    if (!question || busy) return
    setFocusCite(null)
    const id = ++turnId
    const turn: Turn = {
      id, question, mode, status: 'Connecting…', citations: [], answer: '',
      streaming: true, done: false, error: null, scopeNote: null, verified: false, audit: null, expansions: [], startedAt: Date.now(), elapsedMs: null,
    }
    setTurns((t) => [...t, turn])
    addHistory(question)
    setInput('')

    const ctrl = new AbortController()
    abortRef.current = ctrl
    let buffer = ''
    let flushTimer: ReturnType<typeof setTimeout> | null = null
    const flush = () => {
      flushTimer = null
      const text = buffer
      setTurns((ts) => ts.map((t) => (t.id === id ? { ...t, answer: t.answer + text } : t)))
      buffer = ''
    }
    const queueDelta = (text: string) => {
      buffer += text
      if (!flushTimer) flushTimer = setTimeout(flush, 30)
    }

    try {
      await queryStream(
        { query: question, mode, top_k: sources, web, web_results: 3, matter_id: scopeMatterId ?? undefined, private: priv && privAvailable ? true : undefined, as_of: asOf || undefined, jurisdiction: jurisdiction.trim() || undefined },
        {
          onStatus: (message) =>
            setTurns((ts) =>
              ts.map((t) =>
                t.id === id
                  ? { ...t, status: message, scopeNote: message.includes('full vault') ? message : t.scopeNote }
                  : t,
              ),
            ),
          onSources: (citations, m, verified) =>
            setTurns((ts) => ts.map((t) => (t.id === id ? { ...t, citations, mode: m, verified } : t))),
          onDelta: queueDelta,
          onDone: (answer, audit, expansions) => {
            if (flushTimer) { clearTimeout(flushTimer); flushTimer = null }
            const rest = buffer; buffer = ''
            setTurns((ts) =>
              ts.map((t) =>
                t.id === id
                  ? { ...t, answer: answer || t.answer + rest, audit: audit ?? t.audit, expansions: expansions ?? t.expansions, streaming: false, done: true, status: null, elapsedMs: Date.now() - t.startedAt }
                  : t,
              ),
            )
          },
          onError: (message) =>
            setTurns((ts) => ts.map((t) => (t.id === id ? { ...t, error: message, streaming: false, status: null } : t))),
        },
        ctrl.signal,
      )
    } catch (e) {
      if ((e as Error).name === 'AbortError') {
        if (flushTimer) { clearTimeout(flushTimer); flush() }
        setTurns((ts) => ts.map((t) => (t.id === id ? { ...t, streaming: false, status: null, error: 'Stopped.' } : t)))
      } else {
        // Stream transport failed — fall back to the blocking endpoint.
        try {
          const data = await ragApi.query(question, mode, sources, web, 3, scopeMatterId ?? undefined, priv && privAvailable ? true : undefined)
          setTurns((ts) =>
            ts.map((t) =>
              t.id === id
                ? { ...t, answer: data.answer, citations: data.citations, mode: data.mode, scopeNote: data.scope_note ?? null, verified: data.verified ?? false, audit: data.citation_audit ?? null, expansions: data.expansions ?? [], streaming: false, done: true, status: null, elapsedMs: Date.now() - t.startedAt }
                : t,
            ),
          )
        } catch (e2) {
          setTurns((ts) => ts.map((t) => (t.id === id ? { ...t, error: (e2 as Error).message, streaming: false, status: null } : t)))
        }
      }
    } finally {
      if (abortRef.current === ctrl) abortRef.current = null
    }
  }

  // Questions asked from the sidebar history / other views.
  useEffect(() => {
    if (pendingQuestion) {
      const q = consumePending()
      if (q) void ask(q)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pendingQuestion])

  // History restore: a dashboard click reopens the exact turn — original
  // answer, citations, and mode, with no new generation or billing.
  const restore = useSession((s) => s.restore)
  const consumeRestore = useSession((s) => s.consumeRestore)
  useEffect(() => {
    if (!restore || restore.kind !== 'query') return
    const p = consumeRestore('query')
    if (!p) return
    const id = ++turnId
    setTurns((ts) => [
      ...ts,
      {
        id,
        question: String(p.question ?? ''),
        mode: (typeof p.mode === 'string' ? p.mode : mode) as QueryMode,
        status: null,
        citations: (p.citations as Citation[]) ?? [],
        answer: String(p.answer ?? ''),
        streaming: false,
        done: true,
        error: null,
        scopeNote: typeof p.scope_note === 'string' ? p.scope_note : null,
        verified: p.verified === true,
        audit: (p.citation_audit as CitationAudit) ?? null,
        expansions: (p.expansions as string[]) ?? [],
        startedAt: Date.now(),
        elapsedMs: null,
      },
    ])
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [restore])

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth', block: 'end' })
  }, [turns])

  const empty = turns.length === 0

  return (
    <div className="flex items-start">
      <div className="flex-1 min-w-0">
        <div className="max-w-3xl mx-auto px-6 pb-16">
      {scopeMatterId && (
        <div className="mt-6 flex items-center gap-2 rounded-lex border border-stone-200 bg-white px-3.5 py-2.5 text-[13px] shadow-lex">
          <span className="w-1.5 h-1.5 rounded-full bg-emerald-600 flex-shrink-0" />
          <span className="text-stone-600">
            Asking within <span className="font-medium text-ink">{scopeMatter?.title ?? 'this brief'}</span>
            {' '}— answers draw only on this brief's documents.
          </span>
          {!fixedMatterId && (
            <button onClick={() => setStoreMatter(null)} className="ml-auto text-stone-500 hover:text-ink font-medium whitespace-nowrap">
              Search full vault instead
            </button>
          )}
        </div>
      )}
      {empty ? (
        <div className="pt-16">
          <motion.div initial={{ opacity: 0, y: 14 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.5, ease: [0.16, 1, 0.3, 1] }}>
            <div className="flex justify-center"><AssistantAvatar size={84} /></div>
            <p className="text-center text-[12px] font-semibold uppercase tracking-[0.16em] text-stone-400 mt-5">
              Clause · your legal research companion
            </p>
            <h1 className="font-display text-[2.7rem] leading-[1.12] tracking-tight text-center mt-3">
              What would you like<br />to <em className="italic">research</em> today?
            </h1>
            <p className="text-center text-stone-500 mt-4 text-[15px] max-w-md mx-auto">
              Grounded answers from your Vault, streamed as they're drafted — every claim cited to its source.
            </p>
          </motion.div>
          <Composer
            input={input} setInput={setInput}             sources={sources} setSources={setSources} web={web} setWeb={setWeb} priv={priv} setPriv={setPriv} privAvailable={privAvailable}
            asOf={asOf} setAsOf={setAsOf} jurisdiction={jurisdiction} setJurisdiction={setJurisdiction}
            busy={busy} onSubmit={() => ask(input)} onStop={() => abortRef.current?.abort()}
          />
          <div className="mt-8 grid sm:grid-cols-2 gap-3">
            {EXAMPLES.map((ex, i) => (
              <motion.button
                key={ex.title}
                initial={{ opacity: 0, y: 10 }} animate={{ opacity: 1, y: 0 }}
                transition={{ delay: 0.15 + i * 0.07, duration: 0.4 }}
                onClick={() => ask(ex.q)}
                className="text-left rounded-lex-lg border border-stone-200 bg-white shadow-lex px-4 py-3.5 hover:border-stone-300 hover:shadow-lex-md transition-all group"
              >
                <span className="text-[10px] font-semibold uppercase tracking-[0.14em] text-stone-400">{ex.tag}</span>
                <p className="font-display text-[17px] mt-1 group-hover:text-ink">{ex.title}</p>
                <p className="text-[13px] text-stone-500 mt-0.5 line-clamp-2">{ex.q}</p>
              </motion.button>
            ))}
          </div>
        </div>
      ) : (
        <div className="pt-8">
          {turns.some((t) => t.citations.length > 0) && (
            <div className="flex justify-end mb-1">
              <button
                onClick={() => setSourcesOpen((v) => !v)}
                className="inline-flex items-center gap-1.5 rounded-full border border-stone-200 bg-white px-3 py-1.5 text-[12.5px] font-medium text-stone-600 hover:text-ink hover:border-stone-300 transition-colors shadow-lex-sm"
              >
                <PanelRight size={14} /> Sources panel {sourcesOpen ? 'on' : 'off'}
              </button>
            </div>
          )}
          {turns.map((turn) => (
            <TurnView
              key={turn.id} turn={turn}
              onCopy={() => { navigator.clipboard.writeText(turn.answer).catch(() => {}); toast('success', 'Answer copied') }}
              onRegenerate={() => ask(turn.question)}
              onFollowUp={(f) => ask(`${f} — in the context of: "${turn.question}"`)}
              onCite={(c) => setSourceModal(c)}
              onShowSources={() => { if (turn.citations[0]) setFocusCite({ turnId: turn.id, chunkId: turn.citations[0].chunk_id }); setSourcesOpen(true) }}
            />
          ))}
          <div ref={bottomRef} />
          <div className="sticky bottom-4">
            <Composer
              input={input} setInput={setInput}               sources={sources} setSources={setSources} web={web} setWeb={setWeb} priv={priv} setPriv={setPriv} privAvailable={privAvailable}
              asOf={asOf} setAsOf={setAsOf} jurisdiction={jurisdiction} setJurisdiction={setJurisdiction}
              busy={busy} onSubmit={() => ask(input)} onStop={() => abortRef.current?.abort()}
            />
          </div>
        </div>
      )}
        </div>
      </div>
      {!empty && focusTurn && sourcesOpen && (
        <SourcesSidebar
          turn={focusTurn}
          selectedChunkId={focusCite && focusCite.turnId === focusTurn.id ? focusCite.chunkId : null}
          onClose={() => setSourcesOpen(false)}
          go={go}
          onOpenSource={(c) => setSourceModal(c)}
        />
      )}
      {sourceModal && (
        <SourceModal citation={sourceModal} onClose={() => setSourceModal(null)} go={go} />
      )}
    </div>
  )
}


/**
 * Reveal streamed text like handwriting: deltas land in a buffer and a
 * rAF loop writes them out at a steady, adaptive pace — so the answer
 * reads as one continuous generation instead of bursty dumps.
 */
function useTypewriter(full: string, active: boolean): string {
  const [shown, setShown] = useState(() => (active ? '' : full))
  const lenRef = useRef(active ? 0 : full.length)
  const fullRef = useRef(full)
  fullRef.current = full

  useEffect(() => {
    if (!active) {
      lenRef.current = fullRef.current.length
      setShown(fullRef.current)
      return
    }
    let raf = 0
    const step = () => {
      const target = fullRef.current.length
      if (lenRef.current < target) {
        const backlog = target - lenRef.current
        const advance = Math.max(2, Math.ceil(backlog / 5))
        lenRef.current = Math.min(target, lenRef.current + advance)
        setShown(fullRef.current.slice(0, lenRef.current))
        raf = requestAnimationFrame(step)
      }
    }
    raf = requestAnimationFrame(step)
    return () => cancelAnimationFrame(raf)
  }, [active, full])

  return active ? shown : full
}

const HIGHLIGHT_STOP = new Set([
  'what', 'does', 'the', 'and', 'for', 'with', 'under', 'from', 'this', 'that', 'which',
  'about', 'explain', 'give', 'tell', 'please', 'document', 'documents', 'there', 'their',
  'into', 'have', 'been', 'were', 'will', 'would', 'should', 'could', 'when', 'where',
])

function questionTerms(q: string): string[] {
  const words = q.toLowerCase().match(/[a-z0-9][a-z0-9\-]{1,}/g) ?? []
  return [...new Set(words.filter((w) => w.length >= 3 || /^\d+$/.test(w)).filter((w) => !HIGHLIGHT_STOP.has(w)))]
    .sort((a, b) => b.length - a.length)
    .slice(0, 14)
}

/** Text with the question's key terms highlighted — the overlap between
 *  what was asked and the passage the answer drew from. */
function Highlighted({ text, terms }: { text: string; terms: string[] }) {
  if (terms.length === 0) return <>{text}</>
  const pattern = new RegExp(`(${terms.map((t) => t.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|')})`, 'gi')
  const parts = text.split(pattern)
  return (
    <>
      {parts.map((part, i) =>
        i % 2 === 1 ? (
          <mark key={i} className="bg-[#fdf3d7] text-ink rounded px-0.5">{part}</mark>
        ) : (
          <span key={i}>{part}</span>
        ),
      )}
    </>
  )
}

/** The right-hand sources panel: the passages behind the answer, with
 *  the used text highlighted. Follows the latest turn unless a citation
 *  chip pinned a specific source. */
function SourcesSidebar({ turn, selectedChunkId, onClose, go, onOpenSource }: {
  turn: Turn
  selectedChunkId: string | null
  onClose: () => void
  go?: (view: string) => void
  onOpenSource: (c: Citation) => void
}) {
  const terms = questionTerms(turn.question)
  const cardRefs = useRef<Record<string, HTMLDivElement | null>>({})

  useEffect(() => {
    if (selectedChunkId) {
      cardRefs.current[selectedChunkId]?.scrollIntoView({ behavior: 'smooth', block: 'center' })
    }
  }, [selectedChunkId, turn.id])

  return (
    <aside className="sticky top-0 h-screen w-[350px] shrink-0 border-l border-stone-200 bg-[#fbfaf8] hidden lg:flex flex-col">
      <div className="flex items-center gap-2 px-5 h-[4.5rem] border-b border-stone-200 shrink-0">
        <PanelRight size={16} className="text-stone-500" />
        <p className="font-display text-[17px]">Sources</p>
        <span className="text-[12px] text-stone-400">{turn.citations.length} passage{turn.citations.length === 1 ? '' : 's'}</span>
        <button onClick={onClose} className="ml-auto p-1.5 rounded-lex text-stone-400 hover:text-ink hover:bg-stone-100 transition-colors" title="Close sources panel">
          <X size={15} />
        </button>
      </div>
      <div className="px-5 py-3 border-b border-stone-100 shrink-0">
        <p className="text-[11px] uppercase tracking-wider text-stone-400 font-medium">For the question</p>
        <p className="text-[13.5px] font-medium mt-0.5 line-clamp-2">{turn.question}</p>
        <p className="text-[12px] text-stone-400 mt-1">Highlighted words are where the answer drew from the passage.</p>
      </div>
      <div className="flex-1 overflow-y-auto px-4 py-4 space-y-3">
        {turn.citations.map((c) => {
          const selected = c.chunk_id === selectedChunkId
          const isWeb = !!c.source_url
          const isDoc = c.doc_id.startsWith('doc:')
          const isPrivate = c.doc_id.startsWith('private:')
          const domain = c.source_url
            ? (() => { try { return new URL(c.source_url).hostname.replace(/^www\./, '') } catch { return 'source' } })()
            : null
          const openDoc = async () => {
            try {
              const resolved = await ragApi.sourceDocument({ chunk_id: c.chunk_id })
              if (!resolved.document_id) throw new Error('unresolved')
              sessionStorage.setItem('jurisvault.interactDoc', resolved.document_id)
              go?.('interact')
            } catch {
              onOpenSource(c)
            }
          }
          return (
            <div
              key={c.chunk_id}
              ref={(el) => { cardRefs.current[c.chunk_id] = el }}
              onClick={() => onOpenSource(c)}
              title="View the full document"
              className={`rounded-lex-lg border bg-white shadow-lex px-4 py-3.5 transition-colors cursor-pointer hover:border-stone-400 ${selected ? 'border-ink ring-1 ring-ink/60' : 'border-stone-200'}`}
            >
              <div className="flex items-center gap-2">
                <span className="rounded bg-ink text-ivory px-1.5 py-0.5 text-[11px] font-mono font-semibold">{c.doc_id}</span>
                <span className="rounded bg-stone-100 border border-stone-200 px-1.5 py-0.5 text-[11px] font-mono font-semibold text-stone-600">
                  {Math.round(c.score * 100)}%
                </span>
                {isPrivate && (
                  <span className="rounded-full bg-ink text-ivory px-1.5 py-px text-[10px] font-semibold uppercase tracking-wide">Private</span>
                )}
                {c.stale && (
                  <span
                    title={c.stale}
                    className="rounded-full bg-amber-50 border border-amber-600/30 text-amber-800 px-1.5 py-px text-[10px] font-semibold uppercase tracking-wide"
                  >
                    Superseded
                  </span>
                )}
                {isWeb && (
                  <span className="ml-auto text-[10px] font-semibold uppercase tracking-wide text-stone-400">Web</span>
                )}
              </div>
              <p className="text-[13.5px] font-medium mt-2 leading-snug">{c.doc_title ?? c.doc_id}</p>
              <div className="text-[12.5px] text-stone-600 leading-relaxed mt-1.5 max-h-72 overflow-y-auto pr-1">
                <Highlighted text={c.snippet} terms={terms} />
              </div>
              {c.source_url && (
                <a
                  href={c.source_url}
                  target="_blank"
                  rel="noreferrer"
                  onClick={(e) => e.stopPropagation()}
                  className="mt-3 flex items-center gap-1.5 border-t border-stone-100 pt-2.5 text-[12.5px] font-medium text-ink hover:underline underline-offset-2"
                >
                  <ExternalLink size={12} />
                  Open original
                  <span className="text-stone-400 font-normal">
                    {domain}
                    {c.source_date && <> · {c.source_date}</>}
                  </span>
                </a>
              )}
              {isDoc && go && (
                <button
                  onClick={(e) => { e.stopPropagation(); openDoc() }}
                  className="mt-3 flex items-center gap-1.5 border-t border-stone-100 pt-2.5 text-[12.5px] font-medium text-ink hover:underline underline-offset-2"
                >
                  <PanelRight size={12} />
                  Open this document in Interact
                </button>
              )}
              {isPrivate && go && (
                <button
                  onClick={(e) => { e.stopPropagation(); go('private') }}
                  className="mt-3 flex items-center gap-1.5 border-t border-stone-100 pt-2.5 text-[12.5px] font-medium text-ink hover:underline underline-offset-2"
                >
                  <Lock size={12} />
                  Open in Private Vault
                </button>
              )}
            </div>
          )
        })}
      </div>
    </aside>
  )
}

function TurnView({ turn, onCopy, onRegenerate, onFollowUp, onCite, onShowSources }: {
  turn: Turn
  onCopy: () => void
  onRegenerate: () => void
  onFollowUp: (f: string) => void
  onCite: (c: Citation) => void
  onShowSources: () => void
}) {
  const shownAnswer = useTypewriter(turn.answer, turn.streaming)
  const [vote, setVote] = useState<'up' | 'down' | null>(null)
  const [voting, setVoting] = useState(false)
  const [voteError, setVoteError] = useState<string | null>(null)
  const castVote = async (v: 'up' | 'down') => {
    if (voting) return
    setVoting(true)
    setVoteError(null)
    try {
      await ragApi.voteFeedback(turn.question, turn.answer, v)
      setVote(v)
    } catch (e) {
      setVoteError((e as Error).message)
    } finally {
      setVoting(false)
    }
  }
  const [tick, setTick] = useState(0)
  useEffect(() => {
    if (!turn.streaming) return
    const h = setInterval(() => setTick((v) => v + 1), 500)
    return () => clearInterval(h)
  }, [turn.streaming])
  void tick
  const secs = ((turn.elapsedMs ?? Date.now() - turn.startedAt) / 1000).toFixed(1)

  return (
    <motion.div initial={{ opacity: 0, y: 12 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.4 }} className="mb-10">
      <h2 className="font-display text-[1.6rem] leading-snug tracking-tight">{turn.question}</h2>

      {turn.status && (
        <div className="mt-3 flex items-center gap-2.5 text-stone-500 text-[13px]">
          <AssistantAvatar size={22} playing float={false} />
          <span className="font-medium text-ink">Clause</span>
          {turn.status}
          <span className="text-stone-300">·</span>
          <span className="tabular-nums">{secs}s</span>
        </div>
      )}

      {turn.error ? (
        <div className="mt-4 rounded-lex border border-brand-red/30 bg-[#feeaea] px-4 py-3 text-[13px] text-[#a02c2c] flex items-center justify-between">
          <span>{turn.error}</span>
          <button onClick={onRegenerate} className="flex items-center gap-1.5 font-medium underline underline-offset-2">
            <RotateCcw size={13} /> Retry
          </button>
        </div>
      ) : (
        <>
          {turn.answer && (
            <div className="prose-lex mt-3">
              <ReactMarkdown
                remarkPlugins={[remarkGfm]}
                components={citeMarkdownComponents(turn.citations, onCite)}
              >
                {linkifyCitations(shownAnswer, turn.citations)}
              </ReactMarkdown>
              {turn.streaming && <span className="inline-block w-[7px] h-[15px] bg-ink/70 ml-0.5 animate-pulse align-[-2px]" />}
            </div>
          )}

          {turn.scopeNote && (
            <div className="mt-4 rounded-lex border border-amber-600/25 bg-[#fdf6e3] px-4 py-2.5 text-[13px] text-[#7a5b12]">
              {turn.scopeNote}
            </div>
          )}

          {turn.verified && (
            <div className="mt-4 inline-flex items-center gap-1.5 rounded-full bg-[#e7f4e4] text-[#2f6b2f] px-3 py-1 text-[12.5px] font-medium">
              <BadgeCheck size={14} />
              Verified answer — curated and checked by a human
            </div>
          )}

          {turn.done && turn.audit && turn.audit.checked > 0 && (
            <div className="mt-4 inline-flex items-center gap-1.5 rounded-full bg-stone-100 border border-stone-200 text-stone-600 px-3 py-1 text-[12.5px] font-medium">
              <ShieldCheck size={14} className="text-[#2f6b2f]" />
              Citation integrity: {turn.audit.verified} of {turn.audit.checked} verified against their sources
              {turn.audit.stripped > 0 && (
                <span className="text-[#9a6a12]">· {turn.audit.stripped} unsupported removed</span>
              )}
            </div>
          )}

          {turn.done && turn.expansions.length > 0 && (
            <div className="mt-4">
              <p className="text-[11.5px] font-medium uppercase tracking-wide text-stone-400">
                Question expanded for retrieval — also searched as
              </p>
              <div className="mt-1.5 flex flex-wrap gap-1.5">
                {turn.expansions.map((x, i) => (
                  <span key={i} className="inline-block max-w-full truncate rounded-full border border-stone-200 bg-white px-2.5 py-1 text-[12px] text-stone-600" title={x}>
                    {x}
                  </span>
                ))}
              </div>
            </div>
          )}

          {turn.citations.length > 0 && (
            <button
              onClick={onShowSources}
              className="mt-4 inline-flex items-center gap-1.5 text-[12.5px] font-medium text-stone-500 hover:text-ink transition-colors"
            >
              <PanelRight size={13} />
              {turn.citations.length} source{turn.citations.length === 1 ? '' : 's'} in the panel · {turn.mode} mode
              {turn.done && turn.elapsedMs ? ` · answered in ${(turn.elapsedMs / 1000).toFixed(1)}s` : ''}
            </button>
          )}

          {turn.done && (
            <div className="mt-5">
              <div className="flex items-center gap-2">
                <button onClick={onCopy} className="flex items-center gap-1.5 rounded-lex border border-stone-200 bg-white px-2.5 py-1.5 text-[12px] font-medium text-stone-600 hover:text-ink hover:border-stone-300 transition-colors">
                  <Copy size={13} /> Copy
                </button>
                <button onClick={onRegenerate} className="flex items-center gap-1.5 rounded-lex border border-stone-200 bg-white px-2.5 py-1.5 text-[12px] font-medium text-stone-600 hover:text-ink hover:border-stone-300 transition-colors">
                  <RotateCcw size={13} /> Regenerate
                </button>
                <button
                  onClick={() => void castVote('up')}
                  disabled={voting}
                  title="This answer was helpful"
                  className={`flex items-center gap-1.5 rounded-lex border px-2.5 py-1.5 text-[12px] font-medium transition-colors disabled:opacity-40 ${vote === 'up' ? 'bg-ink text-ivory border-ink' : 'border-stone-200 bg-white text-stone-600 hover:text-ink hover:border-stone-300'}`}
                >
                  <ThumbsUp size={13} /> Helpful
                </button>
                <button
                  onClick={() => void castVote('down')}
                  disabled={voting}
                  title="This answer was not helpful"
                  className={`flex items-center gap-1.5 rounded-lex border px-2.5 py-1.5 text-[12px] font-medium transition-colors disabled:opacity-40 ${vote === 'down' ? 'bg-ink text-ivory border-ink' : 'border-stone-200 bg-white text-stone-600 hover:text-ink hover:border-stone-300'}`}
                >
                  <ThumbsDown size={13} /> Not helpful
                </button>
                {vote && <span className="text-[12px] text-stone-400">Feedback saved</span>}
              </div>
              {voteError && <p className="mt-2 text-[12px] text-[#a02c2c]">Could not save feedback: {voteError}</p>}
              <div className="mt-4 flex flex-wrap gap-2">
                {FOLLOW_UPS.map((f) => (
                  <button
                    key={f}
                    onClick={() => onFollowUp(f)}
                    className="rounded-full border border-stone-200 bg-white px-3 py-1.5 text-[13px] text-stone-600 hover:border-stone-300 hover:text-ink transition-colors shadow-lex"
                  >
                    {f}
                  </button>
                ))}
              </div>
            </div>
          )}
        </>
      )}
    </motion.div>
  )
}

function Composer(props: {
  input: string; setInput: (v: string) => void
  sources: number; setSources: (n: number) => void
  web: boolean; setWeb: (v: boolean) => void
  priv: boolean; setPriv: (v: boolean) => void; privAvailable: boolean
  asOf: string; setAsOf: (v: string) => void
  jurisdiction: string; setJurisdiction: (v: string) => void
  busy: boolean; onSubmit: () => void; onStop: () => void
}) {
  const { input, setInput, sources, setSources, web, setWeb, priv, setPriv, privAvailable, asOf, setAsOf, jurisdiction, setJurisdiction, busy, onSubmit, onStop } = props
  const [srcOpen, setSrcOpen] = useState(false)
  const [lawOpen, setLawOpen] = useState(false)
  const lawActive = !!(asOf || jurisdiction.trim())
  return (
    <div className="mt-8 rounded-lex-lg border border-stone-300 bg-white shadow-lex-md">
      <textarea
        value={input}
        onChange={(e) => setInput(e.target.value)}
        onKeyDown={(e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); onSubmit() } }}
        placeholder="Ask anything about your documents…"
        rows={3}
        autoFocus
        className="w-full resize-none px-4 pt-4 pb-2 text-[15px] outline-none placeholder:text-stone-400 bg-transparent rounded-t-lex-lg"
      />
      <div className="flex items-center justify-between px-3 pb-3">
        <div className="flex items-center gap-2">
          <button
            onClick={() => setWeb(!web)}
            title="Blend live web results into the answer"
            className={`flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-[12px] transition-colors ${
              web ? 'bg-ink text-ivory border-ink' : 'border-stone-200 text-stone-500 hover:text-ink hover:border-stone-300'
            }`}
          >
            <Globe size={13} /> Web
          </button>
          {privAvailable && (
            <button
              onClick={() => setPriv(!priv)}
              title="Include your confidential vault documents (only you can see them)"
              className={`flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-[12px] transition-colors ${
                priv ? 'bg-ink text-ivory border-ink' : 'border-stone-200 text-stone-500 hover:text-ink hover:border-stone-300'
              }`}
            >
              <Lock size={13} /> Private
            </button>
          )}
          <div className="relative">
            <button
              onClick={() => setLawOpen((v) => !v)}
              title="Only cite law in force on a date / in a jurisdiction"
              className={`flex items-center gap-1 rounded-full border px-2.5 py-1 text-[12px] transition-colors ${
                lawActive ? 'bg-ink text-ivory border-ink' : 'border-stone-200 text-stone-500 hover:text-ink hover:border-stone-300'
              }`}
            >
              As of{lawActive ? `: ${asOf || jurisdiction}` : ''} <ChevronDown size={12} />
            </button>
            {lawOpen && (
              <div className="absolute bottom-full mb-1.5 left-0 rounded-lex-lg border border-stone-200 bg-white shadow-lex-md p-3 z-10 w-60 space-y-2.5">
                <label className="block">
                  <span className="text-[11px] font-medium uppercase tracking-wide text-stone-500">In force on</span>
                  <input
                    type="date"
                    value={asOf}
                    onChange={(e) => setAsOf(e.target.value)}
                    className="mt-1 w-full rounded-lex border border-stone-200 px-2 py-1.5 text-[13px] outline-none focus:border-stone-400"
                  />
                </label>
                <label className="block">
                  <span className="text-[11px] font-medium uppercase tracking-wide text-stone-500">Jurisdiction</span>
                  <input
                    type="text"
                    value={jurisdiction}
                    onChange={(e) => setJurisdiction(e.target.value)}
                    placeholder="e.g. IN, IN-DL, US-CA"
                    className="mt-1 w-full rounded-lex border border-stone-200 px-2 py-1.5 text-[13px] outline-none focus:border-stone-400 placeholder:text-stone-400"
                  />
                </label>
                <div className="flex items-center justify-between pt-0.5">
                  <button
                    onClick={() => { setAsOf(''); setJurisdiction('') }}
                    className="text-[12px] text-stone-500 hover:text-ink"
                  >
                    Clear
                  </button>
                  <button
                    onClick={() => setLawOpen(false)}
                    className="rounded-full bg-ink text-ivory px-3 py-1 text-[12px] font-medium"
                  >
                    Done
                  </button>
                </div>
                <p className="text-[11px] leading-relaxed text-stone-400">
                  Only documents in force on that date are cited — no fallback to newer or older law.
                </p>
              </div>
            )}
          </div>
          <div className="relative">
            <button
              onClick={() => setSrcOpen((v) => !v)}
              className="flex items-center gap-1 rounded-full border border-stone-200 px-2.5 py-1 text-[12px] text-stone-500 hover:text-ink hover:border-stone-300 transition-colors"
            >
              Sources: {sources} <ChevronDown size={12} />
            </button>
            {srcOpen && (
              <div className="absolute bottom-full mb-1.5 left-0 rounded-lex-lg border border-stone-200 bg-white shadow-lex-md py-1 z-10 w-36">
                {[3, 5, 10, 20].map((n) => (
                  <button
                    key={n}
                    onClick={() => { setSources(n); setSrcOpen(false) }}
                    className="w-full flex items-center justify-between px-3 py-1.5 text-[13px] hover:bg-stone-100 text-left"
                  >
                    {n} sources {sources === n && <Check size={13} />}
                  </button>
                ))}
              </div>
            )}
          </div>
        </div>
        {busy ? (
          <button
            onClick={onStop}
            className="flex items-center gap-1.5 rounded-full bg-[#a02c2c] text-white pl-3 pr-3.5 py-1.5 text-[13px] font-medium hover:bg-[#8a2424] transition-colors"
          >
            <Square size={12} /> Stop
          </button>
        ) : (
          <button
            onClick={onSubmit} disabled={!input.trim()}
            className="w-8 h-8 rounded-full bg-ink text-ivory flex items-center justify-center hover:bg-ink-900 disabled:opacity-40 transition-colors"
            title="Ask (Enter)"
          >
            <ArrowUp size={16} />
          </button>
        )}
      </div>
      <div className="border-t border-stone-200 bg-stone-100/60 px-3 py-2 flex rounded-b-lex-lg">
        <span className="ml-auto hidden sm:block text-[11px] text-stone-400 self-center">Enter to ask · Shift+Enter for a new line</span>
      </div>
    </div>
  )
}
