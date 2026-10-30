/* VM side of the Worker relay tunnel.
 * WSS -> https://<worker>/vm through the egress proxy (manual CONNECT +
 * manual WS handshake + minimal frame codec), then forwards each request to
 * http://127.0.0.1:3000 and returns the response.
 */
const net = require('net');
const tls = require('tls');
const http = require('http');
const crypto = require('crypto');
const fs = require('fs');
const { URL } = require('url');

const WORKER_URL = (process.env.WORKER_URL || (fs.existsSync('/tmp/worker-url.txt') ? fs.readFileSync('/tmp/worker-url.txt', 'utf8') : '')).trim();
if (!WORKER_URL) {
  console.error('Set WORKER_URL env or write the URL to /tmp/worker-url.txt');
  process.exit(1);
}
const APP_HOST = '127.0.0.1';
const APP_PORT = 3000;

const proxyUrl = new URL(process.env.https_proxy || process.env.HTTPS_PROXY);
const proxyB64 = Buffer.from(
  decodeURIComponent(proxyUrl.username) + ':' + decodeURIComponent(proxyUrl.password)
).toString('base64');
const CA = fs.readFileSync('/run/hatch/egress-tls/ca-bundle.pem');

function connectProxy(host, port) {
  return new Promise((resolve, reject) => {
    const sock = net.connect({ host: proxyUrl.hostname, port: parseInt(proxyUrl.port || '3128', 10) });
    sock.on('error', reject);
    sock.on('connect', () => {
      sock.write(
        `CONNECT ${host}:${port} HTTP/1.1\r\nHost: ${host}:${port}\r\n` +
        `Proxy-Authorization: Basic ${proxyB64}\r\nConnection: keep-alive\r\n\r\n`
      );
    });
    let head = Buffer.alloc(0);
    sock.on('data', (chunk) => {
      head = Buffer.concat([head, chunk]);
      const i = head.indexOf('\r\n\r\n');
      if (i === -1) return;
      if (!/^HTTP\/1\.[01] 200/.test(head.slice(0, i).toString())) {
        sock.destroy();
        reject(new Error('proxy CONNECT failed'));
        return;
      }
      const rest = head.slice(i + 4);
      sock.removeAllListeners('data');
      sock.removeAllListeners('error');
      if (rest.length) sock.unshift(rest);
      resolve(sock);
    });
  });
}

// ---- minimal WS frame codec ----
function encodeFrame(opcode, payload) {
  const mask = crypto.randomBytes(4);
  const len = payload.length;
  let header;
  if (len < 126) {
    header = Buffer.from([0x80 | opcode, 0x80 | len]);
  } else if (len < 65536) {
    header = Buffer.from([0x80 | opcode, 0x80 | 126, (len >> 8) & 0xff, len & 0xff]);
  } else {
    header = Buffer.alloc(10);
    header[0] = 0x80 | opcode;
    header[1] = 0x80 | 127;
    header.writeBigUInt64BE(BigInt(len), 2);
  }
  const masked = Buffer.alloc(len);
  for (let i = 0; i < len; i++) masked[i] = payload[i] ^ mask[i % 4];
  return Buffer.concat([header, mask, masked]);
}

