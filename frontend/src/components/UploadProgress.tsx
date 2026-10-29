import { useEffect, useState } from 'react'
import { RotateCcw, X } from 'lucide-react'

/**
 * Shared staged upload/indexing progress (Piece 10, extracted in
 * Piece 17 so every ingest surface renders the same feedback — no
 * per-page copies). One item = one document being added:
 *
 *   queued → uploading (byte progress where the transport provides
 *   it) → indexing (bytes sent; the server is parsing, chunking and
 *   embedding) → ready / failed.
 *
 * "Ready" is only ever set when the server's ingest response has
 * landed — never when the send merely finished. A failure carries
 * the server's actual error message and offers Retry / Dismiss
 * instead of an endless "Indexing…" label.
 */
export type ProgressStatus =
  | 'queued' | 'uploading' | 'indexing' | 'ready' | 'failed' | 'skipped'

export interface ProgressItemView {
  key: number
  name: string
  status: ProgressStatus
  /** Bytes-sent fraction (0..1) during 'uploading'; null when the
   *  transport cannot measure it (indeterminate bar). */
  progress?: number | null
  /** Server error message when status is 'failed'; skip reason when
   *  status is 'skipped'. */
  error?: string
  /** e.g. "12 chunks indexed" / "selected". */
  note?: string
  /** When processing of this item started (ms epoch); drives the
   *  elapsed readout while the item is pending. */
  startedAt?: number
}

export function isPendingStatus(s: ProgressStatus): boolean {
  return s === 'queued' || s === 'uploading' || s === 'indexing'
}

export function formatElapsed(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000))
  if (s < 60) return `${s}s`
  return `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, '0')}s`
}

/** The accept set shared by every vault-ingest picker — the file
 *  pickers feed the one vault ingest pipeline, not separate paths. */
export const UPLOAD_ACCEPT = '.pdf,.txt,.md,.doc,.docx'

/** Per-batch upload intake caps: at most 20 files and at most 10
 *  folders in one batch (one picker/drop action). Items beyond a cap
 *  stay in the batch list as 'skipped' with the reason shown — never
 *  silently dropped. (Moved here from InteractPage in Piece 18 so
 *  every upload surface applies the SAME caps — no divergent copies.) */
export const MAX_BATCH_FILES = 20
export const MAX_BATCH_FOLDERS = 10

/** Top-level folder a batch item belongs to ('' for loose files). */
export function topFolderOf(relPath: string): string {
  return relPath.includes('/') ? relPath.split('/')[0] : ''
}

/** Apply the per-batch caps in arrival order: the first 20 files
 *  from the first 10 distinct folders are accepted; everything else
 *  is marked skipped with the cap that stopped it. */
export function applyBatchCaps(
  files: { file: File; relPath: string }[],
): { file: File; relPath: string; skipReason?: string }[] {
  const folders = new Set<string>()
  let accepted = 0
  return files.map(({ file, relPath }) => {
    const folder = topFolderOf(relPath)
    if (folder && !folders.has(folder) && folders.size >= MAX_BATCH_FOLDERS) {
      return { file, relPath, skipReason: `batch limit of ${MAX_BATCH_FOLDERS} folders reached` }
    }
    if (accepted >= MAX_BATCH_FILES) {
      return { file, relPath, skipReason: `batch limit of ${MAX_BATCH_FILES} files reached` }
    }
    if (folder) folders.add(folder)
    accepted += 1
    return { file, relPath }
  })
}

/** "Folder: Smith-case (12 files)" when the batch came from a folder
 *  (any relative path with a directory part), else "N files". */
export function batchLabelFor(items: { relPath: string }[]): string {
  const foldered = items.find((i) => i.relPath.includes('/'))
  if (foldered) return `Folder: ${foldered.relPath.split('/')[0]} (${items.length} files)`
  return `${items.length} file${items.length === 1 ? '' : 's'}`
}

/** Flatten a drop into files, traversing dropped folders via the
 *  FileSystem entries API (readEntries pages at ~100 entries, so it
 *  is drained in a loop). */
export async function filesFromDataTransfer(dt: DataTransfer): Promise<{ file: File; relPath: string }[]> {
  const out: { file: File; relPath: string }[] = []
  const entries = Array.from(dt.items)
    .filter((i) => i.kind === 'file')
    .map((i) => i.webkitGetAsEntry())
    .filter((e): e is FileSystemEntry => e !== null)
  if (entries.length === 0) {
    return Array.from(dt.files).map((file) => ({ file, relPath: '' }))
  }
  async function walk(entry: FileSystemEntry, path: string): Promise<void> {
    if (entry.isFile) {
      const file = await new Promise<File>((res, rej) => (entry as FileSystemFileEntry).file(res, rej))
      out.push({ file, relPath: path })
    } else if (entry.isDirectory) {
      const reader = (entry as FileSystemDirectoryEntry).createReader()
      for (;;) {
        const children = await new Promise<FileSystemEntry[]>((res, rej) => reader.readEntries(res, rej))
        if (children.length === 0) break
        for (const child of children) await walk(child, `${path}/${child.name}`)
      }
    }
  }
  for (const entry of entries) await walk(entry, entry.name)
  return out
}

