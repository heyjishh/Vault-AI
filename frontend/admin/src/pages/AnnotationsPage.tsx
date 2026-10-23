import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { BadgeCheck, Plus, Trash2 } from 'lucide-react'
import { adminApi, type AnnotationRecord } from '../api'
import { ErrorBanner, PageHead, Spinner } from '../ui'

function AnnotationRow({ a, currentFp }: { a: AnnotationRecord; currentFp: string }) {
  const queryClient = useQueryClient()
  const invalidate = () => queryClient.invalidateQueries({ queryKey: ['annotations'] })
  const del = useMutation({ mutationFn: () => adminApi.deleteAnnotation(a.id), onSuccess: invalidate })
  const toggle = useMutation({
    mutationFn: (enabled: boolean) => adminApi.toggleAnnotation(a.id, enabled),
    onSuccess: invalidate,
  })
  const stale = a.corpus_fp !== currentFp
  return (
    <div className="rounded-lex border border-stone-200 bg-white p-4">
      <div className="flex items-start justify-between gap-3">
        <p className="font-medium text-[15px] leading-snug">{a.question}</p>
        <div className="flex items-center gap-1.5 flex-shrink-0">
          {stale ? (
            <span className="rounded-full bg-[#fdf0e0] text-[#9a6212] px-2 py-0.5 text-[12px]">stale — corpus changed</span>
          ) : a.enabled ? (
            <span className="rounded-full bg-[#e7f4e4] text-[#2f6b2f] px-2 py-0.5 text-[12px]">serving</span>
          ) : (
            <span className="rounded-full bg-stone-100 text-stone-500 px-2 py-0.5 text-[12px]">paused</span>
          )}
          <span className="rounded-full bg-stone-100 text-stone-500 px-2 py-0.5 text-[12px]">
            {a.scope_doc_ids ? `brief · ${a.scope_doc_ids.length} docs` : 'all unscoped'}
          </span>
        </div>
      </div>
      <p className="text-[14px] text-stone-600 mt-2 leading-relaxed line-clamp-3">{a.answer}</p>
      <div className="flex items-center justify-between mt-3">
        <p className="text-[12px] text-stone-400">
          {a.citations.length} source{a.citations.length === 1 ? '' : 's'} · {a.source} · by {a.created_by} ·{' '}
          {new Date(a.created_at).toLocaleDateString('en-IN', { day: 'numeric', month: 'short', year: 'numeric' })}
        </p>
        <div className="flex items-center gap-2">
          <button
            onClick={() => toggle.mutate(!a.enabled)}
            className="text-[13px] font-medium text-stone-600 hover:text-ink border border-stone-200 rounded-lex px-2.5 py-1"
          >
            {a.enabled ? 'Pause' : 'Resume'}
          </button>
          <button
            onClick={() => del.mutate()}
            className="text-stone-400 hover:text-[#a02c2c] p-1"
            title="Delete annotation"
          >
            <Trash2 size={15} />
          </button>
        </div>
      </div>
      {(del.isError || toggle.isError) && (
        <p className="text-[13px] text-[#a02c2c] mt-2">{(del.error ?? toggle.error)?.message}</p>
      )}
    </div>
  )
}

