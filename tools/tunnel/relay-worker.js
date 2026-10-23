/* legal-rag-relay: reverse tunnel over WebSocket, backed by a Durable Object
 * so the VM's socket is visible to every request isolate.
 */
function b64encode(buf) {
  const u8 = new Uint8Array(buf);
  let s = '';
  for (let i = 0; i < u8.length; i += 0x8000) {
    s += String.fromCharCode.apply(null, u8.subarray(i, i + 0x8000));
  }
  return btoa(s);
}

function b64decode(s) {
  const bin = atob(s);
  const u8 = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) u8[i] = bin.charCodeAt(i);
  return u8.buffer;
}

export class Relay {
  constructor(state, env) {
    this.state = state;
    this.vmSocket = null;
    this.pending = new Map();
  }

  async fetch(req) {
    const url = new URL(req.url);

    if (url.pathname === '/vm') {
      if (req.headers.get('Upgrade') !== 'websocket') {
        return new Response('expected websocket', { status: 426 });
      }
      const [client, server] = new WebSocketPair();
      server.accept();
      if (this.vmSocket) { try { this.vmSocket.close(); } catch (e) {} }
      this.vmSocket = server;
      // fail any in-flight requests from the previous socket
      for (const [, p] of this.pending) p(null);
      this.pending.clear();
      server.addEventListener('message', (ev) => {
        try {
          const msg = JSON.parse(ev.data);
          const p = this.pending.get(msg.id);
          if (p) { this.pending.delete(msg.id); p(msg); }
        } catch (e) {}
      });
      const drop = () => { if (this.vmSocket === server) this.vmSocket = null; };
      server.addEventListener('close', drop);
      server.addEventListener('error', drop);
      return new Response(null, { status: 101, webSocket: client });
    }

    if (url.pathname === '/healthz') {
      return new Response(this.vmSocket ? 'vm-connected' : 'vm-missing', {
        status: this.vmSocket ? 200 : 503,
      });
    }

    if (!this.vmSocket) {
      return new Response('tunnel VM not connected', { status: 503 });
    }

    const id = crypto.randomUUID();
    let bodyB64 = null;
    if (req.body) bodyB64 = b64encode(await req.arrayBuffer());
    const headers = {};
    for (const [k, v] of req.headers) {
      const lk = k.toLowerCase();
      if (lk === 'host' || lk === 'connection' || lk === 'content-length') continue;
      headers[lk] = v;
    }

    const resp = await new Promise((resolve) => {
      const to = setTimeout(() => { this.pending.delete(id); resolve(null); }, 55000);
      this.pending.set(id, (m) => { clearTimeout(to); resolve(m); });
      try {
        this.vmSocket.send(JSON.stringify({ id, method: req.method, path: url.pathname + url.search, headers, body: bodyB64 }));
      } catch (e) {
        clearTimeout(to); this.pending.delete(id); resolve(null);
      }
    });

    if (!resp) return new Response('VM did not respond', { status: 504 });

    const resHeaders = {};
    for (const [k, v] of Object.entries(resp.headers || {})) {
      const lk = k.toLowerCase();
      if (lk === 'connection' || lk === 'transfer-encoding' || lk === 'content-length') continue;
      resHeaders[lk] = v;
    }
    return new Response(resp.body ? b64decode(resp.body) : null, {
      status: resp.status || 200,
      headers: resHeaders,
    });
  }
}

export default {
  async fetch(req, env) {
    const id = env.RELAY.idFromName('main');
    return env.RELAY.get(id).fetch(req);
  },
};
