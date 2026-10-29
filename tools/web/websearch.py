#!/usr/bin/env python3
"""Web search for the legal RAG — rebuilt 2026-10-29.

Why the rebuild: from this sandbox's egress the old paths are dead.
DuckDuckGo HTML anomaly-blocks the datacenter IP (HTTP 202, direct AND
via the stealth Firefox — the browser gets the same block page), Bing's
SERP for this IP degenerated into location/dictionary junk (an
"electoral bonds judgment" query returned US Electoral College
dictionary entries), Brave answers 429, Mojeek captchas, and Indian
Kanoon sits behind a Cloudflare challenge that neither plain HTTP
(connection reset) nor the stealth Firefox ("Just a moment…") passes.

What works from here (all verified live 2026-10-29): the r.jina.ai
reader proxy fetches target pages from its own infrastructure, so it
bypasses this egress's per-site blocks:

1. **DDG via the reader proxy** — html.duckduckgo.com rendered by the
   proxy returns real results in ~5s, primary authorities first for
   Indian legal queries (incometaxindia.gov.in led the agriculture
   query). Also used for `site:`-scoped primary-source probes
   (sci.gov.in returned the actual ADR judgment PDF).
2. **Indian Kanoon via the reader proxy** — Kanoon's own search,
   fetched through the proxy: real case results with snippets in ~7s.
   A direct primary-source probe that needs no general engine.
3. **Lottery tickets** — one fast direct attempt each at DDG HTML and
   Brave (8s timeout, no retries): if their throttling ever lifts they
   add diversity for free; today they return nothing and cost ~1s in
   parallel. (Bing is dropped entirely — its results from this IP are
   actively misleading, not just thin.)

All sources run concurrently under a hard global deadline
(WEBSEARCH_DEADLINE, default 40s); whatever has merged by then is
printed. Results are deduped by canonical URL and ranked by
legal-authority domain tier so primary sources lead. Every source logs
its outcome (count or failure reason) to stderr — a silent empty
search is how the old leg hid its death for a week.

Usage: websearch.py "<query>" [max_results]
Returns up to 2x max_results candidates (min 8, cap 12) so the caller
can fall back when a fetch fails.
"""
import html
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed

UA = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0"
JINA_BASE = os.environ.get("WEBSEARCH_JINA_BASE", "https://r.jina.ai/")
DEADLINE = float(os.environ.get("WEBSEARCH_DEADLINE", "40"))
JINA_TIMEOUT = 25
DIRECT_TIMEOUT = 8

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
    "mondaq.com", "law.asia", "vantageasia.com",
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


def clean(s: str) -> str:
    return re.sub(r"\s+", " ", html.unescape(re.sub(r"<[^>]+>", " ", s))).strip()


def clean_md(s: str) -> str:
    """Strip markdown dressing (images, links, bold) from proxy output."""
    s = re.sub(r"!\[[^\]]*\]\([^)]*\)", " ", s)          # images
    s = re.sub(r"\[([^\]]+)\]\([^)]*\)", r"\1", s)       # links -> text
    s = s.replace("**", " ").replace("__", " ")
    return re.sub(r"\s+", " ", html.unescape(s)).strip()


def http_get(url: str, timeout: int) -> str:
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.read().decode("utf-8", errors="ignore")


def jina_get(url: str) -> str:
    return http_get(JINA_BASE + url, JINA_TIMEOUT)


# --- Source: DuckDuckGo via the reader proxy -------------------------------

def parse_ddg_markdown(md: str) -> list:
    """Reader-proxy markdown of the DDG HTML results page: each result
    is a `## [Title](https://duckduckgo.com/l/?uddg=<real>&rut=...)`
    heading followed by a snippet block."""
    out = []
    heads = list(re.finditer(
        r"## \[([^\]]+)\]\((https://duckduckgo\.com/l/\?[^)]+)\)", md))
    for i, m in enumerate(heads):
        qs = urllib.parse.parse_qs(urllib.parse.urlparse(m.group(2)).query)
        real = (qs.get("uddg") or [""])[0]
        if not real.startswith("http"):
            continue
        end = heads[i + 1].start() if i + 1 < len(heads) else len(md)
        snippet = clean_md(md[m.end():end])
        # The block opens with a favicon link remnant + the snippet's
        # own link text; drop a leading duplicate of the title.
        title = clean_md(m.group(1))
        if snippet.startswith(title):
            snippet = snippet[len(title):].strip()
        out.append({"url": real, "title": title, "snippet": snippet[:400]})
    return out


