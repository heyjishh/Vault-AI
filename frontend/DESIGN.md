# Juris Vault AI Frontend — Design System

**Taste (achromatic ledger) over the Harvey-derived palette.**
Piece 34 (31 Oct 2026) restyled the user app with the design-taste-frontend
skill (tasteskill v2, app-screen run): direction *achromatic-ledger*,
seed 1210974860, kind workspace, light — with the brand palette and type
kept from the Harvey REA below. The admin Command Center kept its own
look through Pieces 34–38; Piece 39 brought it onto this same system
(see "Admin parity" below), and Piece 40 (lighter still, the running
gradient, the professional finish) applies to both apps identically.
**Piece 42 supersedes the palette and the display face**: the theme
is now the user's own jurisvault.ai brand theme — see "Brand
re-base" below; the taste-skill token table under "Brand layer" is
kept as history.

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

## Brand layer (user-directed, Pieces 35–36, 38, 40)

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
| Page wash (`--grad-page`) | `linear-gradient(155deg,#fefcf8 0%,#fcfaf6 34%,#f9faf3 68%,#f4f7ec 100%)` | `linear-gradient(155deg,#221a12 0%,#191a13 38%,#12150d 70%,#0d120a 100%)` |
| Glow (`--grad-glow`) | peach `rgba(240,190,120,.26)` at 12% 0% + sage `rgba(146,168,120,.24)` at 92% 8% | terracotta `rgba(214,130,70,.28)` at 12% 0% + sage `rgba(120,150,95,.26)` at 92% 8% |
| Card | `#fffef9` (the app keeps `bg-white` — negligibly different) | `#1d1f19` |
| Raised / chip | `#efece0` (stone-100 fills) | `#26281f` |
| Ink | `#16150f` | `#f0eee3` |
| Accent A (`--accent-a`, terracotta) | `#b4552d` | `#e08a5f` |
| Accent B (`--accent-b`, sage) | `#5c7350` | `#9db488` |

(The glow alphas sit where the reference source puts them: the
stronger glow belongs to its light theme, the softer one to dark.)

**Piece 38 (user-directed).** The light wash and glow stops in
the table are lightened toward white at the user's direction
("make it more light background") — the same 155° direction and
stop positions, paler values. The dark stops remain 1:1 with the
taste-skill reference, untouched.

**Piece 40 (user-directed).** "Make more light and running
gradiant… but do more light, make professional." Three changes,
applied identically to both apps. (1) The light wash and glow are
lightened a second time (the table above carries the current
values); the dark colours are again untouched — only the motion
below is shared. (2) **The running gradient:** the architecture
changed so the wash itself flows — see "The page wash" and
"Moving background" below. (3) **The professional finish:** one
consistent focus treatment app-wide, `:focus-visible` with a 2px
bronze outline at 2px offset (a floor under components' own focus
utilities), and a branded selection tint — bronze at 16% on light,
pale bronze at 28% on dark — instead of the flat grey.

- **The page wash.** The wash is painted by the
  `DualToneBackdrop` layer (below), not by the roots: `.app-wash`
  on the AppShell and Auth roots now carries only the solid
  end-stop fallback (`#f4f7ec` light / `#0d120a` dark), and `body`
  carries the same fallback so overscroll matches — no double
  gradient paint anywhere. The sidebar paints no background of
  its own — it sits transparent over the wash behind its hairline.
- **Moving background — the running gradient.** `DualToneBackdrop`
  (ui.tsx) renders ONE layer carrying both backgrounds:
  `background-image: var(--grad-glow), var(--grad-page)` with the
  glow held at size `auto`, position `0 0`, and the wash painted
  oversized at `175% 175%`. The `washRun` loop (index.css, 34s
  ease-in-out infinite alternate) pans the wash layer's
  `background-position` from `0% 0%` to `100% 100%`, so the whole
  gradient travels diagonally — a slow, continuous flow, in dark
  mode too (same stops; the animation is motion, not colour).
  One instance is fixed behind the whole app in AppShell; Auth
  keeps its own. It is pointer-events-none and decoration only;
  `prefers-reduced-motion` stills it. (Pieces 36–39 instead
  drifted the glow layer alone by transform over a static wash;
  Piece 40 supersedes that.)
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

