#!/usr/bin/env python3
"""Multi-engine web search over plain HTTP (no browser, no API key).

DuckDuckGo HTML is the primary engine — for Indian legal queries it
surfaces primary authorities (sci.gov.in judgment PDFs, Indian Kanoon,
Supreme Court Observer) that Bing's scraped results missed entirely.
Bing HTML is the secondary engine and Brave HTML the third (added
2026-10-23 after Mojeek captcha'd and Startpage served a shell from
this IP); results are merged, deduped by canonical URL, and boosted by
legal-authority domain tier so primary sources lead. DDG requests are
politely paced with backoff retries on throttle statuses. Prints a
JSON array of {url, title, snippet} to stdout.

Usage: websearch.py "<query>" [max_results]
Returns up to 2x max_results candidates (min 8, cap 12) so the caller
can fall back when a fetch fails.
"""
import base64
import html
import json
import os
import random
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

UA = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0"
TIMEOUT = 25

# Primary Indian legal authorities — these lead the merged list.
TIER1 = (
    "indiankanoon.org", "sci.gov.in", "judgments.ecourts.gov.in",
    "ecourts.gov.in", "egazette.nic.in", "legislative.gov.in",
    "lawmin.gov.in", "incometaxindia.gov.in", "gst.gov.in",
    "sebi.gov.in", "rbi.org.in", "cci.gov.in", "nclt.gov.in",
    "indiacode.nic.in", "barcouncilofindia.org", "ecommitteesci.gov.in",
)
# Quality legal commentary / reporting.
TIER2 = (
    "livelaw.in", "barandbench.com", "scconline.com", "scobserver.in",
    "thehindu.com", "indianexpress.com", "prsindia.org",
    "vidhilegalpolicy.in", "manupatra.com", "lexology.com",
    "monday.com", "law.asia", "vantageasia.com",
)


def tier_of(url: str) -> int:
    host = urllib.parse.urlparse(url).netloc.lower().removeprefix("www.")
    for d in TIER1:
        if host == d or host.endswith("." + d):
            return 2
    for d in TIER2:
        if host == d or host.endswith("." + d):
            return 1
    return 0


def canonical(url: str) -> str:
    """Dedupe key: scheme-less, www-less, no fragment/tracking params."""
    p = urllib.parse.urlparse(url)
    host = p.netloc.lower().removeprefix("www.")
    path = re.sub(r"/+$", "", p.path)
    qs = [(k, v) for k, v in urllib.parse.parse_qsl(p.query)
          if not k.startswith("utm_") and k not in ("fbclid", "gclid")]
    return host + path + ("?" + urllib.parse.urlencode(qs) if qs else "")


# --- Polite pacing + throttle backoff -------------------------------------
# DDG anomaly-blocks datacenter IPs after query bursts (HTTP 202 with an
# empty/block body). Two defences: (1) a jittered minimum interval between
# DDG requests, enforced across processes with a timestamp file because
# every CLI invocation is a fresh process; (2) exponential-backoff retries
# on throttle statuses (202 / 429 / 503), max 3 attempts.
RETRY_STATUSES = {202, 429, 503}
DDG_MIN_INTERVAL = (1.2, 2.2)  # seconds, jittered
PACE_FILE = "/tmp/.websearch-ddg-pace"
_last_ddg_at = 0.0


def _pace_ddg() -> None:
    global _last_ddg_at
    interval = random.uniform(*DDG_MIN_INTERVAL)
    try:
        import fcntl
        fd = os.open(PACE_FILE, os.O_CREAT | os.O_RDWR, 0o644)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX)
            raw = os.read(fd, 32).decode().strip()
            last = max(float(raw) if raw else 0.0, _last_ddg_at)
            wait = last + interval - time.time()
            if wait > 0:
                time.sleep(wait)
            _last_ddg_at = time.time()
            os.lseek(fd, 0, os.SEEK_SET)
            os.ftruncate(fd, 0)
            os.write(fd, str(_last_ddg_at).encode())
        finally:
            os.close(fd)
    except Exception:
        # Pacing is best-effort; never fail a search over it.
        wait = _last_ddg_at + interval - time.time()
        if wait > 0:
            time.sleep(wait)
        _last_ddg_at = time.time()


