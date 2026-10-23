import { useCallback, useEffect, useRef, useState } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import { EyeOff, MessageSquare, RotateCcw } from 'lucide-react'
import { useSession } from '../stores/session'

/**
 * Clause — the glass pet, v2. A floating companion for Juris Vault AI who:
 *  - can be dragged anywhere on screen (position persists),
 *  - can be resized from his menu (size persists),
 *  - changes facial expressions (content / happy / thinking / curious /
 *    sleepy, plus blinking),
 *  - knows every page: on each view he greets with a page-specific line,
 *    cycles page-specific tips, and offers page-specific shortcuts,
 *  - can be switched off and back on (sidebar "Clause pet" switch).
 * All prefs live in localStorage under `jurisvault.clausePet*`.
 */

const PET_KEY = 'jurisvault.clausePet'
const SIZE_KEY = 'jurisvault.clausePet.size'
const POS_KEY = 'jurisvault.clausePet.pos'
const PET_EVENT = 'clause-pet-changed'

const SIZE_MIN = 60
const SIZE_MAX = 184
const SIZE_DEFAULT = 96

/* ------------------------------------------------------------------ */
/* enable / disable                                                    */
/* ------------------------------------------------------------------ */

export function petEnabled(): boolean {
  try {
    return localStorage.getItem(PET_KEY) !== 'off'
  } catch {
    return true
  }
}

export function setPetEnabled(on: boolean) {
  try {
    localStorage.setItem(PET_KEY, on ? 'on' : 'off')
  } catch {
    /* private mode — state still flips for this session via the event */
  }
  window.dispatchEvent(new CustomEvent(PET_EVENT, { detail: on }))
}

export function usePetEnabled(): [boolean, (on: boolean) => void] {
  const [on, setOn] = useState<boolean>(petEnabled)
  useEffect(() => {
    const handler = (e: Event) => setOn((e as CustomEvent<boolean>).detail)
    const storage = (e: StorageEvent) => {
      if (e.key === PET_KEY) setOn(e.newValue !== 'off')
    }
    window.addEventListener(PET_EVENT, handler)
    window.addEventListener('storage', storage)
    return () => {
      window.removeEventListener(PET_EVENT, handler)
      window.removeEventListener('storage', storage)
    }
  }, [])
  return [on, setPetEnabled]
}

/** Sidebar switch — the way back in after Clause has been hidden. */
export function ClausePetToggle() {
  const [on, setOn] = usePetEnabled()
  return (
    <div className="flex items-center gap-2.5 px-1 py-1">
      <img src="/clause-pet.png?v=2" alt="" className="w-6 h-7 object-contain" draggable={false} />
      <span className="text-[13px] text-stone-600 flex-1">
        Clause pet
        <span className="block text-[11px] text-stone-400">
          {on ? 'On — drag him anywhere, tap for tips' : 'Companion is hidden'}
        </span>
      </span>
      <button
        role="switch"
        aria-checked={on}
        aria-label="Toggle Clause pet"
        onClick={() => setOn(!on)}
        className={`relative w-9 h-5 rounded-full transition-colors shrink-0 ${on ? 'bg-ink' : 'bg-stone-300'}`}
      >
        <span
          className={`absolute top-0.5 h-4 w-4 rounded-full bg-ivory shadow transition-all ${on ? 'left-[18px]' : 'left-0.5'}`}
        />
      </button>
    </div>
  )
}

/* ------------------------------------------------------------------ */
/* page knowledge                                                      */
/* ------------------------------------------------------------------ */

type Mood = 'content' | 'happy' | 'thinking' | 'curious' | 'sleepy'

type PetAction =
  | { label: string; kind: 'go'; view: string }
  | { label: string; kind: 'ask'; question: string }
  | { label: string; kind: 'new' }

interface PageInfo {
  name: string
  mood: Mood
  greet: string
  tips: string[]
  actions: PetAction[]
}