export default function AnnotationsPage() {
  const queryClient = useQueryClient()
  const data = useQuery({ queryKey: ['annotations'], queryFn: adminApi.annotations })
  const insights = useQuery({ queryKey: ['insights'], queryFn: adminApi.insights })

  const [question, setQuestion] = useState('')
  const [answer, setAnswer] = useState('')
  const [pickedId, setPickedId] = useState<string | null>(null)
  const [edited, setEdited] = useState('')
  const [useEdited, setUseEdited] = useState(false)
  const [formError, setFormError] = useState<string | null>(null)

  const invalidate = () => queryClient.invalidateQueries({ queryKey: ['annotations'] })

  const addManual = useMutation({
    mutationFn: () => adminApi.addAnnotation({ question: question.trim(), answer: answer.trim() }),
    onSuccess: () => { setQuestion(''); setAnswer(''); setFormError(null); invalidate() },
    onError: (e: Error) => setFormError(e.message),
  })
  const addCaptured = useMutation({
    mutationFn: () =>
      adminApi.annotationFromActivity(pickedId!, useEdited ? edited.trim() : undefined),
    onSuccess: () => { setPickedId(null); setEdited(''); setUseEdited(false); setFormError(null); invalidate() },
    onError: (e: Error) => setFormError(e.message),
  })

  const captured = (insights.data?.recent_activity ?? []).filter(
    (e) => e.kind === 'query',
  )
  const picked = captured.find((e) => e.id === pickedId)

  return (
    <div className="max-w-5xl mx-auto px-6 py-10">
      <PageHead
        title="Verified Answers"
        sub="Curate an answer once, and matching questions are served it verbatim — badged as verified. Annotations bind to the corpus: upload or remove a document and they pause as stale until re-created."
      />

      {data.isLoading && <Spinner />}
      {data.isError && <ErrorBanner message={(data.error as Error).message} />}

      {/* Promote a captured question */}
      <div className="rounded-lex border border-stone-200 bg-white p-5 mt-2">
        <h2 className="font-display text-lg tracking-tight flex items-center gap-2">
          <BadgeCheck size={17} className="text-[#2f6b2f]" /> Verify a captured answer
        </h2>
        <p className="text-[13px] text-stone-500 mt-1">
          Pick a real question a user asked, then serve either the captured answer as-is or your corrected version.
        </p>
        {captured.length === 0 ? (
          <p className="text-[14px] text-stone-400 mt-3">No captured questions yet.</p>
        ) : (
          <div className="mt-3 space-y-2 max-h-56 overflow-y-auto pr-1">
            {captured.map((e) => (
              <button
                key={e.id}
                onClick={() => {
                  setPickedId(e.id)
                  setEdited(e.result_excerpt)
                  setUseEdited(false)
                }}
                className={`w-full text-left rounded-lex border px-3 py-2 transition-colors ${
                  pickedId === e.id
                    ? 'border-ink bg-stone-50'
                    : 'border-stone-200 hover:border-stone-300'
                }`}
              >
                <p className="text-[14px] font-medium leading-snug">{e.title}</p>
                <p className="text-[12px] text-stone-400 mt-0.5">
                  {e.user_email} · {new Date(e.created_at).toLocaleString('en-IN')}
                </p>
              </button>
            ))}
          </div>
        )}
        {picked && (
          <div className="mt-4">
            <label className="text-[13px] font-medium text-stone-600">
              Corrected answer (excerpt shown — edit to serve your version instead)
            </label>
            <textarea
              value={edited}
              onChange={(e) => { setEdited(e.target.value); setUseEdited(true) }}
              rows={5}
              className="mt-1 w-full rounded-lex border border-stone-300 px-3 py-2 text-[14px] leading-relaxed focus:outline-none focus:border-ink"
            />
            <div className="flex items-center justify-between mt-2">
              <p className="text-[12px] text-stone-400">
                {useEdited
                  ? 'Will serve: your edited text above.'
                  : 'Will serve: the full captured answer, exactly as generated.'}
              </p>
              <button
                onClick={() => addCaptured.mutate()}
                disabled={addCaptured.isPending}
                className="rounded-lex bg-ink text-ivory px-4 py-2 text-[14px] font-medium disabled:opacity-50"
              >
                {addCaptured.isPending ? 'Verifying…' : 'Verify & serve'}
              </button>
            </div>
          </div>
        )}
      </div>

      {/* Manual annotation */}
      <div className="rounded-lex border border-stone-200 bg-white p-5 mt-4">
        <h2 className="font-display text-lg tracking-tight flex items-center gap-2">
          <Plus size={17} /> Write a verified answer
        </h2>
        <div className="mt-3 space-y-3">
          <input
            value={question}
            onChange={(e) => setQuestion(e.target.value)}
            placeholder="Question users ask, e.g. What does Section 86 of the Income Tax Act deal with?"
            className="w-full rounded-lex border border-stone-300 px-3 py-2 text-[14px] focus:outline-none focus:border-ink"
          />
          <textarea
            value={answer}
            onChange={(e) => setAnswer(e.target.value)}
            rows={4}
            placeholder="The verified answer, served verbatim…"
            className="w-full rounded-lex border border-stone-300 px-3 py-2 text-[14px] leading-relaxed focus:outline-none focus:border-ink"
          />
          <div className="flex justify-end">
            <button
              onClick={() => addManual.mutate()}
              disabled={!question.trim() || !answer.trim() || addManual.isPending}
              className="rounded-lex bg-ink text-ivory px-4 py-2 text-[14px] font-medium disabled:opacity-50"
            >
              {addManual.isPending ? 'Saving…' : 'Add verified answer'}
            </button>
          </div>
        </div>
      </div>

      {formError && <div className="mt-4"><ErrorBanner message={formError} /></div>}

      {/* The serving set */}
      <h2 className="font-display text-xl tracking-tight mt-8 mb-3">
        Serving set · {data.data?.annotations.length ?? 0}
      </h2>
      {data.data && data.data.annotations.length === 0 && (
        <p className="text-[14px] text-stone-400">Nothing verified yet — answers come from retrieval alone.</p>
      )}
      <div className="space-y-3">
        {data.data?.annotations.map((a) => (
          <AnnotationRow key={a.id} a={a} currentFp={data.data!.current_corpus_fp} />
        ))}
      </div>
    </div>
  )
}
