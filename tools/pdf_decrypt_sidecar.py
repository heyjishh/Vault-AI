#!/usr/bin/env python3
"""Decrypt a password-protected PDF via pypdf — fallback for the schemes
lopdf 0.34 cannot handle (AES-128/AES-256; see
crates/ingest/src/pdf_password.rs for the measured coverage table).

Contract (mirrors the Surya OCR sidecar pattern):
  stdin:  password bytes up to the first newline, then the raw PDF bytes.
  stdout: decrypted PDF bytes (input unchanged when not encrypted).
  exit:   0 ok, 42 wrong password, 1 any other failure.
  (42, not 2: python itself exits 2 when the script file is missing, and
  the caller must never mistake that for a wrong password.)

The password is never printed, never passed on argv, and never appears in
any error output — failures are reported by exit code only. Passwords
containing a newline cannot be expressed in this framing; that is the one
accepted limitation (documented in pdf_password.rs).

Requires `pypdf` (+ `cryptography` for AES-256). The import is served
from the vendored directory next to this script
(`tools/pdf-decrypt-pylibs/`, installed by `restore-stack.sh` and kept
under `~` so it survives VM replacement — a system-wide pip install
does NOT survive, which is exactly how the AES path broke after the
2026-10-26 restart). A system-wide `pypdf` still works as a fallback
via the normal import path.
"""

import io
import sys
from pathlib import Path

# Vendored deps first; system site-packages remain as fallback.
sys.path.insert(0, str(Path(__file__).with_name("pdf-decrypt-pylibs")))


def main() -> int:
    data = sys.stdin.buffer.read()
    nl = data.find(b"\n")
    if nl < 0:
        return 1
    try:
        password = data[:nl].decode("utf-8")
    except UnicodeDecodeError:
        return 1
    pdf_bytes = data[nl + 1 :]
    if not pdf_bytes.startswith(b"%PDF-"):
        return 1
    try:
        from pypdf import PdfReader, PdfWriter
    except ImportError:
        return 1
    try:
        reader = PdfReader(io.BytesIO(pdf_bytes))
    except Exception:
        return 1
    if reader.is_encrypted:
        try:
            # Returns 0 when the password does not match (no exception).
            matched = reader.decrypt(password)
        except Exception:
            return 1
        if not matched:
            return 42
    writer = PdfWriter()
    try:
        for page in reader.pages:
            writer.add_page(page)
        out = io.BytesIO()
        writer.write(out)
    except Exception:
        return 1
    sys.stdout.buffer.write(out.getvalue())
    return 0


if __name__ == "__main__":
    sys.exit(main())
