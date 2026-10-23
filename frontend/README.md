# LexAgent

A web admin & management UI for a [LexAgent](https://github.com/infiniflow/lexagent) deployment, built for legal knowledge workspaces.

LexAgent is a React + TypeScript single-page app that wires together S3 buckets, knowledge bases, the Knowledge Graph (GraphRAG), chat assistants, agents, and model configuration through the LexAgent REST API — **no manual SQL or backend edits required**. Every action goes through the API and refreshes via query invalidation (no full page reloads).

---

## Features

- **S3 Connector** — browse buckets, and pick a **whole bucket / folder / single file** via the UI (no forced prefix). Link a bucket or folder directly to a knowledge base.
- **Knowledge Bases** — create and manage KBs, choose the embedding model (recommended: `mistral-embed`), link S3 connectors, and view an ingestion summary.
- **Knowledge Graph (GraphRAG)** — enable and build the graph. The **Build / Rebuild Graph** button inherits the tenant default chat model when the KB has none configured, so the build no longer fails with `"Failed"`.
- **Chat Assistants & Agents** — configure and manage conversational assistants and workflow agents.
- **Models** — view and select tenant-available models (chat / embedding / rerank).
- **SPA behavior** — all mutations invalidate queries and re-fetch; the UI never does a full reload.

---

## Tech Stack

- React 18 + TypeScript + Vite
- TanStack Query for data fetching / caching / invalidation
- Axios API client (`src/services/lexagentClient.ts`)
- Tailwind CSS for styling

---

## Getting Started

```bash
cd lexagent
npm install
npm run dev -- --host 0.0.0.0 --port 5174
```

- **Build:** `npm run build`
- **Lint:** `npm run lint`
- **Type-check:** `npx tsc --noEmit`

The app expects a LexAgent backend running on `http://localhost:9380`. Configure the base URL via the Vite proxy / `VITE_API_BASE` in `vite.config.ts`. Log in with the LexAgent admin account (default `admin@lexagent.io`).

---

## How the Knowledge Graph build works

Building the graph calls `POST /api/v1/datasets/{id}/index?type=graph`. The graphrag task reads each document's **existing** chunks from the doc store and runs entity/relation extraction with a chat model.

Important behavior:

- **Build only includes documents that are already chunked.** Let chunking finish, then build once for a complete graph. Rebuilding later only picks up documents that had no chunks at the time of the first build (already-built docs are checkpointed and skipped).
- The KB's `parser_config.llm_id` must point to a valid chat model, or the build fails with `"Failed"`. The **Build** button auto-sets it to the tenant default chat model when it is empty.
- Embedding falls back to the tenant default (`mistral-embed`).

---

## Configuration notes

- **Embedding:** `mistral-embed` is the recommended embedding model for this deployment.
- **S3:** any S3-compatible bucket works. The connector UI lets you select a whole bucket, a folder prefix, or an individual file — a prefix is optional.
- **Secrets:** API keys and credentials live in the backend `.env` and are **never** shipped with this frontend.

---

## Project Layout

- `src/components/admin/AdminPanel.tsx` — main admin surface (KBs, Knowledge Graph, S3 connectors, models).
- `src/components/knowledge-graph/` — Knowledge Graph visualization.
- `src/services/lexagentClient.ts` — API client (datasets, index, models, connectors).
- `src/hooks/useAdmin.ts` — data hooks (`useRunIndex`, `useTraceIndex`, `useDefaultModels`, `useConnectorLogs`).
- `src/components/admin/S3ConnectorPicker.tsx` — bucket / folder / file picker without forced prefix.

## Key API endpoints used

| Action | Method & path |
| --- | --- |
| List / get dataset | `GET /api/v1/datasets`, `GET /api/v1/datasets/{id}` |
| Update dataset (embedding, parser_config) | `PUT /api/v1/datasets/{id}` |
| Build / trace Knowledge Graph | `POST /api/v1/datasets/{id}/index?type=graph`, `GET /api/v1/datasets/{id}/index?type=graph` |
| Knowledge Graph data | `GET /api/v1/datasets/{id}/graph` |
| Default models | `GET /api/v1/models/default` |
| S3 browse / connectors | `/api/v1/connectors/...` |
