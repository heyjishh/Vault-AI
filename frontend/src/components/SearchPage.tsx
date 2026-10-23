import { useEffect, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { FileText, Search } from 'lucide-react'
import { vaultApi } from '../api/client'
import type { DocSearchHit } from '../api/types'
import type { View } from './AppShell'
import { EmptyState, Spinner } from './ui'

/** Render a snippet with the matched term highlighted. */
function Snippet({ text, needle }: { text: string; needle: string }) {
  const lower = text.toLowerCase()
  const n = needle.toLowerCase()
  const parts: React.ReactNode[] = []
  let i = 0
  let k = 0
  while (true) {
    const idx = lower.indexOf(n, i)
    if (idx === -1) {
      parts.push(text.slice(i))
      break
    }
    if (idx > i) parts.push(text.slice(i, idx))
    parts.push(
      <mark key={k++} className="bg-[#fdf3d7] text-ink rounded px-0.5">
        {text.slice(idx, idx + n.length)}
      </mark>,
    )
    i = idx + n.length
  }
  return <p className="text-[13px] text-stone-500 leading-relaxed">{parts}</p>
}

export default function SearchPage({ go }: { go: (v: View) => void }) {
  const [input, setInput] = useState('')
  const [q, setQ] = useState('')

  useEffect(() => {
    const t = setTimeout(() => setQ(input.trim()), 350)
    return () => clearTimeout(t)
  }, [input])

  const results = useQuery({
    queryKey: ['doc-search', q],
    queryFn: () => vaultApi.search(q),
    enabled: q.length >= 2,
  })

  const openInInteract = (hit: DocSearchHit) => {
    sessionStorage.setItem('jurisvault.interactDoc', hit.doc_id)
    go('interact')
  }

  return (
    <div className="max-w-3xl mx-auto px-6 py-10">
      <h1 className="font-display text-[2rem] tracking-tight">Search documents</h1>
      <p className="text-stone-500 mt-1.5 text-[15px]">
        Find documents in the corpus by title or by any word inside them — names, sections, amounts, clauses.
      </p>

      <div className="mt-6 flex items-center gap-2.5 rounded-full border border-stone-200 bg-white shadow-lex px-4 py-3 focus-within:border-stone-400 transition-colors">
        <Search size={17} className="text-stone-400 shrink-0" />
        <input
          autoFocus
          value={input}
          onChange={(e) => setInput(e.target.value)}
          placeholder="Search the corpus — try “Section 86”, a party name, an amount…"
          className="flex-1 min-w-0 bg-transparent outline-none text-[15px] placeholder:text-stone-400"
        />
        {results.isFetching && <span className="text-[12px] text-stone-400 shrink-0">Searching…</span>}
      </div>

      <div className="mt-6">
        {q.length < 2 ? (
          <EmptyState title="Type at least 2 characters" hint="Results appear as you type, with the matching passages highlighted." />
        ) : results.isLoading ? (
          <Spinner label="Searching the corpus…" />
        ) : (results.data ?? []).length === 0 ? (
          <EmptyState title={`No matches for “${q}”`} hint="Try a different spelling, a shorter term, or ask Clause instead — Ask searches by meaning, not just words." />
        ) : (
          <>
            <p className="text-[13px] text-stone-400 mb-3">
              {(results.data ?? []).length} document{(results.data ?? []).length === 1 ? '' : 's'} mention “{q}”
            </p>
            <div className="space-y-3">
              {(results.data ?? []).map((hit) => (
                <button
                  key={hit.doc_id}
                  onClick={() => openInInteract(hit)}
                  className="w-full text-left rounded-lex-lg border border-stone-200 bg-white shadow-lex px-5 py-4 hover:border-stone-300 hover:shadow-lex-md transition-all"
                >
                  <span className="flex items-center gap-2.5">
                    <FileText size={16} className="text-stone-400 shrink-0" />
                    <span className="font-display text-[16.5px] truncate">{hit.title}</span>
                    <span className="ml-auto text-[11.5px] font-medium text-stone-400 shrink-0">
                      {hit.title_match ? 'title match · ' : ''}{hit.matches} mention{hit.matches === 1 ? '' : 's'}
                    </span>
                  </span>
                  <span className="block mt-2.5 space-y-1.5">
                    {hit.snippets.map((s, i) => <Snippet key={i} text={s} needle={q} />)}
                  </span>
                  <span className="block mt-2.5 text-[12.5px] font-medium text-stone-400">Open in Interact →</span>
                </button>
              ))}
            </div>
          </>
        )}
      </div>
    </div>
  )
}
