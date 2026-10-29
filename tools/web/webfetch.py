#!/usr/bin/env python3
"""Fetch a page and extract readable text. Plain HTTP first (fast, and
most legal sources serve it fine); PDFs are extracted with pypdf (court
sites publish judgments as PDFs); the anti-detect Firefox is the
fallback for bot-guarded or JS-heavy pages.

Usage: webfetch.py "<url>"
Prints JSON {url, title, text, date} to stdout (date may be null).
Exits non-zero only when every path fails.
"""
import io
import json
import os
import re
import sys
import urllib.parse
import urllib.request

UA = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0"
MAX_CHARS = 20000
MIN_HTTP_CHARS = 800


def lines_text(raw: str) -> str:
    lines = []
    for line in raw.split("\n"):
        t = re.sub(r"\s+", " ", line).strip()
        if len(t) > 40:
            lines.append(t)
    return "\n".join(lines)[:MAX_CHARS]


def meta_date(page: str) -> str | None:
    for pat in (
        r'property="article:published_time" content="([0-9]{4}-[0-9]{2}-[0-9]{2})',
        r'name="date" content="([0-9]{4}-[0-9]{2}-[0-9]{2})',
        r'"datePublished"\s*:\s*"([0-9]{4}-[0-9]{2}-[0-9]{2})',
        r'<time[^>]*datetime="([0-9]{4}-[0-9]{2}-[0-9]{2})',
    ):
        m = re.search(pat, page)
        if m:
            return m.group(1)
    return None


def fetch_pdf(data: bytes) -> dict | None:
    try:
        from pypdf import PdfReader
        reader = PdfReader(io.BytesIO(data))
        parts = []
        for page in reader.pages:
            parts.append(page.extract_text() or "")
            if sum(len(p) for p in parts) > MAX_CHARS:
                break
        text = "\n".join(parts)[:MAX_CHARS]
        if len(text.strip()) < 200:
            return None
        title = ""
        if reader.metadata and reader.metadata.title:
            title = str(reader.metadata.title).strip()
        if not title:
            title = next((l.strip() for l in text.split("\n") if len(l.strip()) > 15), "")
        date = None
        if reader.metadata and reader.metadata.creation_date:
            try:
                date = reader.metadata.creation_date.strftime("%Y-%m-%d")
            except Exception:
                date = None
        return {"title": title[:200], "text": text, "date": date}
    except Exception as e:
        print(f"webfetch: pdf failed: {e}", file=sys.stderr)
        return None


def fetch_http(url: str) -> dict | None:
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            ctype = (r.headers.get("Content-Type") or "").lower()
            data = r.read(10_000_000)
    except Exception as e:
        print(f"webfetch: http failed: {e}", file=sys.stderr)
        return None
    if "pdf" in ctype or data[:4] == b"%PDF":
        return fetch_pdf(data)
    page = data.decode("utf-8", errors="ignore")
    date = meta_date(page)
    tm = re.search(r"<title[^>]*>(.*?)</title>", page, re.S)
    title = re.sub(r"\s+", " ", tm.group(1)).strip() if tm else ""
    body = re.sub(
        r"<(script|style|noscript|nav|header|footer|aside|form|iframe|svg)[^>]*>.*?</\1>",
        " ", page, flags=re.S | re.I)
    main = re.search(r"<(article|main)[^>]*>(.*?)</\1>", body, re.S | re.I)
    if main:
        body = main.group(2)
    text = lines_text(re.sub(r"<[^>]+>", "\n", body))
    if len(text) < MIN_HTTP_CHARS:
        return None
    return {"title": title[:200], "text": text, "date": date}


def fetch_firefox(url: str) -> dict | None:
    from invisible_playwright import InvisiblePlaywright
    headless = os.environ.get("STEALTH_HEADLESS", "1") != "0"
    extract_js = """(maxChars) => {
        const doc = document;
        doc.querySelectorAll('script, style, noscript, nav, header, footer, aside, form, iframe, svg').forEach(e => e.remove());
        const root = doc.querySelector('article') || doc.querySelector('main')
            || doc.querySelector('[role="main"]') || doc.body;
        const text = (root.textContent || '');
        const lines = [];
        for (const line of text.split('\\n')) {
            const t = line.replace(/\\s+/g, ' ').trim();
            if (t.length > 40) lines.push(t);
        }
        const meta = doc.querySelector('meta[property="article:published_time"]');
        return {text: lines.join('\\n').slice(0, maxChars),
                date: meta ? (meta.getAttribute('content') || '').slice(0, 10) : ''};
    }"""
    try:
        with InvisiblePlaywright(
            headless=headless,
            extra_prefs={"security.enterprise_roots.enabled": True},
        ) as browser:
            page = browser.new_context(ignore_https_errors=True).new_page()
            try:
                page.goto(url, wait_until="domcontentloaded", timeout=30000)
            except Exception:
                page.goto(url, wait_until="commit", timeout=30000)
            page.wait_for_timeout(2500)
            title = page.title() or ""
            got = page.evaluate(extract_js, MAX_CHARS) or {}
    except Exception as e:
        print(f"webfetch: firefox failed: {e}", file=sys.stderr)
        return None
    text = (got.get("text") or "").strip()
    if not text:
        return None
    return {"title": title.strip()[:200], "text": text,
            "date": got.get("date") or None}


