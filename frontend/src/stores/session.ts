import { create } from 'zustand'

/** Chat history + "ask from anywhere" bridge + toast queue. */

interface Toast {
  id: number
  kind: 'success' | 'error' | 'info'
  message: string
}

interface SessionState {
  history: string[]
  pendingQuestion: string | null
  ask: (q: string) => void
  consumePending: () => string | null
  /** A past activity result waiting to be restored into its page. */
  restore: { kind: string; payload: Record<string, unknown>; nonce: number } | null
  setRestore: (kind: string, payload: Record<string, unknown>) => void
  consumeRestore: (kind: string) => Record<string, unknown> | null
  addHistory: (q: string) => void
  clearHistory: () => void
  toasts: Toast[]
  toast: (kind: Toast['kind'], message: string) => void
  dismissToast: (id: number) => void
}

const HISTORY_KEY = 'lexagent.history'

function loadHistory(): string[] {
  try {
    const raw = localStorage.getItem(HISTORY_KEY)
    return raw ? (JSON.parse(raw) as string[]) : []
  } catch {
    return []
  }
}

let toastId = 0

export const useSession = create<SessionState>((set, get) => ({
  history: loadHistory(),
  pendingQuestion: null,
  restore: null,
  toasts: [],

  setRestore: (kind, payload) => set({ restore: { kind, payload, nonce: Date.now() } }),

  consumeRestore: (kind) => {
    const r = get().restore
    if (r && r.kind === kind) {
      set({ restore: null })
      return r.payload
    }
    return null
  },

  ask: (q) => set({ pendingQuestion: q }),

  consumePending: () => {
    const q = get().pendingQuestion
    if (q) set({ pendingQuestion: null })
    return q
  },

  addHistory: (q) => {
    const next = [q, ...get().history.filter((h) => h !== q)].slice(0, 12)
    localStorage.setItem(HISTORY_KEY, JSON.stringify(next))
    set({ history: next })
  },

  clearHistory: () => {
    localStorage.removeItem(HISTORY_KEY)
    set({ history: [] })
  },

  toast: (kind, message) => {
    const id = ++toastId
    set((s) => ({ toasts: [...s.toasts, { id, kind, message }] }))
    setTimeout(() => get().dismissToast(id), 4200)
  },

  dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
}))
