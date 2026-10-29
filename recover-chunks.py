#!/usr/bin/env python3
"""Recover lost chunk records into a data dir's chunks.json from a donor.

Piece 20 (corpus durability). When the vector store lost committed
chunks to unclean database restarts and the thinned store overwrote
chunks.json, the chunk records (text + embeddings) may still exist in
another snapshot of the same corpus — e.g. the test instance's
data-test/chunks.json, which shares ingest lineage with prod.

For every document in DATA_DIR/documents.json (the registry is the
authority), this script takes the chunk records named by the registry's
chunk_ids from the DONOR chunks.json whenever the local chunks.json
lacks them. Local records always win on conflict. Records for documents
that are not registered in DATA_DIR are never imported. The merged
chunks.json is written back atomically; the script is idempotent.

Usage:
    recover-chunks.py [--dry-run] DATA_DIR DONOR_CHUNKS_JSON

Exit status is 0 on success (including "nothing to recover"); documents
whose chunks exist in neither the local snapshot nor the donor are
reported as UNRECOVERABLE — they must be re-ingested from source.
"""

import json
import os
import sys


def load_json(path):
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


def registry_documents(documents_json):
    """documents.json -> {doc_id: (title, [chunk_ids])}."""
    docs = documents_json.get("documents", {})
    out = {}
    if isinstance(docs, dict):
        items = docs.items()
    else:  # list form
        items = ((d.get("id"), d) for d in docs)
    for doc_id, info in items:
        if not doc_id or not isinstance(info, dict):
            continue
        out[doc_id] = (info.get("title", doc_id), list(info.get("chunk_ids", [])))
    return out


def main(argv):
    dry_run = "--dry-run" in argv
    args = [a for a in argv if a != "--dry-run"]
    if len(args) != 2:
        print(__doc__)
        return 2
    data_dir, donor_path = args
    chunks_path = os.path.join(data_dir, "chunks.json")
    documents_path = os.path.join(data_dir, "documents.json")

    registry = registry_documents(load_json(documents_path))
    local = load_json(chunks_path) if os.path.exists(chunks_path) else []
    donor = load_json(donor_path)

    local_by_id = {c["id"]: c for c in local}
    donor_by_id = {c["id"]: c for c in donor}

    recovered = []
    unrecoverable = []
    for doc_id in sorted(registry):
        title, chunk_ids = registry[doc_id]
        missing = [cid for cid in chunk_ids if cid not in local_by_id]
        if not missing:
            print(f"OK           {doc_id} ({title}): all {len(chunk_ids)} chunks present")
            continue
        got = [donor_by_id[cid] for cid in missing if cid in donor_by_id]
        lost = [cid for cid in missing if cid not in donor_by_id]
        if got:
            recovered.extend(got)
            print(f"RECOVERED    {doc_id} ({title}): +{len(got)} chunk(s) from donor")
        if lost:
            unrecoverable.append((doc_id, title, lost))
            print(f"UNRECOVERABLE {doc_id} ({title}): {len(lost)} chunk(s) in neither "
                  f"local snapshot nor donor: {', '.join(lost)}")

    merged = list(local) + sorted(
        recovered, key=lambda c: (c["doc_id"], c.get("ordinal", 0), c["id"])
    )
    print(f"\nlocal {len(local)} + recovered {len(recovered)} = {len(merged)} chunk records")
    if unrecoverable:
        print(f"{len(unrecoverable)} document(s) unrecoverable from this donor "
              f"(re-ingest from source):")
        for doc_id, title, _ in unrecoverable:
            print(f"  - {doc_id} ({title})")

    if dry_run:
        print("DRY RUN — chunks.json not written")
        return 0
    if not recovered:
        print("nothing to recover — chunks.json unchanged")
        return 0
    tmp_path = chunks_path + ".tmp"
    with open(tmp_path, "w", encoding="utf-8") as f:
        json.dump(merged, f, indent=2)
        f.write("\n")
    os.replace(tmp_path, chunks_path)
    print(f"wrote {chunks_path} ({len(merged)} records)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
