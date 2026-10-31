import { useState } from 'react'
import { CreditCard, Database, LayoutDashboard, Moon, Star, Sun, Users } from 'lucide-react'

/**
 * Piece 39: the brand mark, ported from the user app — the scales
 * of justice centred in a bronze medallion ring (transparent fill).
 * Bronze flips with the theme via --bronze in index.css.
 */
export function BrandMark({ size = 36 }: { size?: number }) {
  return (
    <span
      aria-hidden="true"
      className="inline-flex bg-white rounded-lg p-0.5 shadow-sm shadow-black/10 shrink-0"
    >
      <img
        src="/admin/logo.png"
        alt=""
        draggable={false}
        className="object-contain rounded-[5px]"
        style={{ width: size, height: size }}
      />
    </span>
  )
}

/**
 * Piece 39: dark-mode toggle, ported from the user app. The initial
 * class is set pre-paint by the inline script in index.html (stored
 * `jurisvault.theme` — shared with the user app on this origin —
 * else the system preference); this button flips it and persists
 * the choice.
 */
export function ThemeToggle({ className = '' }: { className?: string }) {
  const [dark, setDark] = useState(
    () => typeof document !== 'undefined' && document.documentElement.classList.contains('dark'),
  )
  function toggle() {
    const next = !dark
    setDark(next)
    document.documentElement.classList.toggle('dark', next)
    try {
      localStorage.setItem('jurisvault.theme', next ? 'dark' : 'light')
    } catch { /* storage unavailable — theme lasts for the session */ }
  }
  return (
    <button
      onClick={toggle}
      title={dark ? 'Switch to light mode' : 'Switch to dark mode'}
      aria-label="Toggle dark mode"
      className={`p-2 rounded-lex text-stone-500 dark:text-stone-400 hover:bg-stone-100 dark:hover:bg-white/10 hover:text-ink transition-colors ${className}`}
    >
      {dark ? <Sun size={16} /> : <Moon size={16} />}
    </button>
  )
}

/**
 * Piece 39: the moving glow layer, ported from the user app — the
 * taste-skill page's hero-glow recipe (index.css: background
 * var(--grad-glow)) drifting slowly over the page wash on the 26s
 * dualDrift loop, stilled by reduced motion. Purely decorative.
 * Default is absolutely positioned (parent must be relative +
 * overflow-hidden); `fixed` pins it to the viewport, as in the
 * shell where one instance glows behind the whole app.
 */
export function DualToneBackdrop({ fixed = false }: { fixed?: boolean }) {
  return (
    <div
      aria-hidden="true"
      className={`dual-tone-backdrop${fixed ? ' dual-tone-backdrop-fixed' : ''}`}
    >
      {/* Piece 44: the rotating conic sheen (CSS in index.css). */}
      <div className="backdrop-spin" aria-hidden />
    </div>
  )
}

/**
 * Piece 42: the jurisvault.ai auth scene, ported from the user app —
 * the brand site's running sign-in background (deep-emerald base,
 * 1px grid, a pulsing core glow, twinkling spark dots; recipes in
 * index.css). Piece 44: the conic rings became the site's orbit
 * system with floating labelled nodes. Rendered only by the
 * admin sign-in (AuthGate), whose root carries `dark auth-scene`,
 * so the scene is always dark whatever the app theme. Purely
 * decorative: every layer is pointer-events-none + aria-hidden.
 * Spark tuples: [left %, top %, twinkle delay s, size px].
 */
const AUTH_SPARKS: ReadonlyArray<readonly [number, number, number, number]> = [
  [8, 18, 0, 3], [16, 72, 0.6, 2], [24, 34, 1.4, 3], [33, 86, 0.3, 2],
  [42, 12, 2.1, 2], [55, 64, 1.0, 3], [63, 26, 1.8, 2], [71, 80, 0.9, 3],
  [78, 44, 2.6, 2], [86, 16, 0.2, 3], [91, 58, 1.2, 2], [95, 88, 2.2, 2],
  [48, 44, 3.0, 2], [5, 52, 1.6, 2],
]

/* Piece 44: floating auth nodes at the site's positions/delays,
   labelled for the Command Center (icons in the site's node style). */
const AUTH_NODES = [
  { label: 'Overview', left: '60%', top: '9%', delay: '0s', Icon: LayoutDashboard },
  { label: 'Users', left: '13%', top: '29%', delay: '1.1s', Icon: Users },
  { label: 'Corpus', left: '87%', top: '35%', delay: '2.2s', Icon: Database },
  { label: 'Billing', left: '8%', top: '62%', delay: '3.3s', Icon: CreditCard },
  { label: 'Golden Rules', left: '77%', top: '66%', delay: '4.4s', Icon: Star },
] as const

export function AuthScene() {
  return (
    <>
      <div aria-hidden="true" className="auth-scene-base" />
      <div aria-hidden="true" className="auth-grid" />
      {/* Piece 44: the site's orbit system (dashed outer orbit 46s,
          plain inner orbit 68s reverse) replaces the conic rings. */}
      <div className="auth-orbit-wrap" aria-hidden>
        <div className="auth-orbit-box">
          <div className="auth-orbit auth-orbit-dashed" />
          <div className="auth-orbit auth-orbit-inner" />
        </div>
      </div>
      {/* Piece 44: floating labelled nodes, large screens only. */}
      <div aria-hidden="true">
        {AUTH_NODES.map(({ label, left, top, delay, Icon }) => (
          <div
            key={label}
            className="auth-node hidden lg:flex"
            style={{ left, top, animationDelay: delay }}
          >
            <span className="auth-node-dot">
              <Icon size={22} strokeWidth={1.6} />
            </span>
            <span className="auth-node-label">{label}</span>
          </div>
        ))}
      </div>
      <div aria-hidden="true" className="auth-core" />
      {AUTH_SPARKS.map(([left, top, delay, size], i) => (
        <span
          key={i}
          aria-hidden="true"
          className="auth-spark"
          style={{
            left: `${left}%`,
            top: `${top}%`,
            width: size,
            height: size,
            animationDelay: `${delay}s`,
          }}
        />
      ))}
    </>
  )
}

