import { useEffect, useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { adminApi } from '../api'
import { ErrorBanner, PageHead, Spinner, StatCard } from '../ui'

function inr(n: number): string {
  return '₹' + n.toLocaleString('en-IN')
}

function fmtDate(iso: string): string {
  return new Date(iso).toLocaleDateString('en-IN', { day: 'numeric', month: 'short', year: 'numeric' })
}

export default function BillingPage() {
  const queryClient = useQueryClient()
  const overview = useQuery({ queryKey: ['billing-overview'], queryFn: adminApi.billingOverview })
  const settings = useQuery({ queryKey: ['billing-settings'], queryFn: adminApi.billingSettings })

  const [provider, setProvider] = useState('mock')
  const [baseUrl, setBaseUrl] = useState('')
  const [merchantId, setMerchantId] = useState('')
  const [secret, setSecret] = useState('')
  const [saved, setSaved] = useState(false)

  useEffect(() => {
    if (settings.data) {
      setProvider(settings.data.provider)
      setBaseUrl(settings.data.youpi_base_url)
      setMerchantId(settings.data.youpi_merchant_id)
    }
  }, [settings.data])

  const save = useMutation({
    mutationFn: () =>
      adminApi.updateBillingSettings({
        provider,
        youpi_base_url: baseUrl,
        youpi_merchant_id: merchantId,
        youpi_webhook_secret: secret || undefined,
      }),
    onSuccess: () => {
      setSaved(true)
      setSecret('')
      queryClient.invalidateQueries({ queryKey: ['billing-settings'] })
      setTimeout(() => setSaved(false), 3000)
    },
  })

  if (overview.isLoading) return <Spinner label="Loading billing…" />
  if (overview.isError || !overview.data) return <ErrorBanner message="Could not load billing data." />
  const o = overview.data
  const webhookUrl = `${window.location.origin}/api/v1/billing/webhook/youpi`

  return (
    <div className="max-w-5xl mx-auto px-6 py-10">
      <PageHead
        title="Billing & Subscriptions"
        sub="Token plans, UPI payments via your own gateway, and per-user spend — money settles straight to the firm's bank."
      />

      <div className="grid grid-cols-2 xl:grid-cols-4 gap-4 mt-6">
        <StatCard label="Total users" value={o.total_users} />
        <StatCard label="Active subscriptions" value={o.active_subscriptions} sub="paid plans in a live period" />
        <StatCard label="MRR" value={inr(o.mrr_inr)} sub="monthly recurring revenue" />
        <StatCard label="Collected" value={inr(o.collected_inr)} sub={`${o.payments_count} successful payments`} />
      </div>

      {/* Per-user spend */}
      <h2 className="font-display text-xl mt-9 mb-3">Users &amp; spend</h2>
      <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex overflow-hidden">
        <table className="w-full text-[13.5px]">
          <thead>
            <tr className="text-left text-meta uppercase tracking-wider text-stone-500 border-b border-stone-200">
              <th className="px-5 py-3 font-medium">User</th>
              <th className="px-5 py-3 font-medium">Plan</th>
              <th className="px-5 py-3 font-medium w-[26%]">Tokens this period</th>
              <th className="px-5 py-3 font-medium">Period ends</th>
              <th className="px-5 py-3 font-medium">Total spent</th>
            </tr>
          </thead>
          <tbody>
            {o.users.map((u) => {
              const pct = Math.min(100, Math.round((u.tokens_used / Math.max(1, u.token_quota)) * 100))
              return (
                <tr key={u.user_id} className="border-b border-stone-100 last:border-0">
                  <td className="px-5 py-3">
                    <span className="font-medium">{u.email}</span>
                    {u.role === 'admin' && <span className="ml-2 rounded-full bg-ink text-ivory px-2 py-0.5 text-[11px]">admin</span>}
                  </td>
                  <td className="px-5 py-3">
                    <span className={`rounded-full px-2 py-0.5 text-[12px] ${u.plan_id === 'premium' ? 'bg-[#efe6fb] text-[#5b3a8e]' : u.plan_id === 'basic' ? 'bg-[#e3ecfb] text-[#2b4a7f]' : 'bg-stone-100 text-stone-600'}`}>
                      {u.plan_name}
                    </span>
                  </td>
                  <td className="px-5 py-3">
                    <div className="flex items-center gap-2">
                      <div className="h-2 flex-1 rounded-full bg-stone-100 overflow-hidden">
                        <div className={`h-full ${pct >= 100 ? 'bg-[#c2410c]' : 'bg-ink'}`} style={{ width: `${pct}%` }} />
                      </div>
                      <span className="text-stone-500 whitespace-nowrap">
                        {u.tokens_used.toLocaleString('en-IN')} / {u.token_quota.toLocaleString('en-IN')}
                      </span>
                    </div>
                  </td>
                  <td className="px-5 py-3 text-stone-600">{fmtDate(u.period_end)}</td>
                  <td className="px-5 py-3 font-medium">{inr(u.spent_inr)}</td>
                </tr>
              )
            })}
          </tbody>
        </table>
      </div>

      {/* Recent payments */}
      <h2 className="font-display text-xl mt-9 mb-3">Recent payments</h2>
      {o.recent_payments.length === 0 ? (
        <p className="text-stone-500 text-[13.5px]">No payments yet. They appear here the moment a user subscribes.</p>
      ) : (
        <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex overflow-hidden">
          <table className="w-full text-[13.5px]">
            <thead>
              <tr className="text-left text-meta uppercase tracking-wider text-stone-500 border-b border-stone-200">
                <th className="px-5 py-3 font-medium">When</th>
                <th className="px-5 py-3 font-medium">User</th>
                <th className="px-5 py-3 font-medium">Order</th>
                <th className="px-5 py-3 font-medium">Plan</th>
                <th className="px-5 py-3 font-medium">Amount</th>
                <th className="px-5 py-3 font-medium">Status</th>
                <th className="px-5 py-3 font-medium">UTR</th>
              </tr>
            </thead>
            <tbody>
              {o.recent_payments.map((p) => (
                <tr key={p.id} className="border-b border-stone-100 last:border-0">
                  <td className="px-5 py-3 text-stone-600">{fmtDate(p.created_at)}</td>
                  <td className="px-5 py-3">{p.user_email}</td>
                  <td className="px-5 py-3 text-stone-500">{p.order_id}</td>
                  <td className="px-5 py-3 capitalize">{p.plan_id}</td>
                  <td className="px-5 py-3">{inr(p.amount_inr)}</td>
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

      {/* Payment gateway settings */}
      <h2 className="font-display text-xl mt-9 mb-3">Payment gateway — connect your bank</h2>
      <div className="rounded-lex-lg border border-stone-200 bg-white shadow-lex px-6 py-5 max-w-3xl">
        <p className="text-[13.5px] text-stone-600">
          Subscriptions are collected through <strong>YouPI-PG</strong>, a self-hosted UPI gateway you run yourself:
          users pay by UPI and the money settles <strong>directly into the bank account linked to your UPI ID</strong> —
          no aggregator, no percentage fee. Point Juris Vault AI at your gateway below.
        </p>
        <div className="grid gap-4 mt-5">
          <label className="block">
            <span className="text-meta uppercase tracking-wider text-stone-500 font-medium">Provider</span>
            <select
              value={provider}
              onChange={(e) => setProvider(e.target.value)}
              className="mt-1.5 w-full rounded-lex border border-stone-300 px-3 py-2 text-[14px] bg-white"
            >
              <option value="mock">Test mode (simulated payments)</option>
              <option value="youpi">YouPI-PG — live UPI gateway</option>
            </select>
          </label>
          <label className="block">
            <span className="text-meta uppercase tracking-wider text-stone-500 font-medium">YouPI-PG base URL</span>
            <input
              value={baseUrl}
              onChange={(e) => setBaseUrl(e.target.value)}
              placeholder="https://pay.yourfirm.in"
              className="mt-1.5 w-full rounded-lex border border-stone-300 px-3 py-2 text-[14px]"
            />
          </label>
          <label className="block">
            <span className="text-meta uppercase tracking-wider text-stone-500 font-medium">Merchant ID (optional)</span>
            <input
              value={merchantId}
              onChange={(e) => setMerchantId(e.target.value)}
              placeholder="00000000-0000-0000-0000-000000000001"
              className="mt-1.5 w-full rounded-lex border border-stone-300 px-3 py-2 text-[14px]"
            />
          </label>
          <label className="block">
            <span className="text-meta uppercase tracking-wider text-stone-500 font-medium">
              Webhook secret {settings.data?.webhook_secret_set ? '(configured — leave blank to keep)' : '(not set)'}
            </span>
            <input
              type="password"
              value={secret}
              onChange={(e) => setSecret(e.target.value)}
              placeholder="Webhook secret from your YouPI-PG dashboard"
              className="mt-1.5 w-full rounded-lex border border-stone-300 px-3 py-2 text-[14px]"
            />
          </label>
          <div className="rounded-lex bg-ivory border border-stone-200 px-4 py-3 text-[13px] text-stone-600">
            In YouPI-PG, set the merchant webhook URL to:
            <code className="block mt-1 text-[12.5px] bg-white border border-stone-200 rounded px-2 py-1.5 break-all">{webhookUrl}</code>
            Payment confirmations arrive there (HMAC-signed) and activate subscriptions instantly; this app also polls the gateway as a fallback.
          </div>
          {save.isError && <ErrorBanner message={(save.error as Error).message} />}
          <div className="flex items-center gap-3">
            <button
              onClick={() => save.mutate()}
              disabled={save.isPending}
              className="rounded-lex bg-ink text-ivory px-5 py-2.5 text-[13.5px] font-medium disabled:opacity-50"
            >
              {save.isPending ? 'Saving…' : 'Save payment settings'}
            </button>
            {saved && <span className="text-[13px] text-[#2f6b2f]">Saved.</span>}
          </div>
          <p className="text-meta text-stone-400">
            Note: YouPI-PG describes itself as an experimental, self-hosted research gateway rather than a certified
            payment aggregator — evaluate it under your own compliance review before taking high-volume live payments,
            and always reconcile against bank records.
          </p>
        </div>
      </div>
    </div>
  )
}
