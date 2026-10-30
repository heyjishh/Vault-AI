# Public tunnel for the legal-rag server

The sandbox blocks every free tunnel trick (cloudflared ignores the proxy,
localtunnel needs raw TCP, the proxy blackholes non-443 CONNECT, UDP is
blocked, Tailscale is outbound-only). What works: a **Cloudflare Worker as a
WebSocket relay** — pure HTTPS/WSS through the egress proxy.

## Architecture

```
browser -> https://legal-rag-relay.jurisvault.workers.dev
        -> Worker -> Durable Object `Relay` (holds the VM socket)
        -> WSS /vm -> vm-agent.js -> http://127.0.0.1:3000
```

- `relay-worker.js` — the Worker + `Relay` Durable Object. Deployed on
  JISHAN's Cloudflare account (workers.dev subdomain `jurisvault`).
  The DO is required: plain Worker globals are per-isolate, so the VM's
  socket would be invisible to request handlers.
- `vm-agent.js` — runs on the VM. Opens WSS to `<worker>/vm` through the
  egress proxy (manual CONNECT + WS handshake + frame codec; Node's WebSocket
  doesn't use the proxy env) and forwards each request to the app. Has a
  dead-socket watchdog: pings every 15s, reconnects if no pong/message for 45s
  (half-open connections otherwise strand the tunnel showing "VM not connected").

## Streaming protocol (Piece 22)

Responses stream end-to-end so SSE (Ask) reaches the browser progressively:

- Agent → Worker, per request id: a head frame `{id, stream:1, status,
  headers}` when the app sends response headers, one `{id, stream:1,
  chunk:<base64>}` frame per app chunk, then `{id, stream:1, end:true}`.
  A mid-stream failure sends `{id, stream:1, error}` (the Worker aborts the
  body); a failure before headers falls back to the legacy one-shot 502.
- The Worker resolves the client `Response` on the head frame and pipes
  chunk frames through a `TransformStream`. It still accepts the previous
  agent's single whole-body message `{id, status, headers, body}`.
- Timeouts are idle-based, not total: 55s for the first frame, 300s without
  a frame mid-stream (agent and Worker) — long generations (Research) are
  fine while bytes keep flowing.

**Rollout order matters: deploy the Worker FIRST, then restart the agent.**
The new Worker handles both agent shapes; an old Worker would mistake a
head frame for a complete (empty) response and break the site.

Deploy (needs Cloudflare auth — `wrangler login` or `CLOUDFLARE_API_TOKEN`;
the original token was used transiently and never stored):

```bash
cd ~/workspace/legal-rag/tools/tunnel
cp relay-worker.js worker.js        # wrangler.toml's main = worker.js
npx wrangler deploy
# then restart the agent the way restore-stack.sh launches it:
kill <vm-agent pid>
cd ~/workspace/legal-rag && setsid nohup env \
  WORKER_URL=https://legal-rag-relay.jurisvault.workers.dev \
  node tools/tunnel/vm-agent.js >> vm-agent.log 2>&1 < /dev/null &
```
- `wrangler.toml` — deploy config (needs `new_sqlite_classes` on free plan).

## Reconnect after a VM restart

The Worker persists on Cloudflare's side. On the VM:

```bash
cd ~/workspace/legal-rag
./target/debug/legal-rag-server &            # from the project dir (loads .env)
WORKER_URL=https://legal-rag-relay.jurisvault.workers.dev \
  node tools/tunnel/vm-agent.js &
```

Health: `curl https://legal-rag-relay.jurisvault.workers.dev/healthz`
should print `vm-connected`.

## Teardown

```bash
pkill -f "vm-agent[.]js"          # stop the agent (isolated pkill, no pattern reuse)
wrangler delete legal-rag-relay   # remove the worker (needs CLOUDFLARE_API_TOKEN)
```

The API token JISHAN pasted for this was used transiently (deploy calls only)
and never stored.
