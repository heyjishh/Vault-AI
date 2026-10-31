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
- **The face is legal paper.** Cards stay warm near-white
  (`#fffef9`-family) and the ground is the dual-tone page wash (see
  the brand layer below; Piece 36). The product's signature element
  is its own: verdict-first answers with citation chips — kept, and
  quieter (borderless stone-100 chips that invert to ink on hover).

## Brand layer (user-directed, Pieces 35–36)

Added 31 Oct 2026 at the user's own direction, on top of the ledger
system above. These are the user's brand choices, carried over from
the taste-skill page he had built the same day. **Piece 36: theme
tokens are taken 1:1 from the taste-skill page (the user's
reference); Piece 35's approximated values were superseded.** The
whole page background IS the dual-tone wash, in both themes, with
the reference's glow fields drifting over it and its accent
variables flipping with the theme.

| Token | Light | Dark |
|---|---|---|
| Page wash (`--grad-page`) | `linear-gradient(155deg,#f7eedd 0%,#f3efe0 34%,#e9ecda 68%,#dde5cd 100%)` | `linear-gradient(155deg,#221a12 0%,#191a13 38%,#12150d 70%,#0d120a 100%)` |
| Glow (`--grad-glow`) | peach `rgba(240,190,120,.50)` at 12% 0% + sage `rgba(146,168,120,.45)` at 92% 8% | terracotta `rgba(214,130,70,.28)` at 12% 0% + sage `rgba(120,150,95,.26)` at 92% 8% |
| Card | `#fffef9` (the app keeps `bg-white` — negligibly different) | `#1d1f19` |
| Raised / chip | `#efece0` (stone-100 fills) | `#26281f` |
| Ink | `#16150f` | `#f0eee3` |
| Accent A (`--accent-a`, terracotta) | `#b4552d` | `#e08a5f` |
| Accent B (`--accent-b`, sage) | `#5c7350` | `#9db488` |

(The glow alphas sit where the reference source puts them: the
stronger glow belongs to its light theme, the softer one to dark.)

- **The page wash.** `.app-wash` (index.css) paints
  `var(--grad-page)` with `background-attachment: fixed` on the
  AppShell and Auth roots, and `body` carries the same gradient so
  overscroll matches (fallback end-stops `#dde5cd` / `#0d120a`).
  The sidebar paints no background of its own — it sits transparent
  over the wash behind its hairline.
- **Moving background.** `DualToneBackdrop` (ui.tsx) renders the
  glow recipe as one layer (`background: var(--grad-glow)`)
  drifting on a 26s ease-in-out alternate loop (`dualDrift` in
  index.css). One instance is fixed behind the whole app in
  AppShell; Auth keeps its own. It is pointer-events-none and
  decoration only; `prefers-reduced-motion` stills it.
- **The dual tone on data.** `.dual-tone-bar` and `.dual-tone-text`
  are the reference's CTA recipe — `linear-gradient(120deg,
  var(--accent-a), var(--accent-b))` — so they flip with the theme:
  the usage-bar fills (sidebar plan block, Plans), and the one
  gradient wordmark (the product name on the Auth page; gradient
  text is allowed nowhere else). The Dashboard usage ring's SVG
  stops read the same variables. The ≥90%/exhausted state still
  turns the fill `#a02c2c` — the data signal beats the brand.
  Tailwind tokens `terra` / `sage` hold the light values.
- **Dark mode.** Class strategy (`darkMode: 'class'`): a pre-paint
  inline script in `index.html` adds `.dark` to `<html>` from the
  stored choice (`localStorage` key `jurisvault.theme` =
  `'dark' | 'light'`) or, with no stored choice, the system
  `prefers-color-scheme`. `ThemeToggle` (sidebar footer, Auth
  top-right) flips the class and persists the choice. The mapping:
  the ground is the dark wash (above); white surface → `#1d1f19`
  (the reference card); stone-100 fills → `#26281f` (the reference
  raised/chip); stone-50 and white/50 washes and hovers →
  `white/5`–`white/10`; stone borders and dividers → `white/10`;
  ink text → `#f4f2ee` (ink-800/900 → `#e8e5e0`); stone-600/500/400
  text → stone-300/400/500. `bg-ink` fills stay ink in both themes.
  The plain-CSS surfaces (prose, citation chips, scrollbars) have
  `.dark` overrides in index.css.

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
| `--color-terra` | `#b4552d` | Dual tone, warm end — the taste-skill page's `--accent-a` (Piece 36; light value, dark flips via the CSS variables) |
| `--color-sage` | `#5c7350` | Dual tone, cool end — the taste-skill page's `--accent-b` (Piece 36; same note) |
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
- Sidebar transparent over the page wash, hairline divider, serif product wordmark
- Assistant = home: greeting + composer, verdict-first answers,
  citation chips `[doc:1]` expanding to snippet cards
- Primary actions = ink-black buttons with white text; destructive =
  red, confirmed through the shared consent dialog
