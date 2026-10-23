/** @type {import('tailwindcss').Config} */
export default {
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
        'app-bg': '#fafaf9',
        'surface': '#FFFFFF',
        'surface-2': '#f2f1f0',
        'border-default': '#e5e5e3',
        'text-primary': '#0f0e0d',
        'text-secondary': '#706d66',
        'text-muted': '#8f8b85',
      },
      fontFamily: {
        sans: ['-apple-system', 'BlinkMacSystemFont', '"Segoe UI"', 'Inter', 'system-ui', 'sans-serif'],
        display: ['Georgia', '"Times New Roman"', 'serif'],
        mono: ['ui-monospace', 'SFMono-Regular', 'Menlo', 'Monaco', 'Consolas', 'monospace'],
      },
      borderRadius: {
        'lex': '4px',
        'lex-lg': '10px',
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
