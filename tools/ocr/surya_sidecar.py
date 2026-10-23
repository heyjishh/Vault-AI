#!/usr/bin/env python3
"""Primary OCR sidecar for legal-rag (Surya).

Contract (matches legal-rag's `CommandOcr`): read one PNG image on stdin,
write the recognized text on stdout, exit non-zero on failure.

Enable in the server environment (this is the default primary):

    RAG_ANYTHING_OCR_PRIMARY_CMD="python3 /path/to/tools/ocr/surya_sidecar.py"

Requires `surya-ocr` on PATH (pip install surya-ocr) and, for Surya 0.22,
a llama.cpp backend: set LLAMA_CPP_BINARY to a `llama-server` binary (see
~/workspace/ocr-bench for the working environment). Surya downloads its
model weights on first run. GPU is strongly recommended in production;
CPU works, just slower (minutes per page on small VMs).

Output handling covers both Surya result schemas:
  - 0.22: <out>/<input-dir-name>/results.json, {img: [{page, blocks:
    [{label, bbox, html}]}]} — block HTML is stripped to plain text.
  - older: {img: {text_lines: [{text}]}}.
"""
import glob
import html as html_mod
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

TAG_RE = re.compile(r"<[^>]+>")


def block_text(block: dict) -> str:
    raw = block.get("html") or block.get("text") or ""
    return html_mod.unescape(TAG_RE.sub("", raw)).strip()


def extract_text(data: dict) -> str:
    chunks: list[str] = []
    for value in data.values():
        pages = value if isinstance(value, list) else [value]
        for page in pages:
            if not isinstance(page, dict):
                continue
            if page.get("blocks"):
                for block in page["blocks"]:
                    t = block_text(block)
                    if t:
                        chunks.append(t)
            elif page.get("text_lines"):
                for line in page["text_lines"]:
                    t = (line.get("text") or "").strip()
                    if t:
                        chunks.append(t)
    return "\n".join(chunks)


def main() -> int:
    png = sys.stdin.buffer.read()
    if not png:
        print("surya_sidecar: empty stdin", file=sys.stderr)
        return 2
    with tempfile.TemporaryDirectory(prefix="surya-ocr-") as td:
        work = Path(td)
        img = work / "page.png"
        img.write_bytes(png)
        outdir = work / "out"
        proc = subprocess.run(
            ["surya_ocr", str(work), "--output_dir", str(outdir)],
            capture_output=True,
            text=True,
        )
        if proc.returncode != 0:
            print(proc.stderr[-2000:], file=sys.stderr)
            return proc.returncode or 1
        candidates = glob.glob(str(outdir / "**" / "results.json"), recursive=True)
        if not candidates:
            print("surya_sidecar: no results.json produced", file=sys.stderr)
            return 1
        data = json.loads(Path(candidates[0]).read_text())
        text = extract_text(data)
        if not text.strip():
            # Surya exits 0 with empty blocks when its backend fails to
            # spawn — treat empty output as failure so the chain falls
            # through to the next engine instead of indexing nothing.
            print("surya_sidecar: empty result (backend failure?)", file=sys.stderr)
            return 1
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