const PAGES: Record<string, PageInfo> = {
  dashboard: {
    name: 'Dashboard',
    mood: 'happy',
    greet: "Welcome back — this is your workspace at a glance.",
    tips: [
      'Your plan, tokens left and renewal date live right here — no surprises later.',
      '“Pick up where you left off” reopens any past answer in one click.',
      'The brief you pick in the sidebar scopes everything I do. Choose wisely.',
    ],
    actions: [
      { label: 'Ask Clause something', kind: 'new' },
      { label: 'See plans & billing', kind: 'go', view: 'plans' },
    ],
  },
  matters: {
    name: 'Briefs',
    mood: 'content',
    greet: 'Briefs are living matters — each one remembers its own documents.',
    tips: [
      'A brief is a hard confidentiality boundary: scoped questions never leak across briefs.',
      'Uploads, Interact and Draft all file into the brief you are working in.',
    ],
    actions: [
      { label: 'Ask within a brief', kind: 'go', view: 'assistant' },
      { label: 'Open the Vault', kind: 'go', view: 'vault' },
    ],
  },
  matter: {
    name: 'Brief',
    mood: 'content',
    greet: 'Inside a brief, everything I retrieve stays inside it. Promise.',
    tips: [
      'Ask from here and I only cite this brief’s documents — nothing outside it.',
      'Add the other side’s filings here and I’ll contrast them for you.',
    ],
    actions: [{ label: 'Ask about this brief', kind: 'go', view: 'assistant' }],
  },
  assistant: {
    name: 'Ask',
    mood: 'content',
    greet: 'Ask against your whole vault — every claim I make carries a citation.',
    tips: [
      'Turn on Web and I blend live sources — each gets an authority card with its date.',
      'See the receipt under my answers: “citation integrity” — unsupported claims are removed before you read them.',
      'पूछिए हिंदी में — I’ll answer in Hindi with the same citations.',
      'Verdict first, then the analysis. No throat-clearing.',
    ],
    actions: [
      { label: 'Try: Section 86 of the IT Act', kind: 'ask', question: 'What does Section 86 of the IT Act deal with?' },
      { label: 'Go to Research mode', kind: 'go', view: 'research' },
    ],
  },
  interact: {
    name: 'Interact',
    mood: 'curious',
    greet: 'Pick a document and interrogate it — summary, chronology, clauses, risks.',
    tips: [
      'The chronology view pulls every date out of a judgment into a timeline.',
      '“Risks” flags the clauses that usually bite — limitation, indemnity, notice periods.',
    ],
    actions: [{ label: 'Open the Vault', kind: 'go', view: 'vault' }],
  },
  draft: {
    name: 'Draft',
    mood: 'thinking',
    greet: 'Drafts here are grounded in your vault — notices, agreements, IRAC memos, petitions.',
    tips: [
      'An IRAC memo from Draft is structured Issue–Rule–Analysis–Conclusion, citations included.',
      'Draft inside a brief and I only lean on that brief’s documents.',
    ],
    actions: [
      { label: 'Ask a question first', kind: 'go', view: 'assistant' },
      { label: 'Run a workflow', kind: 'go', view: 'agents' },
    ],
  },
  research: {
    name: 'Research',
    mood: 'thinking',
    greet: 'Research mode: I frame the claims, hunt evidence for and against, then adjudicate.',
    tips: [
      'Each hypothesis is tested separately — the verdict tells you which ones survived.',
      'Web evidence is blended in with authority tiers: courts and gazettes outrank blogs.',
    ],
    actions: [{ label: 'Back to Ask', kind: 'go', view: 'assistant' }],
  },
  agents: {
    name: 'Agents',
    mood: 'happy',
    greet: 'Fixed, auditable workflows: contract review, due diligence, IRAC memo drafting.',
    tips: [
      'Due diligence includes a web leg — counterparty news and filings, not just your vault.',
      'Every workflow step shows its sources, so you can audit how a conclusion was reached.',
    ],
    actions: [{ label: 'Draft a document', kind: 'go', view: 'draft' }],
  },
  vault: {
    name: 'Vault',
    mood: 'curious',
    greet: 'Your firm’s memory — everything I can cite lives here.',
    tips: [
      'Scanned Hindi judgments? My OCR reads Devanagari — drop the PDF in.',
      'Parent–child chunking means I match the exact clause but answer with the whole section.',
    ],
    actions: [
      { label: 'Ask the vault', kind: 'go', view: 'assistant' },
      { label: 'Search documents', kind: 'go', view: 'search' },
    ],
  },
  search: {
    name: 'Search',
    mood: 'curious',
    greet: 'Full-text search across every document — handy when you half-remember a phrase.',
    tips: ['Found the doc? Open it in Interact and I’ll summarise it on the spot.'],
    actions: [{ label: 'Open Interact', kind: 'go', view: 'interact' }],
  },
  private: {
    name: 'Private Vault',
    mood: 'sleepy',
    greet: 'Shh — Private Vault. Encrypted at rest; only you hold the key.',
    tips: [
      'Private documents never enter the shared corpus, the graph, or anyone else’s answers.',
      'Lock the vault and even I can’t peek — a locked vault is an explicit “no”.',
    ],
    actions: [{ label: 'Back to the Vault', kind: 'go', view: 'vault' }],
  },
  graph: {
    name: 'Knowledge',
    mood: 'thinking',
    greet: 'The knowledge graph — parties, provisions and precedents, wired together.',
    tips: ['Entities extracted from your documents appear here; Ask uses these links to widen retrieval.'],
    actions: [{ label: 'Ask with the graph', kind: 'go', view: 'assistant' }],
  },
  plans: {
    name: 'Plans & Billing',
    mood: 'happy',
    greet: 'Plans & billing — tokens, usage and invoices, all transparent.',
    tips: [
      'Cached answers bill zero tokens. Ask the same thing twice, pay once — actually, not even once.',
      'Basic is ₹2,999 for 2M tokens; Premium ₹9,999 for 10M.',
    ],
    actions: [{ label: 'Back to Dashboard', kind: 'go', view: 'dashboard' }],
  },
  tab: {
    name: 'Page',
    mood: 'content',
    greet: 'A custom page your admin added. I’m still here if you need me.',
    tips: ['Tap me anytime for Ask, Research and Draft shortcuts.'],
    actions: [{ label: 'Ask Clause', kind: 'new' }],
  },
}

