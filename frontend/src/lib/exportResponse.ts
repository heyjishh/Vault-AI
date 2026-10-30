/**
 * Piece 28: export one Q&A response as a real PDF or an RTF file —
 * dependency-free, generated entirely in the browser.
 *
 * Both documents share one shape:
 *   Juris Vault AI header (plus an optional context title)
 *   Question — the exact user question
 *   Response — the answer, keeping its markdown structure
 *              (headings, paragraphs, bullet/numbered lists, tables)
 *   Sources  — the cited documents, when any
 *   Footer   — product line + generation date
 *
 * A single markdown → blocks parser feeds both writers, so the two
 * formats never drift apart in content.
 */

export interface ExportCitation {
  /** Citation label as shown in the UI, e.g. "doc:1". */
  label: string
  title: string
}

export interface ExportInput {
  question: string
  /** Optional context line (Interact action + document titles). */
  title?: string
  answerMarkdown: string
  citations?: ExportCitation[]
  generatedAt?: Date
}

// ---------------------------------------------------------------- blocks

export type Block =
  | { kind: 'heading'; level: number; text: string }
  | { kind: 'paragraph'; text: string }
  | { kind: 'bullet'; text: string }
  | { kind: 'numbered'; text: string; n: number }
  | { kind: 'tableRow'; cells: string[] }
  | { kind: 'code'; text: string }
  | { kind: 'rule' }

/** Structural parse of GitHub-flavoured markdown into blocks. Inline
 *  markup (bold/code/links) stays in the text; each writer renders or
 *  strips it as its format allows. */
export function parseMarkdown(md: string): Block[] {
  const blocks: Block[] = []
  const lines = (md ?? '').replace(/\r\n?/g, '\n').split('\n')
  let para: string[] = []
  let inCode = false
  const flushPara = () => {
    if (para.length > 0) {
      blocks.push({ kind: 'paragraph', text: para.join(' ') })
      para = []
    }
  }
  for (const raw of lines) {
    const line = raw
    if (/^\s*```/.test(line)) {
      flushPara()
      inCode = !inCode
      continue
    }
    if (inCode) {
      blocks.push({ kind: 'code', text: line })
      continue
    }
    const trimmed = line.trim()
    if (!trimmed) {
      flushPara()
      continue
    }
    const heading = /^(#{1,6})\s+(.*)$/.exec(trimmed)
    if (heading) {
      flushPara()
      blocks.push({ kind: 'heading', level: heading[1].length, text: heading[2].trim() })
      continue
    }
    if (/^([-*_]\s*){3,}$/.test(trimmed)) {
      flushPara()
      blocks.push({ kind: 'rule' })
      continue
    }
    // Table rows; the |---|---| separator row carries no content.
    if (trimmed.startsWith('|')) {
      flushPara()
      const cells = trimmed.replace(/^\||\|$/g, '').split('|').map((c) => c.trim())
      if (cells.every((c) => /^:?-{2,}:?$/.test(c))) continue
      blocks.push({ kind: 'tableRow', cells })
      continue
    }
    const bullet = /^[-*+]\s+(.*)$/.exec(trimmed)
    if (bullet) {
      flushPara()
      blocks.push({ kind: 'bullet', text: bullet[1].trim() })
      continue
    }
    const numbered = /^(\d{1,3})[.)]\s+(.*)$/.exec(trimmed)
    if (numbered) {
      flushPara()
      blocks.push({ kind: 'numbered', n: Number(numbered[1]), text: numbered[2].trim() })
      continue
    }
    const quote = /^>\s?(.*)$/.exec(trimmed)
    para.push((quote ? quote[1] : trimmed).trim())
  }
  flushPara()
  return blocks
}

// ---------------------------------------------------------------- inline

export interface InlineRun {
  text: string
  bold: boolean
}

/** Split inline markdown into plain / bold runs: **bold** and __bold__
 *  become bold runs; *italic*, `code`, links and images lose their
 *  markers (link targets survive as text) so no markdown punctuation
 *  ever leaks into the exported document. */
export function inlineRuns(text: string): InlineRun[] {
  const runs: InlineRun[] = []
  const push = (t: string, bold: boolean) => {
    if (!t) return
    const last = runs[runs.length - 1]
    if (last && last.bold === bold) last.text += t
    else runs.push({ text: t, bold })
  }
  const pattern = /(\*\*|__)([\s\S]+?)\1|\*([^*\n]+)\*|_([^_\n]+)_|`([^`\n]+)`|!\[([^\]]*)\]\([^)]*\)|\[([^\]]+)\]\(([^)]*)\)/g
  let last = 0
  let m: RegExpExecArray | null
  while ((m = pattern.exec(text)) !== null) {
    push(text.slice(last, m.index), false)
    if (m[2] !== undefined) push(m[2], true)
    else if (m[3] !== undefined) push(m[3], false)
    else if (m[4] !== undefined) push(m[4], false)
    else if (m[5] !== undefined) push(m[5], false)
    else if (m[6] !== undefined) push(m[6], false)
    else if (m[7] !== undefined) push(m[8] ? `${m[7]} (${m[8]})` : m[7], false)
    last = m.index + m[0].length
  }
  push(text.slice(last), false)
  return runs
}

