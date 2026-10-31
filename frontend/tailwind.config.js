/** @type {import('tailwindcss').Config} */
export default {
  // Piece 35: dark mode is a class on <html> (pre-paint script in
  // index.html + ThemeToggle), never the media query alone.
  darkMode: 'class',
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  theme: {
    extend: {
      colors: {
        // REA'd from harvey.ai (see DESIGN.md)
        ivory: '#fafaf9',
        ink: {
          DEFAULT: '#0f0e0d',
          900: '#1f1d1a',
          800: '#33312c',
          700: '#524f49',
        },
        stone: {
          100: '#f2f1f0',
          200: '#e5e5e3',
          300: '#cccac6',
          400: '#adaba5',
          500: '#8f8b85',
          600: '#706d66',
          700: '#524f49',
        },
        brand: {
          DEFAULT: '#0f0e0d',
          green: '#16a34a',
          red: '#f26161',
          amber: '#eab308',
          highlighter: '#f6f202',
        },
        // Piece 34 (achromatic ledger): the ONE data hue. Piece 35 moved
        // the token-usage bars to the dual tone, so ledger is retained
        // only as a defined token — no longer the usage-bar fill.
        ledger: '#1d5fbf',
        // Dual tone (Pieces 35–36; Piece 42 re-based on the user's
        // own jurisvault.ai): the live theme runs on the
        // --accent-a/--accent-b CSS variables in index.css; these
        // tokens hold the accent pair's values (#2ad06f / #21c063,
        // the brand site's auth-button gradient). See DESIGN.md.
        terra: '#2ad06f',
        sage: '#21c063',
        'app-bg': '#fafaf9',
        'surface': '#FFFFFF',
        'surface-2': '#f2f1f0',
        'border-default': '#e5e5e3',
        'text-primary': '#0f0e0d',
        'text-secondary': '#706d66',
        'text-muted': '#8f8b85',
      },
      fontFamily: {
        // Piece 42: jurisvault.ai's type system — Inter for UI,
        // Archivo (700/800/900) for display.
        sans: ['Inter', '-apple-system', 'BlinkMacSystemFont', '"Segoe UI"', 'system-ui', 'sans-serif'],
        display: ['Archivo', 'Inter', 'system-ui', 'sans-serif'],
        mono: ['ui-monospace', 'SFMono-Regular', 'Menlo', 'Monaco', 'Consolas', 'monospace'],
      },
      borderRadius: {
        // Piece 34: the tight pair — 6px controls, 8px cards.
        'lex': '10px',
        'lex-lg': '14px',
      },
      boxShadow: {
        'lex': '0 1px 3px rgba(0,0,0,0.04), 0 1px 2px rgba(0,0,0,0.04)',
        'lex-md': '0 4px 12px rgba(0,0,0,0.06)',
      },
      fontSize: {
        'meta': ['12px', { lineHeight: '16px', letterSpacing: '0.02em' }],
        'meta2': ['13px', { lineHeight: '16px' }],
      }
    },
  },
  plugins: [],
}