def search_ddg_jina(query: str) -> list:
    url = ("https://html.duckduckgo.com/html/?q="
           + urllib.parse.quote_plus(query))
    return parse_ddg_markdown(jina_get(url))


def search_probe_jina(domain: str, terms: str) -> list:
    """site:-scoped probe through the working DDG path, host-filtered
    (DDG's site: support leaks neighbouring domains)."""
    out = []
    for r in search_ddg_jina(f"site:{domain} {terms}"):
        host = urllib.parse.urlparse(r["url"]).netloc.lower().removeprefix("www.")
        if host == domain or host.endswith("." + domain):
            out.append(r)
    return out[:4]


# --- Source: Indian Kanoon site search via the reader proxy ----------------

def search_kanoon_jina(terms: str) -> list:
    """Kanoon's own search — the single most useful primary source for
    Indian law (bare Acts + case law). Kanoon blocks this egress
    directly (Cloudflare), so it is read through the proxy."""
    url = ("https://indiankanoon.org/search/?formInput="
           + urllib.parse.quote_plus(terms))
    md = jina_get(url)
    out = []
    heads = list(re.finditer(
        r"#### \[([^\]]+)\]\((https://indiankanoon\.org/(?:doc|docfragment)/\d+/)[^)]*\)",
        md))
    for i, m in enumerate(heads):
        doc_url = m.group(2)
        if "/docfragment/" in doc_url:
            doc_url = doc_url.replace("/docfragment/", "/doc/")
        end = heads[i + 1].start() if i + 1 < len(heads) else len(md)
        snippet = clean_md(md[m.end():end])
        out.append({"url": doc_url, "title": clean_md(m.group(1)),
                    "snippet": snippet[:400]})
    return out


# --- Lottery tickets: direct engines, one fast attempt ----------------------

def search_ddg_direct(query: str) -> list:
    """One attempt, no backoff loop: DDG anomaly-blocks this egress
    (HTTP 202) — if that ever lifts, this rejoins for free."""
    url = "https://html.duckduckgo.com/html/?q=" + urllib.parse.quote_plus(query)
    page = http_get(url, DIRECT_TIMEOUT)
    out = []
    for m in re.finditer(
        r'class="result__a"[^>]*href="([^"]+)"[^>]*>(.*?)</a>', page, re.S
    ):
        href = html.unescape(m.group(1))
        uddg = urllib.parse.parse_qs(urllib.parse.urlparse(href).query).get("uddg")
        real = uddg[0] if uddg else href
        if not real.startswith("http"):
            continue
        tail = page[m.end():m.end() + 1200]
        sn = re.search(r'class="result__snippet"[^>]*>(.*?)</a>', tail, re.S)
        out.append({"url": real, "title": clean(m.group(2)),
                    "snippet": clean(sn.group(1))[:400] if sn else ""})
    return out


def search_brave_direct(query: str) -> list:
    """One attempt: Brave served real results from here on 2026-10-23
    and 429s today — cheap to keep asking, in parallel."""
    url = ("https://search.brave.com/search?q=" + urllib.parse.quote_plus(query)
           + "&source=web")
    page = http_get(url, DIRECT_TIMEOUT)
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


PROBE_STOP = set(
    "what was the is are did does do when where who whom which that this "
    "of on in for and or a an to by with from as it its be been were has "
    "have had how why tell me detail about please explain give want know "
    "brief discuss overview".split()
)


def probe_terms(query: str) -> str:
    """Content words of the question, for site:-scoped probe searches."""
    words = re.findall(r"[a-z0-9][a-z0-9\-']+", query.lower())
    return " ".join([w for w in words if w not in PROBE_STOP][:8])


def run_source(name: str, fn) -> list:
    t0 = time.time()
    try:
        out = fn()
        print(f"websearch: {name}: {len(out)} results "
              f"in {time.time() - t0:.1f}s", file=sys.stderr)
        return out
    except Exception as e:
        print(f"websearch: {name} FAILED in {time.time() - t0:.1f}s: "
              f"{type(e).__name__}: {e}", file=sys.stderr)
        return []