## Brand re-base — the jurisvault.ai theme (Piece 42)

JISHAN: "does it is a running background? of gradiant?
https://jurisvault.ai/ use this theme of mine". The product now
wears the theme of his own brand site, in both apps, tokens taken
1:1 from the site's CSS. This supersedes the taste-skill palette
of Pieces 36–40 (their token table above is history) and the
Georgia display face.

- **Accent: the brand green.** `--accent-a` `#2ad06f` →
  `--accent-b` `#21c063` in BOTH themes — the site's auth-button
  gradient pair (the site runs it at 180deg; the app's dual-tone
  recipe keeps its 120deg form). `.dual-tone-bar` / `.dual-tone-text`
  — the usage bars, the Dashboard ring, the one gradient wordmark —
  now run green.
- **Bronze carries the brand green.** `--bronze` is `#21c063` on
  light and `#2ad06f` on dark. The variable keeps its name and its
  reserved uses (Piece 37): the brand mark, brand rules and the
  active-nav indicator — plus the Piece 40 focus ring and the
  selection tint, now green (`rgba(33,192,99,.16)` light /
  `rgba(42,208,111,.28)` dark). The favicon tile is brand green
  with an ivory "J".
- **Type: Archivo + Inter (user-directed — his brand fonts).**
  Display is Archivo (700/800/900) wherever the display face is
  used — wordmark, headings, the Dashboard greeting, prose
  headings — replacing Georgia outright; the UI sans is Inter
  first. Both families load from Google Fonts in each app's
  `index.html`. The serif→Archivo change is deliberate: the brand
  site's voice is a heavy grotesque, not a serif.
