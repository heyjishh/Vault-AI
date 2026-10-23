import { AnimatePresence, motion } from 'framer-motion'
import { CheckCircle2, Info, X, XCircle } from 'lucide-react'
import { useSession } from '../stores/session'

export default function Toaster() {
  const toasts = useSession((s) => s.toasts)
  const dismiss = useSession((s) => s.dismissToast)
  return (
    <div className="fixed bottom-5 right-5 z-50 flex flex-col gap-2 items-end">
      <AnimatePresence>
        {toasts.map((t) => (
          <motion.div
            key={t.id}
            initial={{ opacity: 0, y: 10, scale: 0.97 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 6, scale: 0.97 }}
            transition={{ duration: 0.22 }}
            className="flex items-center gap-2.5 rounded-lex-lg bg-ink text-ivory pl-3.5 pr-2.5 py-2.5 shadow-lex-md text-[13px] max-w-sm"
          >
            {t.kind === 'success' && <CheckCircle2 size={15} className="text-green-400 flex-shrink-0" />}
            {t.kind === 'error' && <XCircle size={15} className="text-red-400 flex-shrink-0" />}
            {t.kind === 'info' && <Info size={15} className="text-stone-300 flex-shrink-0" />}
            <span className="leading-snug">{t.message}</span>
            <button onClick={() => dismiss(t.id)} className="p-1 rounded hover:bg-ivory/15 flex-shrink-0">
              <X size={13} />
            </button>
          </motion.div>
        ))}
      </AnimatePresence>
    </div>
  )
}