def fetch(url: str) -> str:
    host = urllib.parse.urlparse(url).netloc.lower()
    is_ddg = "duckduckgo.com" in host
    for attempt in range(3):
        if is_ddg:
            _pace_ddg()
        req = urllib.request.Request(url, headers={"User-Agent": UA})
        try:
            with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
                status = r.status
                body = r.read().decode("utf-8", errors="ignore")
        except urllib.error.HTTPError as e:
            if e.code not in RETRY_STATUSES:
                raise
            status, body = e.code, ""
        if status in RETRY_STATUSES:
            if attempt < 2:
                delay = 1.5 * (2 ** attempt) * random.uniform(0.8, 1.3)
                print(f"websearch: {host} HTTP {status} — backoff "
                      f"{delay:.1f}s (attempt {attempt + 1}/3)",
                      file=sys.stderr)
                time.sleep(delay)
                continue
            raise RuntimeError(f"{host} throttled: HTTP {status} after 3 attempts")
        return body
    raise RuntimeError(f"{host} fetch failed")


def clean(s: str) -> str:
    return re.sub(r"\s+", " ", html.unescape(re.sub(r"<[^>]+>", " ", s))).strip()


def search_ddg(query: str) -> list:
    url = "https://html.duckduckgo.com/html/?q=" + urllib.parse.quote_plus(query)
    try:
        page = fetch(url)
    except Exception as e:
        print(f"websearch: ddg failed: {e}", file=sys.stderr)
        return []
    out = []
    for m in re.finditer(
        r'class="result__a"[^>]*href="([^"]+)"[^>]*>(.*?)</a>', page, re.S
    ):
        href = html.unescape(m.group(1))
        uddg = urllib.parse.parse_qs(urllib.parse.urlparse(href).query).get("uddg")
        real = uddg[0] if uddg else href
        if not real.startswith("http"):
            continue
        title = clean(m.group(2))
        tail = page[m.end():m.end() + 1200]
        sn = re.search(r'class="result__snippet"[^>]*>(.*?)</a>', tail, re.S)
        out.append({"url": real, "title": title,
                    "snippet": clean(sn.group(1))[:400] if sn else ""})
    return out


def search_bing(query: str) -> list:
    url = ("https://www.bing.com/search?q=" + urllib.parse.quote_plus(query)
           + "&count=20")
    try:
        page = fetch(url)
    except Exception as e:
        print(f"websearch: bing failed: {e}", file=sys.stderr)
        return []
    out = []
    for m in re.finditer(r'<li class="b_algo".*?<h2><a href="([^"]+)"[^>]*>(.*?)</a>', page, re.S):
        href = html.unescape(m.group(1))
        b = re.search(r"[?&]u=a1([A-Za-z0-9+/=]+)", href)
        if b:
            try:
                href = base64.b64decode(b.group(1) + "==").decode("utf-8", "ignore")
            except Exception:
                pass
        if not href.startswith("http"):
            continue
        tail = page[m.end():m.end() + 1500]
        sn = re.search(r'<div class="b_caption"><p[^>]*>(.*?)</p>', tail, re.S)
        out.append({"url": href, "title": clean(m.group(2)),
                    "snippet": clean(sn.group(1))[:400] if sn else ""})
    return out


def search_brave(query: str) -> list:
    """Brave Search HTML — third independent backend. Verified
    2026-10-23 from this datacenter IP: real results in ~1-2s, while
    Mojeek served a captcha page and Startpage a JS shell. Always-on:
    it is fast, clean, and removes the single-point-of-failure when
    DDG throttles and Bing thins out."""
    url = ("https://search.brave.com/search?q=" + urllib.parse.quote_plus(query)
           + "&source=web")
    try:
        page = fetch(url)
    except Exception as e:
        print(f"websearch: brave failed: {e}", file=sys.stderr)
        return []
    out = []
    for block in re.split(r'<div class="snippet ', page)[1:]:
        if 'data-type="web"' not in block[:300]:
            continue
        a = re.search(r'<a href="(https?://[^"]+)"[^>]*class="[^"]*\bl1\b', block)
        if not a:
            a = re.search(r'<a href="(https?://[^"]+)"', block)
        if not a:
            continue
        href = html.unescape(a.group(1))
        if "brave.com" in urllib.parse.urlparse(href).netloc:
            continue
        t = re.search(
            r'class="title search-snippet-title[^"]*"[^>]*>(.*?)</div>',
            block, re.S)
        s = re.search(
            r'class="content desktop-default-regular[^"]*"[^>]*>(.*?)</div>',
            block, re.S)
        out.append({"url": href, "title": clean(t.group(1)) if t else "",
                    "snippet": clean(s.group(1))[:400] if s else ""})
    return out