- **Light ground.** `--grad-page` is `linear-gradient(155deg,
  #ffffff 0%,#f7f7f7 55%,#eef5f0 100%)` — the site's light app
  whites (#fff / soft #f7f7f7, borders #e5e5e5) melting into pale
  green — under a soft green glow (`rgba(33,192,99,.10)` +
  `rgba(42,208,111,.08)`). It still RUNS: the Piece 40 `washRun`
  architecture is unchanged, so the wash flows as before. The
  `.app-wash` / body fallback is the end stop `#eef5f0`.
- **Dark ground = the site's auth emerald.** `--grad-page` is the
  auth base `linear-gradient(155deg,#04100b 0%,#061a12 55%,
  #04100b 100%)` and `--grad-glow` is the site's two radial fields
  (`rgba(33,192,99,.15)` at 22% 48%, `rgba(33,192,99,.09)` at
  88% 8%). Dark surfaces follow the site's auth card: card
  `#0a1a12`, raised `#122419` — the solid equivalents of its
  translucent card/panel tones — swept mechanically through both
  src trees. The dark overscroll fallback is `#04100b`.
- **The running auth scene.** Both sign-in screens (user Auth,
  admin gate) run the site's login background, alive: the emerald
  base stack, a 1px grid (`rgba(255,255,255,.08)`, 44px cells)
  under the site's centre mask, two counter-rotating conic rings
  (`authSpin` 46s, and 68s reverse, masked to a 2px ring), a
  pulsing core glow (`authPulse` 5.5s: opacity .55→1, scale
  1→1.07) and 14 twinkling spark dots (`authTwinkle` 4s). It is
  rendered by `AuthScene` (ui.tsx in both apps; recipes in
  index.css) inside a root carrying `dark auth-scene
  bg-[#04100b]`: the `dark` class is LOCAL to the auth root, so
  the scene is always dark whatever the app theme — the pre-paint
  html theme and everything beyond sign-in are untouched, and
  ThemeToggle stays on the page, still setting the app-wide
  preference. `.auth-scene` sets the site's auth text colour
  `#e9f3ed` so unclassed copy inherits light-on-emerald. All
  scene motion stops under `prefers-reduced-motion`. The shells
  keep the running wash backdrop — the scene is auth-only.

## Deeper adoption — the site's components (Piece 44)

JISHAN: "make more enhancement and running gradient, and adopt
more thing from main website, so it look alike same". Piece 42
took the site's palette, type and auth background; Piece 44 takes
its component language, in both apps.

- **Brand primary buttons.** `.btn-brand` (index.css in both
  apps) is the site's `.auth-btn-primary` minus its `width:100%`:
  `linear-gradient(180deg,#2ad06f,#21c063)`, white 600 text, 12px
  radius, a soft green shadow (`0 10px 26px -10px
  rgba(33,192,99,.7)`), brighten + deeper shadow on hover, 1px
  press-down on active, flat 60% opacity when disabled. Every
  primary action wears it now (37 buttons): sign-in submits, Ask /
  Interact / Dashboard-hero sends, the Agents / Research / Draft
  run buttons, Vault upload + brief uploads, Save actions, brief /
  space / member creates, Plans subscribe + pay-confirm, the
  Private Vault set / unlock / change / seal actions, the pet-menu
  actions, the error view's Reload, and in admin the sign-in
  submit, Save buttons (Customize, Billing, System Settings),
  Corpus ingest, Golden run / save / add, Annotations serve / add,
  and the admin-only screen's "Open Juris Vault AI". NOT
  converted, deliberately: destructive actions (the ConfirmDialog
  confirm stays red), nav items and the sidebar "New research"
  CTA, tab pills (incl. the DocInsight view switcher's selected
  state), toggles, badges/chips, the AppShell composer-adjacent
  utilities, the Refine "Refined — Undo" state chip, and inline
  retries ("Try again" in DocInsight).
- **Auth scene: orbits + floating nodes.** The conic rings are
  replaced by the site's orbit system: a dashed green orbit
  (`rgba(33,192,99,.21)`, `authSpin` 46s) around a plain hairline
  inner orbit inset 20% (68s reverse), the pair sized
  `min(88vmin,54rem)`. Five labelled nodes bob on the site's 7s
  `authFloat` loop at the site's own positions/delays — the user
  app wears the site's five (AI Research, Case Laws, Drafts,
  Statutes, Judgments); the admin wears Overview, Users, Corpus,
  Billing, Golden Rules. Node style is the site's: a 44px dark
  glass dot (`rgba(8,26,19,.85)`, blur, green ring + glow) with a
  green icon over an 11px `#b6c9bf` label; nodes render on large
  screens only. The core glow, sparks, grid and base are
  unchanged. (One implementation note: `.auth-node` sets no
  `display` in CSS — the nodes carry Tailwind `hidden lg:flex`,
  and an unlayered display rule would defeat the small-screen
  hiding in the cascade.)
- **The app backdrop moves more.** `DualToneBackdrop` gains a
  child layer, `.backdrop-spin`: a conic sheen (two soft green
  wedges, `.07`/`.05` alpha on light, `.14`/`.10` on dark)
  rotating on `authSpin` at 70s — the auth scene's spin, at
  whisper volume, behind the whole app. The glow also breathes:
  `glowPulse` 9s alternate (opacity .78 ↔ 1) joins `washRun` on
  `.dual-tone-backdrop`. Both motions stop under
  `prefers-reduced-motion`.
- **Radii + heading weight.** The radius pair steps up to the
  site's scale: `lex` 6px → 10px, `lex-lg` 8px → 14px (its
  buttons are 12px, its cards rounded-xl and up). A plain-CSS
  `.font-display { font-weight: 700 }` sits after the Tailwind
  directives in both apps, so Archivo headings read at the
  masthead's bold weight wherever the display family is used.