/* ------------------------------------------------------------------ */
/* face                                                                */
/* ------------------------------------------------------------------ */

function Face({ mood, blinking }: { mood: Mood; blinking: boolean }) {
  const ink = '#2a2723'
  const eyeProps = { fill: ink }
  let eyes: React.ReactNode
  let mouth: React.ReactNode

  if (mood === 'happy') {
    eyes = (
      <>
        <path d="M22 36 Q30 24 38 36" fill="none" stroke={ink} strokeWidth="4.6" strokeLinecap="round" />
        <path d="M62 36 Q70 24 78 36" fill="none" stroke={ink} strokeWidth="4.6" strokeLinecap="round" />
      </>
    )
  } else if (mood === 'sleepy') {
    eyes = (
      <>
        <path d="M22 33 L38 33" stroke={ink} strokeWidth="4.6" strokeLinecap="round" />
        <path d="M62 33 L78 33" stroke={ink} strokeWidth="4.6" strokeLinecap="round" />
      </>
    )
  } else if (blinking) {
    eyes = (
      <>
        <path d="M22 32 L38 32" stroke={ink} strokeWidth="4.4" strokeLinecap="round" />
        <path d="M62 32 L78 32" stroke={ink} strokeWidth="4.4" strokeLinecap="round" />
      </>
    )
  } else if (mood === 'thinking') {
    eyes = (
      <>
        <circle cx="33" cy="27" r="5.2" {...eyeProps} />
        <circle cx="73" cy="27" r="5.2" {...eyeProps} />
      </>
    )
  } else if (mood === 'curious') {
    eyes = (
      <>
        <circle cx="30" cy="31" r="6" {...eyeProps} />
        <circle cx="71" cy="31" r="4.2" {...eyeProps} />
      </>
    )
  } else {
    eyes = (
      <>
        <circle cx="30" cy="31" r="5.2" {...eyeProps} />
        <circle cx="70" cy="31" r="5.2" {...eyeProps} />
      </>
    )
  }

  if (mood === 'happy') {
    mouth = <path d="M35 45 Q50 60 65 45" fill="none" stroke={ink} strokeWidth="4.4" strokeLinecap="round" />
  } else if (mood === 'thinking') {
    mouth = <path d="M41 51 Q50 47 59 51" fill="none" stroke={ink} strokeWidth="4.2" strokeLinecap="round" />
  } else if (mood === 'sleepy') {
    mouth = <circle cx="50" cy="50" r="3.4" fill="none" stroke={ink} strokeWidth="3.8" />
  } else if (mood === 'curious') {
    mouth = <path d="M40 46 Q51 53 61 45" fill="none" stroke={ink} strokeWidth="4.2" strokeLinecap="round" />
  } else {
    mouth = <path d="M38 45 Q50 54 62 45" fill="none" stroke={ink} strokeWidth="4.4" strokeLinecap="round" />
  }

  return (
    <svg
      viewBox="0 0 100 64"
      className="absolute pointer-events-none"
      style={{
        left: '27.5%',
        top: '16.5%',
        width: '45%',
        height: '26%',
        transform: mood === 'curious' ? 'rotate(-3.5deg)' : undefined,
      }}
      aria-hidden
    >
      {eyes}
      {mouth}
    </svg>
  )
}