export function StatCard({ label, value, sub }: { label: string; value: string | number; sub?: string }) {
  return (
    <div className="rounded-lex-lg border border-stone-200 dark:border-white/10 bg-white dark:bg-[#0a1a12] px-5 py-4">
      <p className="text-meta text-stone-500 dark:text-stone-400 font-medium">{label}</p>
      <p className="font-display text-3xl mt-1.5 tracking-tight">{value}</p>
      {sub && <p className="text-meta text-stone-500 dark:text-stone-400 mt-1">{sub}</p>}
    </div>
  )
}

export function Spinner({ label }: { label?: string }) {
  return (
    <div className="flex items-center gap-3 text-stone-500 dark:text-stone-400 py-8 justify-center">
      <span className="flex gap-1">
        <span className="dot-pulse w-1.5 h-1.5 rounded-full bg-stone-500 inline-block" />
        <span className="dot-pulse w-1.5 h-1.5 rounded-full bg-stone-500 inline-block" />
        <span className="dot-pulse w-1.5 h-1.5 rounded-full bg-stone-500 inline-block" />
      </span>
      {label && <span className="text-[13px]">{label}</span>}
    </div>
  )
}

export function ErrorBanner({ message }: { message: string }) {
  return (
    <div className="rounded-lex border border-brand-red/30 bg-[#feeaea] px-4 py-3 text-[13px] text-[#a02c2c]">{message}</div>
  )
}

export function PageHead({ title, sub }: { title: string; sub: string }) {
  return (
    <>
      <h1 className="font-display text-[2rem] tracking-tight">{title}</h1>
      <p className="text-stone-500 dark:text-stone-400 mt-1.5 text-[15px]">{sub}</p>
    </>
  )
}

/** Vertical bar chart (pure SVG, design-system colours). */
export function BarChart({ data, height = 120 }: { data: { label: string; value: number }[]; height?: number }) {
  const max = Math.max(1, ...data.map((d) => d.value))
  const w = 100 / Math.max(1, data.length)
  return (
    <div>
      <svg viewBox={`0 0 100 ${height / 4}`} className="w-full" style={{ height }} preserveAspectRatio="none">
        {data.map((d, i) => {
          const h = Math.max(1.5, (d.value / max) * ((height / 4) - 4))
          return (
            <rect
              key={i}
              x={i * w + w * 0.18}
              y={(height / 4) - h}
              width={w * 0.64}
              height={h}
              rx={0.8}
              className="fill-[#0f0e0d] dark:fill-[#f4f2ee]"
              opacity={d.value === 0 ? 0.12 : 0.88}
            >
              <title>{d.label}: {d.value}</title>
            </rect>
          )
        })}
      </svg>
      <div className="flex justify-between text-[10.5px] text-stone-400 dark:text-stone-500 mt-1">
        <span>{data[0]?.label}</span>
        <span>{data[data.length - 1]?.label}</span>
      </div>
    </div>
  )
}

const DONUT_COLORS = ['#0f0e0d', '#a8a29e', '#d6d3d1', '#78716c']

/** Donut chart with legend (pure SVG). */
export function Donut({ data }: { data: { label: string; value: number }[] }) {
  const total = Math.max(1, data.reduce((s, d) => s + d.value, 0))
  let acc = 0
  const R = 15.9155 // circumference = 100
  return (
    <div className="flex items-center gap-5">
      <svg viewBox="0 0 42 42" className="w-28 h-28 shrink-0">
        <circle cx="21" cy="21" r={R} fill="none" stroke="#f5f5f4" strokeWidth="5" />
        {data.map((d, i) => {
          const frac = d.value / total
          const dash = `${frac * 100} ${100 - frac * 100}`
          const offset = 25 - acc * 100
          acc += frac
          return (
            <circle
              key={d.label}
              cx="21" cy="21" r={R} fill="none"
              stroke={DONUT_COLORS[i % DONUT_COLORS.length]}
              strokeWidth="5"
              strokeDasharray={dash}
              strokeDashoffset={offset}
            >
              <title>{d.label}: {d.value}</title>
            </circle>
          )
        })}
        <text x="21" y="20" textAnchor="middle" className="fill-[#0f0e0d] dark:fill-[#f4f2ee]" fontSize="8" fontWeight="600">
          {data.reduce((s, d) => s + d.value, 0)}
        </text>
        <text x="21" y="26.5" textAnchor="middle" className="fill-[#78716c] dark:fill-[#a8a29e]" fontSize="3.6">users</text>
      </svg>
      <ul className="space-y-1.5 text-[13px]">
        {data.map((d, i) => (
          <li key={d.label} className="flex items-center gap-2">
            <span className="w-2.5 h-2.5 rounded-sm inline-block" style={{ background: DONUT_COLORS[i % DONUT_COLORS.length] }} />
            {d.label} <span className="text-stone-400 dark:text-stone-500">· {d.value}</span>
          </li>
        ))}
      </ul>
    </div>
  )
}
