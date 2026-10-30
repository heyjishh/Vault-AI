#!/usr/bin/env bash
# OCR command wrapper: run the Surya sidecar with the environment this
# deployment needs, each piece set only when it is actually present so
# the wrapper is a no-op passthrough on a properly installed host.
#
# Why this exists (Piece 25): the bare sidecar invocation fails in this
# sandbox — `surya_ocr` lives in the ocr-bench venv (not on the server
# PATH) and Surya 0.22 silently returns empty output unless its
# llama.cpp backend and local GGUF weights are wired up (see
# ~/workspace/ocr-bench/run_surya_full.sh for the measured-good env).
# Pointing the server's OCR command at this wrapper makes BOTH the PDF
# page fallback and direct image ingestion work; on a host where Surya
# is installed normally, the exports below simply find nothing to add.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"

BENCH_ENV="$HOME/workspace/ocr-bench/bench-env"
if [ -x "$BENCH_ENV/bin/surya_ocr" ]; then
  case ":$PATH:" in
    *":$BENCH_ENV/bin:"*) ;;
    *) export PATH="$BENCH_ENV/bin:$PATH" ;;
  esac
fi

LLAMA_BIN="$HOME/workspace/tools/llama.cpp/llama-b11751/llama-server"
if [ -z "${LLAMA_CPP_BINARY:-}" ] && [ -x "$LLAMA_BIN" ]; then
  export LLAMA_CPP_BINARY="$LLAMA_BIN"
fi

SNAP="$(ls -d "$HOME"/.cache/huggingface/hub/models--datalab-to--surya-ocr-2-gguf/snapshots/*/ 2>/dev/null | head -n 1)"
if [ -n "$SNAP" ]; then
  if [ -z "${SURYA_GGUF_LOCAL_MODEL_PATH:-}" ] && [ -f "${SNAP}surya-2.gguf" ]; then
    export SURYA_GGUF_LOCAL_MODEL_PATH="${SNAP}surya-2.gguf"
  fi
  if [ -z "${SURYA_GGUF_LOCAL_MMPROJ_PATH:-}" ] && [ -f "${SNAP}surya-2-mmproj.gguf" ]; then
    export SURYA_GGUF_LOCAL_MMPROJ_PATH="${SNAP}surya-2-mmproj.gguf"
  fi
fi

export HF_HUB_OFFLINE="${HF_HUB_OFFLINE:-1}"
# OCR is purely local (llama-server on 127.0.0.1, local weights), and
# an inherited proxy environment actively breaks it here: Surya's
# health probe for its llama.cpp backend goes through httpx, whose
# proxy parsing chokes on this sandbox's proxy variables (IPv6
# entries in no_proxy -> "InvalidURL: Invalid port: ':1]'"), so the
# probe never succeeds and Surya kills the backend after its 600 s
# startup timeout — every OCR silently fails. Strip the proxy vars
# for the OCR subprocess; nothing it does needs the network.
unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY
export NO_PROXY="localhost,127.0.0.1"
export no_proxy="localhost,127.0.0.1"
# Surya's per-request inference timeout defaults to 600 s, which a
# dense page/photo exceeds on a small CPU box (the request is killed
# mid-flight and the whole OCR fails). An hour matches this
# deployment's reality: OCR is slow here, not broken.
export SURYA_INFERENCE_TIMEOUT_SECONDS="${SURYA_INFERENCE_TIMEOUT_SECONDS:-3600}"
# Memory: Surya spawns its backend with 8 parallel slots by default
# (a ~98k-token KV cache). On this 8 GB host — where the app stack
# (embeddings/reranker/colbert sidecars) already holds several GB —
# the backend then dies during load from memory pressure. Two slots
# (~24k ctx) keep the cache small; OCR here is throughput-bound by
# CPU anyway, not by slot parallelism.
export SURYA_INFERENCE_PARALLEL="${SURYA_INFERENCE_PARALLEL:-2}"

exec python3 "$REPO/tools/ocr/surya_sidecar.py"
