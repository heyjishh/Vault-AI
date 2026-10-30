/* legal-rag-relay: reverse tunnel over WebSocket, backed by a Durable Object
 * so the VM's socket is visible to every request isolate.
 *
 * Response protocol (VM agent -> Worker), per request id:
 *  - Streaming (current agent): a head frame {id, stream:1, status, headers}
 *    as soon as the app sends response headers, then one
 *    {id, stream:1, chunk:<base64>} frame per app chunk, then
 *    {id, stream:1, end:true}. A mid-stream failure arrives as
 *    {id, stream:1, error:<message>} and aborts the response body.
 *    The Worker resolves the client Response on the head frame and pipes
 *    chunk frames through a TransformStream, so bytes reach the browser as
 *    the app produces them.
 *  - Legacy (previous agent): a single {id, status, headers, body:<base64>}
 *    message with the whole response. Still accepted, so a new Worker can
 *    be deployed before the agent is restarted (never the other way round:
 *    an old Worker would mistake a head frame for a complete response).
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

function cleanHeaders(headers) {
  const out = {};
  for (const [k, v] of Object.entries(headers || {})) {
    const lk = k.toLowerCase();
    if (lk === 'connection' || lk === 'transfer-encoding' || lk === 'content-length') continue;
    out[lk] = v;
  }
  return out;
}

const FIRST_FRAME_TIMEOUT_MS = 55000;
const STREAM_IDLE_TIMEOUT_MS = 300000;

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
      for (const [, entry] of this.pending) entry.drop();
      this.pending.clear();
      server.addEventListener('message', (ev) => {
        try {
          const msg = JSON.parse(ev.data);
          const entry = this.pending.get(msg.id);
          if (entry) entry.feed(msg);
        } catch (e) {}
      });
      const drop = () => {
        if (this.vmSocket === server) this.vmSocket = null;
        for (const [, entry] of this.pending) entry.drop();
        this.pending.clear();
      };
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

    // The request resolves on its FIRST response frame: a legacy whole-body
    // message, or a stream head (chunk frames then feed the body stream).
    const first = await new Promise((resolve) => {
      const entry = {
        started: false,
        writer: null,
        idleTimer: null,
        finish: () => {
          if (entry.idleTimer) clearTimeout(entry.idleTimer);
          if (this.pending.get(id) === entry) this.pending.delete(id);
        },
        armIdle: (ms) => {
          if (entry.idleTimer) clearTimeout(entry.idleTimer);
          entry.idleTimer = setTimeout(() => {
            if (!entry.started) {
              entry.finish();
              resolve(null);
            } else {
              const w = entry.writer;
              entry.finish();
              if (w) w.abort(new Error('tunnel idle timeout')).catch(() => {});
            }
          }, ms);
        },
        drop: () => {
          if (!entry.started) {
            entry.finish();
            resolve(null);
          } else {
            const w = entry.writer;
            entry.finish();
            if (w) w.abort(new Error('vm disconnected')).catch(() => {});
          }
        },
        feed: (msg) => {
          if (!msg.stream) {
            // Legacy single-message response from a previous agent build.
            entry.finish();
            resolve(msg);
            return;
          }
          if (!entry.started) {
            if (msg.error) {
              entry.finish();
              resolve(null);
              return;
            }
            entry.started = true;
            const ts = new TransformStream();
            entry.writer = ts.writable.getWriter();
            entry.armIdle(STREAM_IDLE_TIMEOUT_MS);
            resolve({ streamed: true, status: msg.status || 200, headers: msg.headers, readable: ts.readable });
            return;
          }
          if (msg.chunk) {
            entry.armIdle(STREAM_IDLE_TIMEOUT_MS);
            entry.writer.write(b64decode(msg.chunk)).catch(() => entry.finish());
          } else if (msg.end) {
            const w = entry.writer;
            entry.finish();
            if (w) w.close().catch(() => {});
          } else if (msg.error) {
            const w = entry.writer;
            entry.finish();
            if (w) w.abort(new Error(msg.error)).catch(() => {});
          }
        },
      };
      this.pending.set(id, entry);
      entry.armIdle(FIRST_FRAME_TIMEOUT_MS);
      try {
        this.vmSocket.send(JSON.stringify({ id, method: req.method, path: url.pathname + url.search, headers, body: bodyB64 }));
      } catch (e) {
        entry.drop();
      }
    });

    if (!first) return new Response('VM did not respond', { status: 504 });

    if (first.streamed) {
      return new Response(first.readable, {
        status: first.status,
        headers: cleanHeaders(first.headers),
      });
    }

    return new Response(first.body ? b64decode(first.body) : null, {
      status: first.status || 200,
      headers: cleanHeaders(first.headers),
    });
  }
}

export default {
  async fetch(req, env) {
    const id = env.RELAY.idFromName('main');
    return env.RELAY.get(id).fetch(req);
  },
};