class WsConn {
  constructor(sock) {
    this.sock = sock;
    this.buf = Buffer.alloc(0);
    this.handlers = {};
    this.fragOpcode = null;
    this.fragParts = [];
    sock.on('data', (c) => this.onData(c));
    sock.on('close', () => this.emit('close'));
    sock.on('error', (e) => this.emit('error', e));
  }
  on(ev, fn) { (this.handlers[ev] = this.handlers[ev] || []).push(fn); }
  emit(ev, ...a) { (this.handlers[ev] || []).forEach((f) => f(...a)); }
  sendText(s) { this.sock.write(encodeFrame(0x1, Buffer.from(s))); }
  ping() { this.sock.write(encodeFrame(0x9, Buffer.alloc(0))); }
  onData(chunk) {
    this.buf = Buffer.concat([this.buf, chunk]);
    for (;;) {
      if (this.buf.length < 2) return;
      const b0 = this.buf[0], b1 = this.buf[1];
      const fin = (b0 & 0x80) !== 0;
      const opcode = b0 & 0x0f;
      const masked = (b1 & 0x80) !== 0;
      let len = b1 & 0x7f;
      let off = 2;
      if (len === 126) {
        if (this.buf.length < 4) return;
        len = this.buf.readUInt16BE(2); off = 4;
      } else if (len === 127) {
        if (this.buf.length < 10) return;
        len = Number(this.buf.readBigUInt64BE(2)); off = 10;
      }
      const maskKey = masked ? this.buf.slice(off, off + 4) : null;
      if (masked) off += 4;
      if (this.buf.length < off + len) return;
      let payload = this.buf.slice(off, off + len);
      this.buf = this.buf.slice(off + len);
      if (masked) {
        const u = Buffer.alloc(len);
        for (let i = 0; i < len; i++) u[i] = payload[i] ^ maskKey[i % 4];
        payload = u;
      }
      this.onFrame(fin, opcode, payload);
    }
  }
  onFrame(fin, opcode, payload) {
    if (opcode === 0x8) { this.emit('close'); this.sock.destroy(); return; }
    if (opcode === 0x9) { this.sock.write(encodeFrame(0xA, payload)); return; }
    if (opcode === 0xA) { this.emit('pong'); return; }
    if (opcode === 0x0) { // continuation
      this.fragParts.push(payload);
      if (fin) {
        const full = Buffer.concat(this.fragParts);
        const op = this.fragOpcode;
        this.fragParts = []; this.fragOpcode = null;
        if (op === 0x1) this.emit('message', full.toString('utf8'));
      }
      return;
    }
    if (!fin) { this.fragOpcode = opcode; this.fragParts = [payload]; return; }
    if (opcode === 0x1) this.emit('message', payload.toString('utf8'));
    else if (opcode === 0x2) this.emit('message', payload.toString('utf8'));
  }
}

/* Stream the app response to the Worker as it is produced: a head frame
 * {id, stream:1, status, headers} when the app sends headers, one
 * {id, stream:1, chunk:<base64>} frame per app chunk, and a closing
 * {id, stream:1, end:true}. A failure before headers falls back to the
 * legacy single-message 502 shape so any Worker build renders it safely;
 * a failure mid-stream sends {id, stream:1, error} and the Worker aborts
 * the response body. Requires the streaming Worker — deploy the Worker
 * BEFORE restarting this agent (an old Worker would mistake the head
 * frame for a complete response). Frames for concurrent requests
 * interleave on the socket and are demultiplexed by id at the Worker. */
function forwardToApp(msg, send) {
  const body = msg.body ? Buffer.from(msg.body, 'base64') : null;
  const headers = { ...msg.headers, host: `${APP_HOST}:${APP_PORT}` };
  delete headers['content-length'];
  let headSent = false;
  let done = false;
  const frame = (obj) => { try { send(JSON.stringify(obj)); } catch (e) {} };
  // Production finding (Piece 22b): body frames that reach the Worker in
  // the same burst as the head frame are lost. The Durable Object resolves
  // the client Response from the head, but the runtime has not yet wired
  // the response stream through to the client, and chunk writes landing
  // in that window are discarded — the end frame then closes an empty
  // body (live symptom: 200 with Content-Length 0). wrangler dev buffers
  // such early writes, which is why the local rehearsal passed while the
  // live path failed. Fix: hold the FIRST body frame (chunk, end, or
  // error) for a short settle window after the head, and serialize every
  // body frame through a per-request promise chain so order is exact and
  // later frames stream with no added delay. (Verified live: gates of
  // 100ms and 400ms both deliver complete bodies; 200ms ships as the
  // margin between the proven floor and generation latency.)
  const HEAD_SETTLE_MS = 200;
  let bodyChain = null;
  const bodyFrame = (obj) => {
    if (!bodyChain) {
      bodyChain = new Promise((r) => setTimeout(r, HEAD_SETTLE_MS));
    }
    bodyChain = bodyChain.then(() => frame(obj));
  };
  const fail = (message) => {
    if (done) return;
    done = true;
    if (!headSent) {
      frame({ id: msg.id, status: 502, headers: {}, body: Buffer.from('app error: ' + message).toString('base64') });
    } else {
      bodyFrame({ id: msg.id, stream: 1, error: message });
    }
  };
  const req = http.request(
    { host: APP_HOST, port: APP_PORT, path: msg.path, method: msg.method, headers },
    (res) => {
      headSent = true;
      frame({ id: msg.id, stream: 1, status: res.statusCode, headers: res.headers });
      res.on('data', (c) => bodyFrame({ id: msg.id, stream: 1, chunk: c.toString('base64') }));
      res.on('end', () => {
        if (done) return;
        done = true;
        bodyFrame({ id: msg.id, stream: 1, end: true });
      });
      res.on('error', (e) => fail(e.message));
    }
  );
  req.on('error', (e) => fail(e.message));
  // Idle timeout, not a total deadline: Node resets the socket timer on
  // activity, so a long generation survives while bytes keep flowing
  // (SSE phases/heartbeats do); only a 300s silent stall kills it.
  req.setTimeout(300000, () => { fail('app idle timeout'); req.destroy(); });
  if (body) req.write(body);
  req.end();
}

