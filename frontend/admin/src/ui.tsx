export function StatCard({ label, value, sub }: { label: string; value: string | number; sub?: string }) {
  return (
    <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-5 py-4">
      <p className="text-meta uppercase tracking-wider text-stone-500 font-medium">{label}</p>
      <p className="font-display text-3xl mt-1.5 tracking-tight">{value}</p>
      {sub && <p className="text-meta text-stone-500 mt-1">{sub}</p>}
    </div>
  )
}

export function Spinner({ label }: { label?: string }) {
  return (
    <div className="flex items-center gap-3 text-stone-500 py-8 justify-center">
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
      <p className="text-stone-500 mt-1.5 text-[15px]">{sub}</p>
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
              className="fill-[#0f0e0d]"
              opacity={d.value === 0 ? 0.12 : 0.88}
            >
              <title>{d.label}: {d.value}</title>
            </rect>
          )
        })}
      </svg>
      <div className="flex justify-between text-[10.5px] text-stone-400 mt-1">
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
        <text x="21" y="20" textAnchor="middle" className="fill-[#0f0e0d]" fontSize="8" fontWeight="600">
          {data.reduce((s, d) => s + d.value, 0)}
        </text>
        <text x="21" y="26.5" textAnchor="middle" className="fill-[#78716c]" fontSize="3.6">users</text>
      </svg>
      <ul className="space-y-1.5 text-[13px]">
        {data.map((d, i) => (
          <li key={d.label} className="flex items-center gap-2">
            <span className="w-2.5 h-2.5 rounded-sm inline-block" style={{ background: DONUT_COLORS[i % DONUT_COLORS.length] }} />
            {d.label} <span className="text-stone-400">· {d.value}</span>
          </li>
        ))}
      </ul>
    </div>
  )
}
