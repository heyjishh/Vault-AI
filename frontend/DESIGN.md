# Juris Vault AI Frontend — REA Design Spec (from harvey.ai)

Reverse-engineered from **www.harvey.ai** via the REA repo (`rea analyze-web-bundle` + CSS token extraction, 2026-10-20).
Harvey's internal app sits behind Auth0, so this spec captures their **public design system**, which the app shares (semantic `--background-*` / `--text-*` / `--border-*` tokens).

## Palette (exact extracted tokens)
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
| `--color-green` | `#16a34a` | Success accent |
| `--color-red-500` | `#f26161` | Destructive |
| `--color-yellow` | `#eab308` | Warning |
| `--color-highlighter` | `#f6f202` | Citation highlight |
| `--color-dark-bronze` | `#593d3a` | Rare accent |

## Typography
- **UI sans**: HarveySans = Diatype Variable → fallback system stack (`-apple-system, "Segoe UI", Inter…`)
- **Display serif**: HarveySerif (regular + italic) for headlines → fallback Georgia/“Times New Roman” serif
- Weights: 400 / 500 / 700. Generous whitespace, tight tracking on display.

## Shape & surface
- Radius: 4px (buttons), 10–12px (cards), 16px max. Subtle shadows: `0 1px 3px rgba(0,0,0,.04)`.
- Header height 4.5rem; hairline 1px borders; minimal chrome.

## Layout (Harvey app structure)
- Left sidebar navigation: Assistant · Research · Agents · Vault · Knowledge Graph · System
- Ivory sidebar, hairline divider, serif product wordmark
- Assistant = home: centered greeting + composer card, mode pills, citation chips `[doc:1]` expanding to snippet cards
- Primary actions = ink-black buttons with white text; destructive = red ghost