async function runOnce() {
  const u = new URL(WORKER_URL);
  const raw = await connectProxy(u.hostname, 443);
  const sock = tls.connect({
    socket: raw, servername: u.hostname, ca: [CA],
    ALPNProtocols: ['http/1.1'],
  });
  await new Promise((resolve, reject) => {
    sock.on('secureConnect', resolve);
    sock.on('error', reject);
  });
  const key = crypto.randomBytes(16).toString('base64');
  const hs =
    `GET /vm HTTP/1.1\r\nHost: ${u.hostname}\r\nUpgrade: websocket\r\n` +
    `Connection: Upgrade\r\nSec-WebSocket-Key: ${key}\r\nSec-WebSocket-Version: 13\r\n\r\n`;
  const resHead = await new Promise((resolve, reject) => {
    let buf = Buffer.alloc(0);
    const onData = (c) => {
      buf = Buffer.concat([buf, c]);
      const i = buf.indexOf('\r\n\r\n');
      if (i !== -1) {
        sock.removeListener('data', onData);
        const rest = buf.slice(i + 4);
        if (rest.length) sock.unshift(rest);
        resolve(buf.slice(0, i).toString());
      }
    };
    sock.on('data', onData);
    sock.on('error', reject);
    sock.write(hs);
  });
  if (!/^HTTP\/1\.1 101/.test(resHead)) {
    throw new Error('WS handshake failed: ' + resHead.split('\r\n')[0]);
  }
  console.error('WS tunnel established to', WORKER_URL);
  const ws = new WsConn(sock);
  // Dead-socket watchdog: the Worker replies to pings; if nothing (pong or
  // message) arrives for 45s the connection is half-dead -> reconnect.
  let lastActive = Date.now();
  ws.on('pong', () => { lastActive = Date.now(); });
  const keepalive = setInterval(() => {
    try { ws.ping(); } catch (e) {}
    if (Date.now() - lastActive > 45000) {
      console.error('no pong for 45s, dropping half-dead socket');
      try { sock.destroy(); } catch (e) {}
    }
  }, 15000);
  ws.on('message', (text) => {
    lastActive = Date.now();
    let m;
    try {
      m = JSON.parse(text);
    } catch (e) {
      return;
    }
    // Fire-and-forget: forwardToApp streams frames back as the app
    // produces them; concurrent requests interleave by id.
    forwardToApp(m, (s) => ws.sendText(s));
  });
  await new Promise((resolve) => {
    ws.on('close', resolve);
    ws.on('error', resolve);
  });
  clearInterval(keepalive);
  throw new Error('ws closed');
}

(async () => {
  for (;;) {
    try {
      await runOnce();
    } catch (e) {
      console.error('tunnel loop:', e.message, '- retrying in 3s');
    }
    await new Promise((r) => setTimeout(r, 3000));
  }
})();