def main() -> None:
    query = sys.argv[1] if len(sys.argv) > 1 else ""
    max_results = int(sys.argv[2]) if len(sys.argv) > 2 else 5
    if not query.strip():
        print(json.dumps([]))
        return
    want = min(max(max_results * 2, 8), 12)
    terms = probe_terms(query) or query

    sources = [
        ("kanoon-jina", lambda: search_kanoon_jina(terms)),
        ("ddg-jina", lambda: search_ddg_jina(query)),
    ]
    # Primary-source probes through the working DDG path.
    q = query.lower()
    probes = []
    if any(c in q for c in (
        "judgment", "judgement", "supreme court", "high court",
        "tribunal", "verdict", "constitution bench",
    )) or " v. " in q or " vs " in q or " vs. " in q:
        probes.append("sci.gov.in")
    if re.search(r"\b(act|section|statute|amendment|notification|rules)\b", q):
        probes += ["indiacode.nic.in", "legislative.gov.in"]
    if re.search(r"\b(income tax|gst|customs|excise|taxation)\b", q):
        probes.append("incometaxindia.gov.in")
    if not probes:
        # Legal product: a web question with no trigger words still
        # deserves the statute-book probe.
        probes = ["indiacode.nic.in"]
    for d in dict.fromkeys(probes):
        sources.append((f"probe:{d}",
                        (lambda dom: lambda: search_probe_jina(dom, terms))(d)))
    sources.append(("brave-direct", lambda: search_brave_direct(query)))
    sources.append(("ddg-direct", lambda: search_ddg_direct(query)))

    by_idx = {}
    t0 = time.time()
    with ThreadPoolExecutor(max_workers=len(sources)) as ex:
        futs = {ex.submit(run_source, name, fn): (i, name)
                for i, (name, fn) in enumerate(sources)}
        try:
            for fut in as_completed(futs, timeout=DEADLINE):
                by_idx[futs[fut][0]] = fut.result()
        except TimeoutError:
            pending = [futs[f][1] for f in futs if not f.done()]
            print(f"websearch: deadline {DEADLINE:.0f}s hit; "
                  f"abandoning: {', '.join(pending)}", file=sys.stderr)
        # Never block on stragglers: their sockets carry their own
        # timeouts (<=25s) and the process exits with what it has.
        ex.shutdown(wait=False)
    engines = [by_idx[i] for i in sorted(by_idx)]
    print(f"websearch: {sum(len(e) for e in engines)} raw results "
          f"in {time.time() - t0:.1f}s", file=sys.stderr)

    merged = {}  # canonical -> (sort_key, engine_idx, result)
    for engine_rank, results in enumerate(engines):
        for rank, r in enumerate(results):
            key = canonical(r["url"])
            # Tier dominates; then earlier engine, then rank within it.
            sort_key = (tier_of(r["url"]) * 1000 - engine_rank * 100 - rank)
            if key in merged:
                old_key, old_eng, old = merged[key]
                if not old["snippet"] and r["snippet"]:
                    old["snippet"] = r["snippet"]
                if sort_key > old_key:
                    r["snippet"] = r["snippet"] or old["snippet"]
                    merged[key] = (sort_key, engine_rank, r)
            else:
                merged[key] = (sort_key, engine_rank, r)
    ranked = [(eng, r) for _, eng, r in
              sorted(merged.values(), key=lambda kv: -kv[0])]
    # Diversity cap: no single engine may fill more than half the
    # window. Without it the Kanoon sweep (10 tier-1 hits) crowds out
    # the official explainer pages (Income Tax Dept, India Code) that
    # sit in another engine at the same tier — the fetch stage then
    # never sees them. Deferred results backfill if slots remain.
    cap = max(2, (want + 1) // 2)
    out, deferred, counts = [], [], {}
    for eng, r in ranked:
        if counts.get(eng, 0) < cap:
            out.append(r)
            counts[eng] = counts.get(eng, 0) + 1
        else:
            deferred.append(r)
        if len(out) >= want:
            break
    if len(out) < want:
        out += deferred[:want - len(out)]
    print(json.dumps(out))


if __name__ == "__main__":
    main()