DDG_FIREFOX_JS = """(maxResults) => {
    const out = [];
    for (const a of document.querySelectorAll('a.result__a')) {
        if (out.length >= maxResults) break;
        let href = a.getAttribute('href') || '';
        const m = href.match(/uddg=([^&]+)/);
        if (m) { try { href = decodeURIComponent(m[1]); } catch (e) {} }
        if (href.startsWith('http')) out.push({url: href, title: (a.textContent || '').replace(/\\s+/g, ' ').trim(), snippet: ''});
    }
    return out;
}"""


def search_ddg_firefox(query: str) -> list:
    """DDG through the anti-detect Firefox. DDG's plain-HTTP endpoint
    anomaly-blocks datacenter IPs after query bursts (HTTP 202, zero
    results); the real browser passes. Slow (~15-20s) — fallback only."""
    import os
    from invisible_playwright import InvisiblePlaywright
    url = "https://html.duckduckgo.com/html/?q=" + urllib.parse.quote_plus(query)
    try:
        with InvisiblePlaywright(
            headless=os.environ.get("STEALTH_HEADLESS", "1") != "0",
            extra_prefs={"security.enterprise_roots.enabled": True},
        ) as browser:
            page = browser.new_context(ignore_https_errors=True).new_page()
            page.goto(url, wait_until="domcontentloaded", timeout=45000)
            page.wait_for_timeout(4000)
            return page.evaluate(DDG_FIREFOX_JS, 10) or []
    except Exception as e:
        print(f"websearch: ddg-firefox failed: {e}", file=sys.stderr)
        return []


PROBE_STOP = set(
    "what was the is are did does do when where who whom which that this "
    "of on in for and or a an to by with from as it its be been were has "
    "have had how why".split()
)


def probe_terms(query: str) -> str:
    """Content words of the question, for site:-scoped probe searches."""
    words = re.findall(r"[a-z0-9][a-z0-9\-']+", query.lower())
    return " ".join([w for w in words if w not in PROBE_STOP][:8])


def search_probe(domain: str, terms: str) -> list:
    """site:-scoped search restricted to one primary-source domain.

    General engines rank news above the actual judgment PDF for
    question-form queries; a scoped probe guarantees the primary
    source is at least a candidate. Host-filtered because DDG's
    site: support leaks neighbouring domains."""
    q = f"site:{domain} {terms}"
    out = []
    for r in search_bing(q) + search_ddg(q):
        host = urllib.parse.urlparse(r["url"]).netloc.lower().removeprefix("www.")
        if host == domain or host.endswith("." + domain):
            out.append(r)
    return out[:4]


def main() -> None:
    query = sys.argv[1] if len(sys.argv) > 1 else ""
    max_results = int(sys.argv[2]) if len(sys.argv) > 2 else 5
    if not query.strip():
        print(json.dumps([]))
        return
    want = min(max(max_results * 2, 8), 12)

    engines = [search_ddg(query), search_bing(query), search_brave(query)]

    # Primary-source probes: for judgment / statute questions, ask the
    # engines directly for the primary authority. Tier sorting floats
    # these (tier 2) above any news or commentary in the merged list.
    q = query.lower()
    terms = probe_terms(query)
    if terms:
        if any(c in q for c in (
            "judgment", "judgement", "supreme court", "high court",
            "tribunal", "verdict", "constitution bench",
        )) or " v. " in q or " vs " in q or " vs. " in q:
            engines.append(search_probe("sci.gov.in", terms))
            engines.append(search_probe("indiankanoon.org", terms))
        if re.search(r"\b(act|section|statute|amendment|notification|rules)\b", q):
            engines.append(search_probe("indiacode.nic.in", terms))
            engines.append(search_probe("legislative.gov.in", terms))

    if sum(len(e) for e in engines) < 5:
        print("websearch: http engines thin — firefox fallback", file=sys.stderr)
        engines.append(search_ddg_firefox(query))

    merged = {}  # canonical -> (sort_key, result)
    for engine_rank, results in enumerate(engines):
        for rank, r in enumerate(results):
            key = canonical(r["url"])
            # Tier dominates; then earlier engine, then rank within it.
            sort_key = (tier_of(r["url"]) * 1000 - engine_rank * 100 - rank)
            if key in merged:
                old_key, old = merged[key]
                if not old["snippet"] and r["snippet"]:
                    old["snippet"] = r["snippet"]
                if sort_key > old_key:
                    r["snippet"] = r["snippet"] or old["snippet"]
                    merged[key] = (sort_key, r)
            else:
                merged[key] = (sort_key, r)
    ranked = [r for _, r in sorted(merged.values(), key=lambda kv: -kv[0])]
    print(json.dumps(ranked[:want]))


if __name__ == "__main__":
    main()