/** A clock that ticks while `active`, for elapsed readouts. */
export function useNow(active: boolean, intervalMs = 500): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!active) return
    setNow(Date.now())
    const id = setInterval(() => setNow(Date.now()), intervalMs)
    return () => clearInterval(id)
  }, [active, intervalMs])
  return now
}

export function UploadProgressList({
  items,
  headerLabel,
  onClear,
  onRetry,
  onDismiss,
  className,
}: {
  items: ProgressItemView[]
  /** Batch title (e.g. "Folder: Smith-case (12 files)") — when set,
   *  a batch summary bar is shown above the rows. */
  headerLabel?: string
  onClear?: () => void
  onRetry?: (key: number) => void
  onDismiss?: (key: number) => void
  className?: string
}) {
  const anyPending = items.some((i) => isPendingStatus(i.status))
  const now = useNow(anyPending)
  if (items.length === 0) return null
  const finished = items.filter(
    (u) => u.status === 'ready' || u.status === 'failed' || u.status === 'skipped',
  ).length
  const ready = items.filter((u) => u.status === 'ready').length
  return (
    <div className={`rounded-lex border border-stone-200 px-3 py-2.5 ${className ?? 'mt-3'}`}>
      {headerLabel != null && (
        <>
          <div className="flex items-baseline justify-between gap-3">
            <p className="text-[13px] font-medium">{headerLabel}</p>
            {onClear && (
              <button
                onClick={onClear}
                className="text-[12px] text-stone-400 hover:text-stone-600"
              >
                Clear list
              </button>
            )}
          </div>
          <div className="mt-2">
            <div className="flex items-baseline justify-between gap-3">
              <span className="text-[12px] text-stone-500">
                {finished} of {items.length} files done · {ready} ready
              </span>
              <span className="text-[12px] text-stone-400">
                {Math.round((finished / items.length) * 100)}%
              </span>
            </div>
            <div className="h-1.5 mt-1 rounded-full bg-stone-200 overflow-hidden">
              <div
                className="h-full bg-stone-700 transition-all duration-300"
                style={{ width: `${(finished / items.length) * 100}%` }}
              />
            </div>
          </div>
        </>
      )}
      <ul className={headerLabel != null ? 'mt-2 space-y-2' : 'space-y-2'}>
        {items.map((u) => {
          const elapsed =
            isPendingStatus(u.status) && u.startedAt != null
              ? ` · ${formatElapsed(now - u.startedAt)}`
              : ''
          const terminal = u.status === 'ready' || u.status === 'failed' || u.status === 'skipped'
          return (
            <li key={u.key} className="text-[13px]">
              <div className="flex items-center gap-2">
                <span className="flex-1 truncate">{u.name}</span>
                {u.status === 'queued' && <span className="text-stone-400">Queued{elapsed}</span>}
                {u.status === 'uploading' && (
                  <span className="text-stone-500">
                    Uploading…{u.progress != null ? ` ${Math.round(u.progress * 100)}%` : ''}{elapsed}
                  </span>
                )}
                {u.status === 'indexing' && <span className="text-stone-500">Indexing…{elapsed}</span>}
                {u.status === 'ready' && (
                  <span className="text-emerald-700">Ready{u.note ? ` · ${u.note}` : ''}</span>
                )}
                {u.status === 'failed' && (
                  <span className="text-[#a02c2c]">Failed: {u.error}</span>
                )}
                {u.status === 'skipped' && (
                  <span className="text-amber-700">Skipped — {u.error}</span>
                )}
                {u.status === 'failed' && onRetry && (
                  <button
                    onClick={() => onRetry(u.key)}
                    className="flex items-center gap-1 text-[12px] font-medium text-stone-500 hover:text-ink"
                    title="Retry this upload"
                  >
                    <RotateCcw size={12} /> Retry
                  </button>
                )}
                {terminal && onDismiss && (
                  <button
                    onClick={() => onDismiss(u.key)}
                    className="text-stone-400 hover:text-ink p-0.5"
                    title="Dismiss"
                  >
                    <X size={13} />
                  </button>
                )}
              </div>
              {(u.status === 'uploading' || u.status === 'indexing' || u.status === 'ready') && (
                <div className="h-1 mt-1 rounded-full bg-stone-200 overflow-hidden">
                  <div
                    className={`h-full transition-all duration-300 ${
                      u.status === 'ready'
                        ? 'bg-emerald-600'
                        : u.status === 'indexing'
                          ? 'bg-stone-400 animate-pulse'
                          : 'bg-stone-500'
                    } ${u.status === 'uploading' && u.progress == null ? 'animate-pulse' : ''}`}
                    style={{
                      width:
                        u.status === 'uploading' && u.progress != null
                          ? `${Math.round(u.progress * 100)}%`
                          : '100%',
                    }}
                  />
                </div>
              )}
            </li>
          )
        })}
      </ul>
    </div>
  )
}