# --- LiveLaw chain ---------------------------------------------------------
# LiveLaw defeats both the plain-HTTP and the Firefox paths (verified
# 2026-10-23: canonical article URL failed end-to-end). Fallback order:
# canonical (HTTP) -> AMP variant (HTTP) -> Firefox -> r.jina.ai text
# extraction proxy (last resort). Each attempt is logged to stderr with
# its path label so the winning path is visible in server logs.

def is_livelaw(url: str) -> bool:
    host = urllib.parse.urlparse(url).netloc.lower().removeprefix("www.")
    return host == "livelaw.in" or host.endswith(".livelaw.in")


def livelaw_amp_url(url: str) -> str | None:
    p = urllib.parse.urlparse(url)
    if "/amp/" in p.path:
        return None
    path = "/amp/" + p.path.lstrip("/")
    return urllib.parse.urlunparse(p._replace(path=path, query=""))


def fetch_jina(url: str) -> dict | None:
    """Text-extraction proxy (r.jina.ai) — returns the page as markdown
    with Title:/Published Time: headers. Last resort for LiveLaw."""
    req = urllib.request.Request("https://r.jina.ai/" + url,
                                 headers={"User-Agent": UA})
    try:
        with urllib.request.urlopen(req, timeout=45) as r:
            raw = r.read().decode("utf-8", errors="ignore")
    except Exception as e:
        print(f"webfetch: jina failed: {e}", file=sys.stderr)
        return None
    tm = re.search(r"^Title:\s*(.+)$", raw, re.M)
    title = tm.group(1).strip() if tm else ""
    dm = re.search(r"Published Time:\s*([0-9]{4}-[0-9]{2}-[0-9]{2})", raw)
    date = dm.group(1) if dm else None
    body = raw.split("Markdown Content:", 1)[-1]
    body = re.sub(r"!\[[^\]]*\]\([^)]*\)", " ", body)          # images
    body = re.sub(r"\[([^\]]+)\]\([^)]*\)", r"\1", body)        # links -> text
    text = lines_text(body)
    if len(text) < 400:
        return None
    return {"title": title[:200], "text": text, "date": date}


def fetch_livelaw(url: str) -> dict | None:
    got = fetch_http(url)
    print(f"webfetch: livelaw path=canonical {'ok' if got else 'fail'}",
          file=sys.stderr)
    if got:
        return got
    amp = livelaw_amp_url(url)
    if amp:
        got = fetch_http(amp)
        print(f"webfetch: livelaw path=amp {'ok' if got else 'fail'}",
              file=sys.stderr)
        if got:
            return got
    got = fetch_firefox(url)
    print(f"webfetch: livelaw path=firefox {'ok' if got else 'fail'}",
          file=sys.stderr)
    if got:
        return got
    got = fetch_jina(url)
    print(f"webfetch: livelaw path=jina {'ok' if got else 'fail'}",
          file=sys.stderr)
    return got


def is_kanoon(url: str) -> bool:
    host = urllib.parse.urlparse(url).netloc.lower().removeprefix("www.")
    return host == "indiankanoon.org" or host.endswith(".indiankanoon.org")


def fetch_kanoon(url: str) -> dict | None:
    """Indian Kanoon blocks this sandbox's egress directly — plain
    HTTP is connection-reset and the stealth Firefox stops at
    Cloudflare's "Just a moment…" (both verified 2026-10-29). The
    reader proxy fetches Kanoon fine, so it leads here; the direct
    paths remain as fallbacks for when the blocks lift."""
    got = fetch_jina(url)
    print(f"webfetch: kanoon path=jina {'ok' if got else 'fail'}",
          file=sys.stderr)
    if got:
        return got
    got = fetch_http(url) or fetch_firefox(url)
    print(f"webfetch: kanoon path=direct {'ok' if got else 'fail'}",
          file=sys.stderr)
    return got


def main() -> None:
    url = sys.argv[1] if len(sys.argv) > 1 else ""
    if not url.startswith("http"):
        print("webfetch: need an http(s) url", file=sys.stderr)
        sys.exit(2)
    if is_livelaw(url):
        got = fetch_livelaw(url)
    elif is_kanoon(url):
        got = fetch_kanoon(url)
    else:
        # Order matters: plain HTTP first (fast when it works), then
        # the reader proxy (fast, and reads from its own infrastructure
        # so per-site egress blocks don't apply — e.g.
        # incometaxindia.gov.in 403s HTTP and stalls the browser for
        # 60s+ before the proxy answers in ~4s), and the stealth
        # Firefox only as the last resort for JS-heavy pages the proxy
        # cannot render either.
        got = fetch_http(url) or fetch_jina(url) or fetch_firefox(url)
    if not got:
        print("webfetch: no readable text found", file=sys.stderr)
        sys.exit(1)
    print(json.dumps({"url": url, "title": got["title"],
                      "text": got["text"], "date": got["date"]}))


if __name__ == "__main__":
    main()
