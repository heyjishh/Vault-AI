import { useEffect, useRef, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { BadgeIndianRupee, Check, CreditCard, ExternalLink, Loader2 } from 'lucide-react'
import { billingApi, type MyBilling, type Plan } from '../api/client'
import { useSession } from '../stores/session'
import { EmptyState, Pager, Spinner, usePagination } from './ui'

function inr(n: number): string {
  return '₹' + n.toLocaleString('en-IN')
}

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(n % 1_000_000 === 0 ? 0 : 1) + 'M'
  if (n >= 1_000) return (n / 1_000).toFixed(0) + 'k'
  return String(n)
}

function fmtDate(iso: string): string {
  return new Date(iso).toLocaleDateString('en-IN', { day: 'numeric', month: 'short', year: 'numeric' })
}

export default function PlansPage() {
  const toast = useSession((s) => s.toast)
  const queryClient = useQueryClient()
  const me = useQuery({ queryKey: ['billing-me'], queryFn: billingApi.me })
  const pagedPayments = usePagination(me.data?.payments ?? [], 5)
  const plans = useQuery({ queryKey: ['billing-plans'], queryFn: billingApi.plans })
  const [pendingMock, setPendingMock] = useState<{ paymentId: string; plan: Plan } | null>(null)
  const [waiting, setWaiting] = useState<string | null>(null)
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null)

  useEffect(() => () => { if (pollRef.current) clearInterval(pollRef.current) }, [])

  const stopPolling = () => {
    if (pollRef.current) clearInterval(pollRef.current)
    pollRef.current = null
    setWaiting(null)
  }

  const startPolling = (paymentId: string) => {
    setWaiting(paymentId)
    let tries = 0
    pollRef.current = setInterval(async () => {
      tries += 1
      try {
        const p = await billingApi.paymentStatus(paymentId)
        if (p.status === 'success') {
          stopPolling()
          toast('success', 'Payment confirmed — your plan is active.')
          queryClient.invalidateQueries({ queryKey: ['billing-me'] })
        } else if (p.status === 'failed' || p.status === 'expired' || p.status === 'cancelled') {
          stopPolling()
          toast('error', `Payment ${p.status}. You can try again.`)
        } else if (tries > 60) {
          stopPolling()
          toast('error', 'Still waiting on the gateway — check back in a moment; status refreshes automatically.')
        }
      } catch { /* keep polling */ }
    }, 3000)
  }

  const buy = useMutation({
    mutationFn: (plan: Plan) => billingApi.checkout(plan.id),
    onSuccess: (res, plan) => {
      if (res.mode === 'redirect' && res.checkout_url) {
        window.open(res.checkout_url, '_blank', 'noopener')
        toast('success', 'Complete the UPI payment in the gateway tab — this page updates automatically.')
        startPolling(res.payment_id)
      } else {
        setPendingMock({ paymentId: res.payment_id, plan })
      }
    },
    onError: (e) => toast('error', (e as Error).message),
  })

  const confirmMock = useMutation({
    mutationFn: (paymentId: string) => billingApi.completeMock(paymentId),
    onSuccess: () => {
      setPendingMock(null)
      toast('success', 'Test payment complete — your plan is active.')
      queryClient.invalidateQueries({ queryKey: ['billing-me'] })
    },
    onError: (e) => toast('error', (e as Error).message),
  })

  if (me.isLoading || plans.isLoading) return <Spinner label="Loading plans…" />
  if (me.isError || !me.data) return <EmptyState title="Billing unavailable" hint="Could not load your plan. Try again shortly." />

  const b: MyBilling = me.data
  const pct = Math.min(100, Math.round((b.tokens_used / Math.max(1, b.token_quota)) * 100))
  const exhausted = b.tokens_used >= b.token_quota

  return (
    <div className="max-w-5xl mx-auto px-6 py-8">
      <h1 className="font-display text-[2rem] tracking-tight">Plans &amp; billing</h1>
      <p className="text-stone-500 mt-1.5 text-[15px]">
        Token-based plans. Every question, research run, workflow and document ingest draws from your monthly token pool.
      </p>

      {/* Current plan + usage */}
      <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-6 py-5 mt-6">
        <div className="flex flex-wrap items-baseline justify-between gap-3">
          <div>
            <p className="text-meta uppercase tracking-wider text-stone-500 font-medium">Current plan</p>
            <p className="font-display text-2xl mt-1">
              {b.plan.name}
              <span className="text-stone-400 text-base ml-2">
                {b.plan.price_inr === 0 ? 'free forever' : `${inr(b.plan.price_inr)} / month`}
              </span>
            </p>
          </div>
          <p className="text-[13px] text-stone-500">
            Period ends {fmtDate(b.subscription.period_end)}
          </p>
        </div>
        <div className="mt-4">
          <div className="flex justify-between text-[13px]">
            <span className={exhausted ? 'text-[#a02c2c] font-medium' : 'text-stone-600'}>
              {b.tokens_used.toLocaleString('en-IN')} tokens used
            </span>
            <span className="text-stone-500">of {b.token_quota.toLocaleString('en-IN')} ({pct}%)</span>
          </div>
          <div className="h-2.5 rounded-full bg-stone-100 mt-1.5 overflow-hidden">
            <div
              className={`h-full rounded-full ${exhausted ? 'bg-[#c2410c]' : 'bg-ink'}`}
              style={{ width: `${pct}%` }}
            />
          </div>
          <p className="text-meta text-stone-400 mt-2">
            LLM in {b.llm_prompt_tokens.toLocaleString('en-IN')} · LLM out {b.llm_completion_tokens.toLocaleString('en-IN')} · Embeddings {b.embedding_tokens.toLocaleString('en-IN')}
            {exhausted && ' · Quota exhausted — upgrade to keep asking questions.'}
          </p>
        </div>
      </div>

      {/* Test-payment confirmation (mock provider) */}
      {pendingMock && (
        <div className="rounded-lex-lg border border-ink bg-ivory px-6 py-5 mt-5 flex flex-wrap items-center gap-4 justify-between">
          <div>
            <p className="font-medium flex items-center gap-2">
              <CreditCard size={16} /> Test payment — {pendingMock.plan.name} plan
            </p>
            <p className="text-[13px] text-stone-500 mt-0.5">
              The gateway is in test mode. Confirm to simulate paying {inr(pendingMock.plan.price_inr)} and activate the plan.
            </p>
          </div>
          <div className="flex gap-2">
            <button
              className="rounded-lex bg-ink text-ivory px-4 py-2 text-[13px] font-medium disabled:opacity-50"
              disabled={confirmMock.isPending}
              onClick={() => confirmMock.mutate(pendingMock.paymentId)}
            >
              {confirmMock.isPending ? 'Processing…' : `Pay ${inr(pendingMock.plan.price_inr)} (test)`}
            </button>
            <button className="rounded-lex border border-stone-300 px-4 py-2 text-[13px]" onClick={() => setPendingMock(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}

      {waiting && (
        <div className="rounded-lex border border-stone-200 bg-white px-5 py-3.5 mt-5 flex items-center gap-3 text-[13.5px]">
          <Loader2 size={16} className="animate-spin" />
          Waiting for the UPI gateway to confirm your payment…
          <button className="ml-auto text-stone-500 underline" onClick={stopPolling}>Stop waiting</button>
        </div>
      )}

      {/* Plan cards */}
      <div className="grid md:grid-cols-3 gap-4 mt-6">
        {(plans.data ?? []).map((plan) => {
          const current = plan.id === b.plan.id
          return (
            <div
              key={plan.id}
              className={`rounded-lex-lg border bg-white shadow-lex px-6 py-5 flex flex-col ${current ? 'border-ink ring-1 ring-ink' : 'border-stone-200'}`}
            >
              <p className="font-display text-xl">{plan.name}</p>
              <p className="mt-2">
                <span className="font-display text-3xl">{plan.price_inr === 0 ? '₹0' : inr(plan.price_inr)}</span>
                <span className="text-stone-500 text-[13px]"> / month</span>
              </p>
              <p className="text-[13px] text-stone-500 mt-1 flex items-center gap-1.5">
                <BadgeIndianRupee size={14} className="text-stone-400" />
                {fmtTokens(plan.monthly_tokens)} tokens included
              </p>
              <ul className="mt-4 space-y-2 text-[13.5px] flex-1">
                {plan.features.map((f) => (
                  <li key={f} className="flex gap-2">
                    <Check size={15} className="text-stone-400 mt-0.5 shrink-0" />
                    <span>{f}</span>
                  </li>
                ))}
              </ul>
              <button
                disabled={current || buy.isPending || plan.price_inr === 0}
                onClick={() => buy.mutate(plan)}
                className="mt-5 rounded-lex bg-ink text-ivory px-4 py-2.5 text-[13.5px] font-medium disabled:opacity-40 flex items-center justify-center gap-2"
              >
                {current ? 'Current plan' : plan.price_inr === 0 ? 'Included' : (
                  <>Subscribe <ExternalLink size={13} /></>
                )}
              </button>
            </div>
          )
        })}
      </div>

      {/* Payment history */}
      <h2 className="font-display text-xl mt-10 mb-3">Payment history</h2>
      {b.payments.length === 0 ? (
        <EmptyState title="No payments yet" hint="Your subscription payments will appear here." />
      ) : (
        <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex overflow-hidden">
          <table className="w-full text-[13.5px]">
            <thead>
              <tr className="text-left text-meta uppercase tracking-wider text-stone-500 border-b border-stone-200">
                <th className="px-5 py-3 font-medium">Date</th>
                <th className="px-5 py-3 font-medium">Plan</th>
                <th className="px-5 py-3 font-medium">Amount</th>
                <th className="px-5 py-3 font-medium">Via</th>
                <th className="px-5 py-3 font-medium">Status</th>
                <th className="px-5 py-3 font-medium">UTR</th>
              </tr>
            </thead>
            <tbody>
              {pagedPayments.pageItems.map((p) => (
                <tr key={p.id} className="border-b border-stone-100 last:border-0">
                  <td className="px-5 py-3">{fmtDate(p.created_at)}</td>
                  <td className="px-5 py-3 capitalize">{p.plan_id}</td>
                  <td className="px-5 py-3">{inr(p.amount_inr)}</td>
                  <td className="px-5 py-3">{p.provider === 'youpi' ? 'UPI (YouPI-PG)' : 'Test mode'}</td>
                  <td className="px-5 py-3">
                    <span className={`rounded-full px-2 py-0.5 text-[12px] ${p.status === 'success' ? 'bg-[#e7f4e4] text-[#2f6b2f]' : p.status === 'initiated' ? 'bg-[#fdf3d7] text-[#8a6d1a]' : 'bg-[#feeaea] text-[#a02c2c]'}`}>
                      {p.status}
                    </span>
                  </td>
                  <td className="px-5 py-3 text-stone-500">{p.utr ?? '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {b.payments.length > 0 && (
        <Pager page={pagedPayments.page} pages={pagedPayments.pages} from={pagedPayments.from} to={pagedPayments.to} total={pagedPayments.total} onPage={pagedPayments.setPage} />
      )}
      <p className="text-meta text-stone-400 mt-4">
        Payments are collected by the workspace’s own UPI gateway — money settles directly to the firm’s bank account, with no aggregator fee.
      </p>
    </div>
  )
}
