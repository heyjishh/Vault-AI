# Public tunnel for the legal-rag server

The sandbox blocks every free tunnel trick (cloudflared ignores the proxy,
localtunnel needs raw TCP, the proxy blackholes non-443 CONNECT, UDP is
blocked, Tailscale is outbound-only). What works: a **Cloudflare Worker as a
WebSocket relay** — pure HTTPS/WSS through the egress proxy.

## Architecture

```
browser -> https://legal-rag-relay.jishan-legal-rag.workers.dev
        -> Worker -> Durable Object `Relay` (holds the VM socket)
        -> WSS /vm -> vm-agent.js -> http://127.0.0.1:3000
```

- `relay-worker.js` — the Worker + `Relay` Durable Object. Deployed on
  JISHAN's Cloudflare account (workers.dev subdomain `jishan-legal-rag`).
  The DO is required: plain Worker globals are per-isolate, so the VM's
  socket would be invisible to request handlers.
- `vm-agent.js` — runs on the VM. Opens WSS to `<worker>/vm` through the
  egress proxy (manual CONNECT + WS handshake + frame codec; Node's WebSocket
  doesn't use the proxy env) and forwards each request to the app. Has a
  dead-socket watchdog: pings every 15s, reconnects if no pong/message for 45s
  (half-open connections otherwise strand the tunnel showing "VM not connected").
- `wrangler.toml` — deploy config (needs `new_sqlite_classes` on free plan).

## Reconnect after a VM restart

The Worker persists on Cloudflare's side. On the VM:

```bash
cd ~/workspace/legal-rag
./target/debug/legal-rag-server &            # from the project dir (loads .env)
WORKER_URL=https://legal-rag-relay.jishan-legal-rag.workers.dev \
  node tools/tunnel/vm-agent.js &
```

Health: `curl https://legal-rag-relay.jishan-legal-rag.workers.dev/healthz`
should print `vm-connected`.

## Teardown

```bash
pkill -f "vm-agent[.]js"          # stop the agent (isolated pkill, no pattern reuse)
wrangler delete legal-rag-relay   # remove the worker (needs CLOUDFLARE_API_TOKEN)
```

The API token JISHAN pasted for this was used transiently (deploy calls only)
and never stored.