/** Inline markdown flattened to plain text (bold markers removed). */
export function plainText(text: string): string {
  return inlineRuns(text).map((r) => r.text).join('')
}

// ---------------------------------------------------------------- shared

interface DocSection {
  heading: string
  blocks: Block[]
}

function documentSections(input: ExportInput): DocSection[] {
  const sections: DocSection[] = [
    { heading: 'Question', blocks: parseMarkdown(input.question) },
    { heading: 'Response', blocks: parseMarkdown(input.answerMarkdown) },
  ]
  const citations = (input.citations ?? []).filter((c) => c.title || c.label)
  if (citations.length > 0) {
    sections.push({
      heading: 'Sources',
      blocks: citations.map((c) => ({
        kind: 'bullet' as const,
        text: c.label ? `**${c.label}** — ${c.title}` : c.title,
      })),
    })
  }
  return sections
}

function dateLine(input: ExportInput): string {
  const d = input.generatedAt ?? new Date()
  return d.toLocaleString('en-IN', {
    day: 'numeric', month: 'long', year: 'numeric', hour: 'numeric', minute: '2-digit',
  })
}

/** Filename base: the question, slugified, capped at ~60 chars. */
export function exportFileBase(input: ExportInput): string {
  const slug = input.question
    .toLowerCase()
    .normalize('NFD')
    .replace(/[̀-ͯ]/g, '')
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 60)
    .replace(/-+$/g, '')
  return slug || 'juris-vault-response'
}

// ---------------------------------------------------------------- PDF

/** Fold text to printable ASCII for the built-in PDF fonts: accents
 *  lose their marks, common legal/typographic symbols get readable
 *  ASCII stand-ins (₹ → Rs, § → Sec., smart quotes → straight). */
export function asciiFold(s: string): string {
  const mapped = s
    .replace(/[‘’]/g, "'")
    .replace(/[“”]/g, '"')
    .replace(/[–—]/g, '-')
    .replace(/…/g, '...')
    .replace(/₹\s*/g, 'Rs ')
    .replace(/§\s*/g, 'Sec. ')
    .replace(/•/g, '-')
    .replace(/ /g, ' ')
    .replace(/©/g, '(c)')
    .replace(/×/g, 'x')
    .replace(/→/g, '->')
    .replace(/≤/g, '<=')
    .replace(/≥/g, '>=')
  return mapped
    .normalize('NFD')
    .replace(/[̀-ͯ]/g, '')
    .replace(/[^\x20-\x7E\n]/g, '')
}

function pdfEscape(s: string): string {
  return s.replace(/\\/g, '\\\\').replace(/\(/g, '\\(').replace(/\)/g, '\\)')
}

interface PdfLine {
  text: string
  bold: boolean
  size: number
  /** Vertical space consumed by this line (leading + paragraph gap). */
  advance: number
  /** Extra left indent in points. */
  indent: number
}

/** Helvetica's average glyph advance is ~0.5em for mixed prose; 0.52
 *  keeps wrapped lines safely inside the measure. */
function wrapText(text: string, size: number, widthPt: number): string[] {
  const maxChars = Math.max(12, Math.floor(widthPt / (size * 0.52)))
  const out: string[] = []
  for (const hard of text.split('\n')) {
    const words = hard.split(/\s+/).filter(Boolean)
    if (words.length === 0) {
      out.push('')
      continue
    }
    let cur = ''
    for (const word of words) {
      if (word.length > maxChars) {
        if (cur) { out.push(cur); cur = '' }
        for (let i = 0; i < word.length; i += maxChars) out.push(word.slice(i, i + maxChars))
        continue
      }
      const next = cur ? `${cur} ${word}` : word
      if (next.length <= maxChars) cur = next
      else { out.push(cur); cur = word }
    }
    if (cur) out.push(cur)
  }
  return out
}