/* ------------------------------------------------------------------ */
/* pet                                                                 */
/* ------------------------------------------------------------------ */

interface ClausePetProps {
  view: string
  go: (view: string) => void
  askQuestion: (q: string) => void
  startNewChat: () => void
}

function loadSize(): number {
  try {
    const n = Number(localStorage.getItem(SIZE_KEY))
    return n >= SIZE_MIN && n <= SIZE_MAX ? n : SIZE_DEFAULT
  } catch {
    return SIZE_DEFAULT
  }
}

function loadPos(): { x: number; y: number } | null {
  try {
    const raw = localStorage.getItem(POS_KEY)
    if (!raw) return null
    const p = JSON.parse(raw) as { x: number; y: number }
    return typeof p.x === 'number' && typeof p.y === 'number' ? p : null
  } catch {
    return null
  }
}

export default function ClausePet({ view, go, askQuestion, startNewChat }: ClausePetProps) {
  const [on, setOn] = usePetEnabled()
  const toast = useSession((s) => s.toast)
  const [size, setSize] = useState<number>(loadSize)
  const [pos, setPos] = useState<{ x: number; y: number } | null>(loadPos)
  const [menuOpen, setMenuOpen] = useState(false)
  const [bubbleText, setBubbleText] = useState<string | null>(null)
  const [hovering, setHovering] = useState(false)
  const [dragging, setDragging] = useState(false)
  const [happyUntil, setHappyUntil] = useState(0)
  const [lastInteract, setLastInteract] = useState(Date.now())
  const [blinking, setBlinking] = useState(false)
  const [waving, setWaving] = useState(false)

  const dragRef = useRef<{ startX: number; startY: number; baseX: number; baseY: number; moved: boolean } | null>(null)
  const justDraggedRef = useRef(0)
  const rootRef = useRef<HTMLDivElement>(null)

  const page = PAGES[view] ?? PAGES.tab

  /* greeting + tip cycle per page */
  useEffect(() => {
    if (!on) return
    setBubbleText(page.greet)
    setMenuOpen(false)
    const hide = setTimeout(() => setBubbleText(null), 7500)
    let i = 0
    const cycle = setInterval(() => {
      setBubbleText(page.tips[i % page.tips.length])
      i += 1
      setTimeout(() => setBubbleText(null), 6500)
    }, 30000)
    return () => {
      clearTimeout(hide)
      clearInterval(cycle)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view, on])

  /* blink loop */
  useEffect(() => {
    let t1: ReturnType<typeof setTimeout>
    let t2: ReturnType<typeof setTimeout>
    const loop = () => {
      t1 = setTimeout(() => {
        setBlinking(true)
        t2 = setTimeout(() => {
          setBlinking(false)
          loop()
        }, 150)
      }, 2600 + Math.random() * 2600)
    }
    loop()
    return () => {
      clearTimeout(t1)
      clearTimeout(t2)
    }
  }, [])

  const interact = useCallback(() => setLastInteract(Date.now()), [])

  const idleSleepy = Date.now() - lastInteract > 50_000 && !menuOpen
  const mood: Mood =
    Date.now() < happyUntil
      ? 'happy'
      : dragging
        ? 'curious'
        : idleSleepy
          ? 'sleepy'
          : hovering
            ? 'curious'
            : page.mood

  const wave = useCallback(() => {
    setWaving(true)
    setTimeout(() => setWaving(false), 900)
  }, [])

  const hide = useCallback(() => {
    setOn(false)
    toast('info', 'Clause pet hidden — turn him back on in the sidebar, under Clause pet.')
  }, [setOn, toast])

  const setSizeValue = (value: number) => {
    const next = Math.min(SIZE_MAX, Math.max(SIZE_MIN, Math.round(value)))
    setSize(next)
    try {
      localStorage.setItem(SIZE_KEY, String(next))
    } catch { /* ignore */ }
    interact()
  }

  const resetPos = () => {
    setPos(null)
    try {
      localStorage.removeItem(POS_KEY)
    } catch { /* ignore */ }
    interact()
  }

  /* ---- drag (pointer events; click = tap without moving) ---- */
  const onPointerDown = (e: React.PointerEvent) => {
    const el = rootRef.current
    if (!el) return
    const rect = el.getBoundingClientRect()
    dragRef.current = {
      startX: e.clientX,
      startY: e.clientY,
      baseX: rect.left,
      baseY: rect.top,
      moved: false,
    }
    ;(e.target as HTMLElement).setPointerCapture?.(e.pointerId)
    interact()
  }

  const onPointerMove = (e: React.PointerEvent) => {
    const d = dragRef.current
    if (!d) return
    const dx = e.clientX - d.startX
    const dy = e.clientY - d.startY
    if (!d.moved && Math.hypot(dx, dy) < 5) return
    if (!d.moved) {
      d.moved = true
      setDragging(true)
      setMenuOpen(false)
      setBubbleText(null)
    }
    const w = rootRef.current?.offsetWidth ?? size
    const h = rootRef.current?.offsetHeight ?? size * 1.3
    const x = Math.min(Math.max(4, d.baseX + dx), window.innerWidth - w - 4)
    const y = Math.min(Math.max(4, d.baseY + dy), window.innerHeight - h - 4)
    setPos({ x, y })
  }

  const onPointerUp = () => {
    const d = dragRef.current
    dragRef.current = null
    if (d?.moved) {
      setDragging(false)
      justDraggedRef.current = Date.now()
      setPos((p) => {
        if (p) {
          try {
            localStorage.setItem(POS_KEY, JSON.stringify(p))
          } catch { /* ignore */ }
        }
        return p
      })
    }
    interact()
  }

  const onPetClick = () => {
    if (Date.now() - justDraggedRef.current < 250) return // it was a drag, not a tap
    interact()
    setHappyUntil(Date.now() + 1600)
    wave()
    setMenuOpen((m) => !m)
  }

  if (!on) return null

  const bubbleShown = menuOpen || bubbleText !== null

  const runAction = (a: PetAction) => {
    setMenuOpen(false)
    setBubbleText(null)
    interact()
    if (a.kind === 'go') go(a.view)
    else if (a.kind === 'ask') askQuestion(a.question)
    else startNewChat()
  }

  return (
    <div
      ref={rootRef}
      className="fixed z-40 flex flex-col items-end select-none"
      style={
        pos
          ? { left: pos.x, top: pos.y }
          : { right: 20, bottom: 20 }
      }
    >
      <AnimatePresence>
        {bubbleShown && (
          <motion.div
            initial={{ opacity: 0, y: 6, scale: 0.96 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 4, scale: 0.97 }}
            transition={{ duration: 0.18 }}
            className="mb-1 mr-1 w-[268px] rounded-lex-lg border border-stone-200 bg-white shadow-lex-md px-3.5 py-2.5 relative"
          >
            <p className="text-[10px] font-semibold uppercase tracking-wider text-stone-400">
              Clause · {page.name}
            </p>
            <p className="text-[12.5px] leading-snug text-stone-600 mt-0.5">
              {menuOpen ? 'At your service — here’s what I can do on this page:' : bubbleText}
            </p>
            {menuOpen && (
              <div className="mt-2 flex flex-col gap-1">
                <div className="px-1 pt-0.5 pb-2 mb-1.5 border-b border-stone-100">
                  <div className="flex items-center">
                    <span className="text-[11px] uppercase tracking-wider text-stone-400 font-medium">Size</span>
                    <span className="ml-auto text-[11px] text-stone-400 tabular-nums">{size}px</span>
                    {pos && (
                      <button
                        onClick={resetPos}
                        title="Send Clause back to his corner"
                        aria-label="Reset Clause position"
                        className="ml-1.5 p-0.5 rounded text-stone-300 hover:text-ink transition-colors"
                      >
                        <RotateCcw size={11} />
                      </button>
                    )}
                  </div>
                  <input
                    type="range"
                    min={SIZE_MIN}
                    max={SIZE_MAX}
                    step={2}
                    value={size}
                    onChange={(e) => setSizeValue(Number(e.target.value))}
                    aria-label="Clause pet size"
                    className="pet-size-slider w-full mt-2"
                    style={{
                      background: `linear-gradient(90deg, #0f0e0d ${((size - SIZE_MIN) / (SIZE_MAX - SIZE_MIN)) * 100}%, #e7e5e0 ${((size - SIZE_MIN) / (SIZE_MAX - SIZE_MIN)) * 100}%)`,
                    }}
                  />
                </div>
                {page.actions.map((a) => (
                  <button
                    key={a.label}
                    onClick={() => runAction(a)}
                    className="flex items-center gap-2 rounded-lex bg-ink text-ivory px-2.5 py-1.5 text-[12.5px] font-medium hover:bg-ink-900 transition-colors text-left"
                  >
                    <MessageSquare size={13} className="shrink-0" /> {a.label}
                  </button>
                ))}
                <button
                  onClick={hide}
                  className="flex items-center gap-2 rounded-lex px-2.5 py-1.5 text-[12.5px] text-stone-500 hover:bg-stone-100 hover:text-ink transition-colors"
                >
                  <EyeOff size={13} /> Hide pet
                </button>
              </div>
            )}
            <span className="absolute -bottom-[5px] right-8 w-2.5 h-2.5 rotate-45 bg-white border-b border-r border-stone-200" />
          </motion.div>
        )}
      </AnimatePresence>

      <div
        className="relative"
        onMouseEnter={() => {
          setHovering(true)
          interact()
        }}
        onMouseLeave={() => setHovering(false)}
      >
        <button
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onClick={onPetClick}
          title="Clause — drag me anywhere, tap for page tips"
          aria-label="Clause, your research companion. Drag to move, tap for options."
          className={`relative block outline-none touch-none ${dragging ? 'cursor-grabbing' : 'cursor-grab'}`}
          style={{ width: size }}
        >
          <span className="pet-bob block relative">
            <motion.img
              src="/clause-pet.png?v=2"
              alt="Clause, a small glass crystal companion"
              draggable={false}
              animate={waving ? { rotate: [0, -7, 6, -4, 0], y: [0, -10, 0, -5, 0] } : { rotate: 0, y: 0 }}
              transition={{ duration: 0.85, ease: 'easeInOut' }}
              className="w-full h-auto drop-shadow-[0_10px_16px_rgba(15,14,13,0.18)] pointer-events-none"
            />
            <Face mood={mood} blinking={blinking} />
            {mood === 'sleepy' && (
              <>
                <span className="pet-zzz absolute text-[#7aa5c4] font-display pointer-events-none" style={{ right: '6%', top: '2%', fontSize: size * 0.16 }}>z</span>
                <span className="pet-zzz absolute text-[#7aa5c4] font-display pointer-events-none" style={{ right: '-2%', top: '-6%', fontSize: size * 0.22, animationDelay: '0.9s' }}>Z</span>
              </>
            )}
            <span className="pet-sparkle absolute top-1 -left-1 text-[#b7791f] pointer-events-none" style={{ fontSize: size * 0.14 }}>✦</span>
            <span className="pet-sparkle absolute text-[#7aa5c4] pointer-events-none" style={{ top: '26%', right: -4, fontSize: size * 0.1, animationDelay: '1.2s' }}>✦</span>
          </span>
        </button>
        <span className="pet-shadow block mx-auto mt-0.5 h-[7px] rounded-full bg-ink pointer-events-none" style={{ width: size * 0.56 }} />
        <span className="absolute -bottom-2 left-1/2 -translate-x-1/2 flex items-center gap-1 rounded-full bg-ink text-ivory pl-1.5 pr-2 py-[2px] shadow-lex-sm pointer-events-none">
          <span className={`w-1.5 h-1.5 rounded-full ${mood === 'sleepy' ? 'bg-stone-400' : 'bg-emerald-400'}`} />
          <span className="text-[10px] font-semibold tracking-wide">Clause</span>
        </span>
      </div>
    </div>
  )
}
