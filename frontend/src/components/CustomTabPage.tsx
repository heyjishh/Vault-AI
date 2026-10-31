import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import type { UiTab } from '../api/types'

/** Renders an admin-defined sidebar page (markdown content). */
export default function CustomTabPage({ tab }: { tab: UiTab }) {
  return (
    <div className="w-full px-6 py-10">
      <h1 className="font-display text-[2rem] tracking-tight mb-6">{tab.label}</h1>
      <div className="text-[15px] leading-relaxed text-stone-700 dark:text-stone-200 space-y-4 [&_h1]:font-display [&_h1]:text-2xl [&_h1]:text-ink dark:[&_h1]:text-[#f4f2ee] [&_h2]:font-display [&_h2]:text-xl [&_h2]:text-ink dark:[&_h2]:text-[#f4f2ee] [&_h3]:font-semibold [&_h3]:text-ink dark:[&_h3]:text-[#f4f2ee] [&_a]:underline [&_a]:text-ink dark:[&_a]:text-[#f4f2ee] [&_ul]:list-disc [&_ul]:pl-5 [&_ol]:list-decimal [&_ol]:pl-5 [&_strong]:text-ink dark:[&_strong]:text-[#f4f2ee] [&_code]:bg-stone-100 dark:[&_code]:bg-white/10 [&_code]:px-1 [&_code]:rounded">
        <ReactMarkdown remarkPlugins={[remarkGfm]}>{tab.content ?? ''}</ReactMarkdown>
      </div>
    </div>
  )
}