function blocksToPdfLines(blocks: Block[], width: number): PdfLine[] {
  const lines: PdfLine[] = []
  const pushWrapped = (text: string, opts: Partial<PdfLine>, indent = 0) => {
    const size = opts.size ?? 10.5
    for (const piece of wrapText(plainText(text), size, width - indent)) {
      lines.push({
        text: piece,
        bold: opts.bold ?? false,
        size,
        advance: opts.advance ?? size * 1.42,
        indent,
      })
    }
  }
  for (const b of blocks) {
    switch (b.kind) {
      case 'heading':
        lines.push({ text: '', bold: false, size: 6, advance: 8, indent: 0 })
        pushWrapped(b.text, { bold: true, size: b.level <= 1 ? 13.5 : 11.5, advance: 19 })
        break
      case 'paragraph':
        pushWrapped(b.text, { advance: 15.5 })
        lines.push({ text: '', bold: false, size: 5, advance: 6, indent: 0 })
        break
      case 'bullet':
      case 'numbered': {
        // Hanging indent: the marker sits on the first wrapped line,
        // continuation lines align with the text.
        const marker = b.kind === 'bullet' ? '- ' : `${b.n}. `
        wrapText(plainText(b.text), 10.5, width - 16).forEach((piece, i) => {
          lines.push({
            text: i === 0 ? `${marker}${piece}` : piece,
            bold: false,
            size: 10.5,
            advance: 15,
            indent: i === 0 ? 2 : 16,
          })
        })
        break
      }
      case 'tableRow':
        pushWrapped(b.cells.map(plainText).join('  |  '), { size: 10, advance: 15 })
        break
      case 'code':
        pushWrapped(b.text, { size: 9.5, advance: 14 })
        break
      case 'rule':
        lines.push({ text: '------------------------------------------------------------', bold: false, size: 9, advance: 14, indent: 0 })
        break
    }
  }
  return lines
}

/** A real, minimal PDF 1.4 document: catalog, pages tree, the two
 *  built-in Helvetica fonts, and one content stream per page with
 *  correct xref offsets. Opens in any standard viewer. */
export function buildPdf(input: ExportInput): Blob {
  const pageW = 595
  const pageH = 842
  const margin = 56
  const width = pageW - margin * 2
  const topY = pageH - margin
  const bottomY = 64

  const lines: PdfLine[] = []
  lines.push({ text: 'Juris Vault AI', bold: true, size: 17, advance: 24, indent: 0 })
  if (input.title) {
    for (const piece of wrapText(asciiFold(plainText(input.title)), 11, width)) {
      lines.push({ text: piece, bold: false, size: 11, advance: 16, indent: 0 })
    }
  }
  lines.push({ text: '', bold: false, size: 6, advance: 10, indent: 0 })
  for (const section of documentSections(input)) {
    lines.push({ text: section.heading, bold: true, size: 13, advance: 24, indent: 0 })
    lines.push(...blocksToPdfLines(section.blocks, width).map((l) => ({ ...l, text: asciiFold(l.text) })))
    lines.push({ text: '', bold: false, size: 6, advance: 10, indent: 0 })
  }

  // Paginate.
  const pages: PdfLine[][] = []
  let current: PdfLine[] = []
  let y = topY
  for (const line of lines) {
    if (y - line.advance < bottomY && current.length > 0) {
      pages.push(current)
      current = []
      y = topY
    }
    current.push(line)
    y -= line.advance
  }
  if (current.length > 0 || pages.length === 0) pages.push(current)

  const footer = asciiFold(`Generated by Juris Vault AI - ${dateLine(input)}`)
  const pageStreams = pages.map((pageLines, idx) => {
    const parts: string[] = []
    let cursor = topY
    for (const line of pageLines) {
      cursor -= line.advance
      if (!line.text) continue
      const font = line.bold ? 'F2' : 'F1'
      parts.push(
        `BT /${font} ${line.size} Tf ${margin + line.indent} ${cursor.toFixed(1)} Td (${pdfEscape(line.text)}) Tj ET`,
      )
    }
    parts.push(
      `BT /F1 8.5 Tf ${margin} 40 Td (${pdfEscape(`${footer}  -  Page ${idx + 1} of ${pages.length}`)}) Tj ET`,
    )
    return parts.join('\n')
  })

  // Object layout: 1 catalog, 2 pages, 3/4 fonts, then per page a
  // Page object + its content stream.
  const objects: string[] = []
  const fontRegular = 3
  const fontBold = 4
  const firstPageObj = 5
  objects[1] = `<< /Type /Catalog /Pages 2 0 R >>`
  const kids = pageStreams.map((_, i) => `${firstPageObj + i * 2} 0 R`).join(' ')
  objects[2] = `<< /Type /Pages /Kids [${kids}] /Count ${pageStreams.length} >>`
  objects[fontRegular] = `<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>`
  objects[fontBold] = `<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>`
  pageStreams.forEach((stream, i) => {
    const pageObj = firstPageObj + i * 2
    const contentObj = pageObj + 1
    objects[pageObj] =
      `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 ${pageW} ${pageH}] ` +
      `/Resources << /Font << /F1 ${fontRegular} 0 R /F2 ${fontBold} 0 R >> >> /Contents ${contentObj} 0 R >>`
    objects[contentObj] = stream
  })

  let pdf = '%PDF-1.4\n'
  const offsets: number[] = [0]
  const total = firstPageObj + pageStreams.length * 2
  for (let n = 1; n < total; n++) {
    offsets[n] = pdf.length
    if (n >= firstPageObj && (n - firstPageObj) % 2 === 1) {
      const stream = objects[n]
      pdf += `${n} 0 obj\n<< /Length ${stream.length} >>\nstream\n${stream}\nendstream\nendobj\n`
    } else {
      pdf += `${n} 0 obj\n${objects[n]}\nendobj\n`
    }
  }
  const xrefAt = pdf.length
  pdf += `xref\n0 ${total}\n`
  pdf += '0000000000 65535 f \n'
  for (let n = 1; n < total; n++) {
    pdf += `${String(offsets[n]).padStart(10, '0')} 00000 n \n`
  }
  pdf += `trailer\n<< /Size ${total} /Root 1 0 R >>\nstartxref\n${xrefAt}\n%%EOF`
  return new Blob([pdf], { type: 'application/pdf' })
}

