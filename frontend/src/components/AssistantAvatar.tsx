/**
 * Clause — the Juris Vault AI assistant's avatar. A living mark: an ink
 * orb carrying the scales of justice, a slow-spinning dashed orbit, and
 * an equalizer badge while "playing" (thinking/speaking). Pure SVG + CSS
 * so it stays crisp at any size.
 */
export default function AssistantAvatar({
  size = 64,
  playing = false,
  float = true,
}: {
  size?: number
  playing?: boolean
  float?: boolean
}) {
  return (
    <div
      className={`relative shrink-0 ${float ? 'avatar-float' : ''}`}
      style={{ width: size, height: size }}
      aria-label="Clause, your legal research companion"
      role="img"
    >
      {/* pulse ring */}
      <span className="avatar-ring absolute inset-0 rounded-full border-2 border-[#b7791f]/50" />
      {/* orb */}
      <span className="absolute inset-0 rounded-full bg-gradient-to-br from-[#23201c] via-ink to-[#3a352d] shadow-lex-md flex items-center justify-center overflow-hidden">
        <svg viewBox="0 0 48 48" width={size * 0.62} height={size * 0.62} fill="none" stroke="#fafaf9" strokeWidth="2" strokeLinecap="round">
          {/* beam */}
          <line x1="10" y1="13" x2="38" y2="13" />
          <circle cx="24" cy="13" r="1.6" fill="#b7791f" stroke="none" />
          {/* pillar + base */}
          <line x1="24" y1="13" x2="24" y2="34" />
          <line x1="17" y1="38" x2="31" y2="38" />
          <line x1="24" y1="34" x2="24" y2="38" />
          {/* left pan */}
          <line x1="10" y1="13" x2="5.5" y2="22" />
          <line x1="10" y1="13" x2="14.5" y2="22" />
          <path d="M3.5 22 a6.5 3.4 0 0 0 13 0" />
          {/* right pan */}
          <line x1="38" y1="13" x2="33.5" y2="22" />
          <line x1="38" y1="13" x2="42.5" y2="22" />
          <path d="M31.5 22 a6.5 3.4 0 0 0 13 0" />
        </svg>
        {/* orbit */}
        <svg viewBox="0 0 100 100" className="avatar-spin-slow absolute inset-0 w-full h-full">
          <circle cx="50" cy="50" r="46" fill="none" stroke="#b7791f" strokeOpacity="0.55" strokeWidth="1.6" strokeDasharray="4 7" />
        </svg>
      </span>
      {/* status / playing badge */}
      {playing ? (
        <span className="absolute -bottom-1 -right-1 h-[26%] min-h-[15px] px-[3px] rounded-full bg-[#b7791f] border-2 border-ivory flex items-end gap-[2px] py-[3px]">
          <span className="eq-bar w-[3px] h-[9px] bg-ivory rounded-full" />
          <span className="eq-bar w-[3px] h-[9px] bg-ivory rounded-full" />
          <span className="eq-bar w-[3px] h-[9px] bg-ivory rounded-full" />
        </span>
      ) : (
        <span className="absolute bottom-0 right-0 w-[24%] h-[24%] min-w-[11px] min-h-[11px] rounded-full bg-emerald-500 border-2 border-ivory" />
      )}
    </div>
  )
}
