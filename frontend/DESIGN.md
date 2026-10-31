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
- **One hue, on data only.** `#1d5fbf` (Tailwind token `ledger`) appears
  only on the main data element — the token-usage bars (sidebar plan
  block, Dashboard usage ring, Plans usage bar). Nowhere else: not on
  buttons, links, nav, badges or charts' chrome.
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
| `--color-ledger` | `#1d5fbf` | **The one data hue — token-usage bars only** |
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
