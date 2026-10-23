import { create } from 'zustand'

const KEY = 'jurisvault.currentMatter'

interface MatterState {
  /** The matter the whole app is currently scoped to (null = full vault). */
  currentMatterId: string | null
  setCurrentMatter: (id: string | null) => void
}

export const useMatter = create<MatterState>((set) => ({
  currentMatterId: localStorage.getItem(KEY),
  setCurrentMatter: (id) => {
    if (id) localStorage.setItem(KEY, id)
    else localStorage.removeItem(KEY)
    set({ currentMatterId: id })
  },
}))
