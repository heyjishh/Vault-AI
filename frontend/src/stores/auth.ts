import { create } from 'zustand'
import { authApi, tokens } from '../api/client'
import type { UserView } from '../api/types'

interface AuthState {
  user: UserView | null
  authed: boolean
  loading: boolean
  error: string | null
  login: (email: string, password: string) => Promise<void>
  register: (email: string, password: string) => Promise<void>
  logout: () => void
  hydrate: () => void
  setUser: (u: UserView) => void
}

export const useAuth = create<AuthState>((set) => ({
  user: null,
  authed: false,
  loading: false,
  error: null,

  hydrate: () => {
    if (tokens.access) {
      set({ authed: true })
      authApi.me().then((u) => set({ user: u })).catch(() => { /* stale token; interceptor handles */ })
    }
  },

  setUser: (u) => set({ user: u }),

  login: async (email, password) => {
    set({ loading: true, error: null })
    try {
      const r = await authApi.login(email, password)
      tokens.save(r.tokens)
      set({ user: r.user, authed: true, loading: false })
    } catch (e) {
      set({ error: (e as Error).message, loading: false })
      throw e
    }
  },

  register: async (email, password) => {
    set({ loading: true, error: null })
    try {
      const r = await authApi.register(email, password)
      tokens.save(r.tokens)
      set({ user: r.user, authed: true, loading: false })
    } catch (e) {
      set({ error: (e as Error).message, loading: false })
      throw e
    }
  },

  logout: () => {
    tokens.clear()
    set({ user: null, authed: false })
  },
}))