// ---------------------------------------------------------------- RTF

/** RTF-escape one run of text (control words escaped, non-ASCII as
 *  \uN with a '?' fallback byte). */
function rtfText(s: string): string {
  let out = ''
  for (const ch of s) {
    const code = ch.codePointAt(0) ?? 0
    if (ch === '\\') out += '\\\\'
    else if (ch === '{') out += '\\{'
    else if (ch === '}') out += '\\}'
    else if (ch === '\n') out += '\\line '
    else if (code < 128) out += ch
    else {
      const signed = code > 32767 ? code - 65536 : code
      out += `\\u${signed}?`
    }
  }
  return out
}

function rtfRuns(text: string): string {
  return inlineRuns(text)
    .map((r) => (r.bold ? `\\b ${rtfText(r.text)}\\b0 ` : rtfText(r.text)))
    .join('')
}

function blocksToRtf(blocks: Block[]): string {
  const parts: string[] = []
  for (const b of blocks) {
    switch (b.kind) {
      case 'heading':
        parts.push(`\\pard\\sb240\\sa120\\f0\\fs${b.level <= 1 ? 28 : 24}\\b ${rtfRuns(b.text)}\\b0\\fs22\\par`)
        break
      case 'paragraph':
        parts.push(`\\pard\\sa160\\f0\\fs22 ${rtfRuns(b.text)}\\par`)
        break
      case 'bullet':
        parts.push(`\\pard\\li360\\fi-180\\sa80\\f0\\fs22 \\bullet\\tab ${rtfRuns(b.text)}\\par`)
        break
      case 'numbered':
        parts.push(`\\pard\\li360\\fi-180\\sa80\\f0\\fs22 ${b.n}.\\tab ${rtfRuns(b.text)}\\par`)
        break
      case 'tableRow':
        parts.push(`\\pard\\sa80\\f0\\fs22 ${rtfRuns(b.cells.join(' | '))}\\par`)
        break
      case 'code':
        parts.push(`\\pard\\sa80\\f0\\fs20 ${rtfText(b.text)}\\par`)
        break
      case 'rule':
        parts.push('\\pard\\brdrb\\brdrs\\brdrw10\\brsp20 \\sa160\\par\\pard\\par')
        break
    }
  }
  return parts.join('\n')
}

/** A complete RTF 1.x document: font table, styled headings, bold
 *  runs, bullets, a Sources section and a footer line. */
export function buildRtf(input: ExportInput): Blob {
  const parts: string[] = []
  parts.push('{\\rtf1\\ansi\\ansicpg1252\\deff0{\\fonttbl{\\f0\\fnil Helvetica;}}')
  parts.push('\\f0\\fs36\\b Juris Vault AI\\b0\\par')
  if (input.title) {
    parts.push(`\\fs22\\i ${rtfRuns(input.title)}\\i0\\par`)
  }
  parts.push('\\pard\\brdrb\\brdrs\\brdrw10\\brsp20 \\sa200\\par\\pard\\par')
  for (const section of documentSections(input)) {
    parts.push(`\\pard\\sb280\\sa140\\fs28\\b ${rtfText(section.heading)}\\b0\\fs22\\par`)
    parts.push(blocksToRtf(section.blocks))
  }
  parts.push(`\\pard\\sb280\\fs18\\i Generated by Juris Vault AI - ${rtfText(dateLine(input))}\\i0\\par`)
  parts.push('}')
  return new Blob([parts.join('\n')], { type: 'application/rtf' })
}
