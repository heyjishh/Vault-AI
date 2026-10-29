import { create } from 'zustand'

const KEY = 'jurisvault.currentMatter'

interface MatterState {
  /** The brief the whole app is currently scoped to. Brief-only
   *  vault: an active brief always exists — AppShell resolves null
   *  (nothing chosen yet, or a stale pick) to the default brief as
   *  soon as the brief list loads, and Vault/Interact resolve the
   *  same fallback locally, so no surface ever shows an unscoped
   *  "full vault". null therefore only survives transiently, plus
   *  for the rest of a session after Ask's in-page widening (Ask
   *  retrieval internals are unchanged by the brief-only model). */
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
