# Juris Vault AI Frontend — Design System

**Taste (achromatic ledger) over the Harvey-derived palette.**
Piece 34 (31 Oct 2026) restyled the user app with the design-taste-frontend
skill (tasteskill v2, app-screen run): direction *achromatic-ledger*,
seed 1210974860, kind workspace, light — with the brand palette and type
kept from the Harvey REA below. The admin Command Center keeps its own
look and was not part of this pass.

## The move

- **No colour in the chrome at all.** The primary action is a near-black
  ink fill (`#0f0e0d`); the secondary is a grey pill (`#e5e5e5` borders /
  stone fills). Everything else is ink and grey.
- **One hue, on data only.** The main data element — the token-usage
  bars (sidebar plan block, Dashboard usage ring, Plans usage bar) —
  carried the single ledger hue `#1d5fbf` under Piece 34. **Piece 35
  supersedes this:** the usage element now wears the dual tone (see
  the brand layer below). The `ledger` token stays defined but is no
  longer the usage fill. Colour still appears nowhere else in the
  chrome: not on buttons, links, nav, badges or charts' chrome.
- **Status is a word, not a fill.** Meaning survives as a small coloured
  dot beside a sentence-case word (supported/refuted/inconclusive,
  Superseded, treatment states, payment status) in the signal colours
  green `#16a34a`, red `#a02c2c`, amber `#b7791f`. The token bar itself
  turns `#a02c2c` at ≥90% used — that colour change *is* the signal.
  No tinted pills, badges or panels anywhere.
- **Flat surfaces.** No shadows on anything at rest; separation is a
  1px hairline (`#e5e5e3`) or a tone step. Shadows survive only on what
  truly floats: dialogs, menus, toasts.
- **Radii: the tight pair.** `lex` 6px (controls), `lex-lg` 8px (cards).
  Progress bars and dots stay fully round — they are data shapes, not
  chrome.
- **Labels are sentence case** at 12.5–13px in stone; no uppercase,
  no letter-tracking. **Figures are tabular** (`tabular-nums`) in the
  sans at medium weight — KPIs are bare type in one hairline-divided
  strip, never tiled cards.
- **The face is legal paper.** The ground stays the warm ivory
  `#fafaf9` with white cards. The product's signature element is its
  own: verdict-first answers with citation chips — kept, and quieter
  (borderless stone-100 chips that invert to ink on hover).

## Brand layer (user-directed, Piece 35)

Added 31 Oct 2026 at the user's own direction, on top of the ledger
system above. These are the user's brand choices, carried over from
the taste-skill page he had built the same day.

- **The dual tone: terracotta `#c4664a` → sage `#8fa382`** (Tailwind
  tokens `terra` / `sage`). It appears in exactly three places, always
  soft — tints and washes, never saturated blocks:
  1. the token-usage data element (`.dual-tone-bar` fills in the
     sidebar plan block and Plans; an SVG gradient on the Dashboard
     usage ring). The ≥90%/exhausted state still turns the fill
     `#a02c2c` — the data signal beats the brand;
  2. the moving backdrop (below);
  3. one gradient wordmark (`.dual-tone-text`) — the product name on
     the Auth page. Gradient text is allowed nowhere else.
- **Moving background.** `DualToneBackdrop` (ui.tsx): two large
  blurred radial fields — terracotta top-left, sage bottom-right —
  drifting on a 26s ease-in-out alternate loop (`dualDrift` in
  index.css), at ~0.16 alpha in light mode and ~0.22 in dark.
  Placed only twice: full-screen behind the Auth card, and clipped
  inside the Dashboard greeting hero. It is pointer-events-none and
  decoration only; `prefers-reduced-motion` stills it.