- **Dashboard hero field (user app).** The greeting band gains
  `.hero-field` behind its content: the auth grid (44px cells)
  and a green radial glow (`36rem 18rem at 18% 0%`, `.10` light /
  `.14` dark; lines ink `.045` light / white `.05` dark) under a
  top-anchored radial mask that fades the field out downward —
  the sign-in scene's texture, in the light register, where the
  user lands.

## Legal identity (Piece 37)

JISHAN's verdict on Pieces 34–36: the theme was aligned and dark
mode was fine, but the product "did not seem like a legal business"
— the soft pastel wash reads generic, and nothing on screen carried
the gravitas of a law practice. Piece 37 fixes identity, not theme:
the Piece 36 wash, glow, dual-tone data accents, and all dark-mode
work are untouched.

- **The medallion mark.** `BrandMark` (ui.tsx): the lucide scales of
  justice centred in a 1.5px bronze ring, transparent fill. It is
  the only brand mark — the sidebar lockup and both Auth nameplates
  carry it. **Superseded by Piece 43 (1 Nov 2026):** BrandMark now
  renders the jurisvault.ai logo itself — the green shield-and-
  scales emblem (`public/logo.png`, byte-identical to the main
  website's header logo) in the site's own white-chip treatment —
  and both favicons are the emblem.
- **The serif lockup.** Product name in the display serif (21px in
  the sidebar, 24px on Auth) with the tagline beneath at 11.5px,
  and a 2px bronze rule, 44px wide, under the lockup. The Dashboard
  greeting returns to the display serif at 30px, with the same rule
  above it and its finding line unchanged beneath.
- **Bronze.** `--bronze` in index.css: `#593d3a` on light — the dark
  bronze of the original Harvey palette, the legal metal — and
  `#c8b184` on dark, applied through `.text-bronze`, `.bg-bronze`
  and `.border-bronze`. Bronze is reserved for exactly three uses:
  the brand mark, brand rules, and the active-nav indicator (a 2px
  bronze left bar inset on the active sidebar pill). Nothing else
  recolours.
- **The "L" monogram is gone.** The black rounded square with a
  serif "L" — a remnant of the retired LexAgent name — survived on
  the Auth page (both lockups) and in the favicon. The favicon is
  now a bronze (`#593d3a`) square with an ivory serif "J".

## Admin parity (Piece 39)

The Command Center (`frontend/admin/`) now runs this same design
system — JISHAN: "do make admin same". Its token layer is a 1:1
port of the user app's `index.css` (the Piece 38 lightened light
wash and glow — and, since Piece 40, the second lightening, the
running-gradient architecture and the focus/selection finish,
kept identical in both apps; the dark wash, glow, accents and
bronze unchanged),
with `darkMode: 'class'` and the same radii (`lex` 6px, `lex-lg`
8px). The pre-paint theme script is identical and shares the
`jurisvault.theme` localStorage key — same origin, so one choice
governs both apps. The admin sidebar is transparent over the wash
(the old always-dark ink sidebar is gone); its lockup is the
medallion + serif name + "Command Center" second line + bronze
rule, the active nav item gains the bronze inset left bar, and the
sign-in gate mirrors the user Auth nameplate with a ThemeToggle
top-right (a second toggle sits in the shell footer). The ledger
tells were swept through the admin pages too: no resting shadows,
sentence-case labels, chrome buttons at `rounded-lex`. Known
leftovers, same class as the user app's: tinted data pills keep
their light tints in dark mode, and the shared ErrorBanner keeps
its light red tint in both apps.

## Shell — recorded deviation

The dealt sheet offered no-sidebar, then icon-rail shells. Both were
rejected: a 12-destination product needs labelled navigation. The shell
stays the app-screens chapter default (§3.1/§3.2): a **labelled sidebar
on an open plane** — 256px, 14px rows, the active item marked by a
neutral grey pill fill (Piece 37 adds the bronze left bar — see Legal
identity above). The lockup is the medallion + serif name + tagline
specified in the Legal identity section.

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
