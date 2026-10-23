# Juris Vault AI — Frontend

Two Vite + React 19 + TypeScript apps, styled with the ivory/ink design
system specified in [DESIGN.md](DESIGN.md):

- **User app** (`src/`) — Dashboard, Ask, Research, Agents, Vault,
  Briefs, Interact, Draft, Private Vault, Plans & Billing. Builds to
  `dist/`, served by the Rust server at `/`.
- **Admin Command Center** (`admin/`) — users, corpus & vault, retrieval
  & models, customization, billing, insights, golden-rule evals. Builds to
  `admin/dist/`, served at `/admin`.

## Develop

```bash
npm install
npm run dev          # user app (expects the API on :3000)
cd admin && npm install && npm run dev
```

## Build

```bash
npm run build        # → dist/
cd admin && npm run build   # → admin/dist/
```

The server serves the built output from disk — rebuilding needs no
server restart.