- **Dark mode.** Class strategy (`darkMode: 'class'`): a pre-paint
  inline script in `index.html` adds `.dark` to `<html>` from the
  stored choice (`localStorage` key `jurisvault.theme` =
  `'dark' | 'light'`) or, with no stored choice, the system
  `prefers-color-scheme`. `ThemeToggle` (sidebar footer, Auth
  top-right) flips the class and persists the choice. The mapping is
  fixed: ivory ground → `#14120f`; white surface → `#1e1c19`;
  stone-100/50 fills and hovers → `white/5`–`white/10`; stone borders
  and dividers → `white/10`; ink text → `#f4f2ee` (ink-800/900 →
  `#e8e5e0`); stone-600/500/400 text → stone-300/400/500. `bg-ink`
  fills stay ink in both themes. The plain-CSS surfaces (prose,
  citation chips, scrollbars) have `.dark` overrides in index.css.

**Recorded exception.** The taste skill's house style bans gradients
outright. That ban is **overridden here by the user's own brand
decision**: the dual tone is his chosen identity for the product,
kept inside the three soft placements above. Every other Piece 34
rule — flat surfaces, no resting shadows, sentence-case labels,
tabular figures, ink primaries, dot-and-word status — still applies
in both themes.

## Shell — recorded deviation

The dealt sheet offered no-sidebar, then icon-rail shells. Both were
rejected: a 12-destination product needs labelled navigation. The shell
stays the app-screens chapter default (§3.1/§3.2): a **labelled sidebar
on an open plane** — 256px, 14px rows, the active item marked by a
neutral grey pill fill only (no ring, no bar, no colour). The wordmark
is the serif product name alone, with no icon tile.

## Palette (unchanged — Harvey REA tokens, 2026-10-20)

| Token | Value | Use |
|---|---|---|
| `--color-gray-50-ivory` | `#fafaf9` | Page background — warm ivory paper |
| `--color-gray-950-ink` | `#0f0e0d` | Primary text, primary buttons |
| `--color-gray-100` | `#f2f1f0` | Secondary surfaces |
| `--color-gray-200` | `#e5e5e3` | Hairline borders |
| `--color-gray-300` | `#cccac6` | Stronger borders |
| `--color-gray-400` | `#adaba5` | Placeholder/disabled |
| `--color-gray-500` | `#8f8b85` | Muted text |
| `--color-gray-600` | `#706d66` | Secondary text |
| `--color-gray-700` | `#524f49` | Tertiary emphasis |
| `--color-gray-800` | `#33312c` | — |
| `--color-gray-900` | `#1f1d1a` | — |
| `--color-green` | `#16a34a` | Success signal (dot/word only) |
| `--color-red` | `#a02c2c` | Negative signal (dot/word; delete action) |
| `--color-amber` | `#b7791f` | Notice signal (dot/word only) |
| `--color-ledger` | `#1d5fbf` | Piece 34's data hue — retained as a token; usage bars moved to the dual tone in Piece 35 |
| `--color-terra` | `#c4664a` | Dual tone, warm end (Piece 35 — washes, usage gradient, Auth wordmark only) |
| `--color-sage` | `#8fa382` | Dual tone, cool end (Piece 35 — same restricted placements) |
| `--color-dark-bronze` | `#593d3a` | Rare accent |

(The old `#f6f202` highlighter is retired: text selection is now the
neutral `#e5e5e3`.)

## Typography
- **UI sans**: system stack (`-apple-system, "Segoe UI", Inter…`)
- **Display serif**: Georgia/“Times New Roman” — kept for the wordmark,
  prose headings and long-form reading surfaces; screen titles and
  figures are set in the sans (page title 24px semibold, KPI values
  24px medium tabular).
- Weights: 400 / 500 / 600.

## Layout
- Left sidebar navigation: Dashboard · Briefs · Ask · Interact · Draft ·
  Research · Agents · Vault · Search documents · Private Vault ·
  Knowledge · Plans & Billing
- Ivory sidebar, hairline divider, serif product wordmark
- Assistant = home: greeting + composer, verdict-first answers,
  citation chips `[doc:1]` expanding to snippet cards
- Primary actions = ink-black buttons with white text; destructive =
  red, confirmed through the shared consent dialog
