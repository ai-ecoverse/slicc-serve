import { createHash, randomBytes, randomUUID } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { createServer, request } from 'node:http';
import { createRequire } from 'node:module';

const GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';
const PREVIEW_HOST = /^([0-9a-f]{20}-[0-9a-f]{32})\.localhost(?::\d+)?$/;
const WINDOW = 1024 * 1024;
const CHUNK = 64 * 1024;
const HOP = new Set([
  'connection',
  'keep-alive',
  'proxy-authenticate',
  'proxy-authorization',
  'proxy-connection',
  'te',
  'trailer',
  'transfer-encoding',
  'upgrade',
]);
const PROXY = new Set(['host', 'cdn-loop', 'forwarded', 'x-real-ip', 'true-client-ip']);
const PROXY_PREFIX = /^(cf-|x-forwarded-|x-sliccy-)/;

let bridgeJs;

function bootstrap() {
  bridgeJs ??= readFileSync(
    createRequire(import.meta.url).resolve('@ai-ecoverse/cherry/preview-bridge.js')
  );
  return bridgeJs;
}

function hex(n) {
  return randomBytes(n).toString('hex');
}

function encodeFrame(opcode, payload) {
  const data = Buffer.isBuffer(payload) ? payload : Buffer.from(payload);
  let head;
  if (data.length < 126) {
    head = Buffer.from([0x80 | opcode, data.length]);
  } else if (data.length < 65536) {
    head = Buffer.alloc(4);
    head[0] = 0x80 | opcode;
    head[1] = 126;
    head.writeUInt16BE(data.length, 2);
  } else {
    head = Buffer.alloc(10);
    head[0] = 0x80 | opcode;
    head[1] = 127;
    head.writeBigUInt64BE(BigInt(data.length), 2);
  }
  return Buffer.concat([head, data]);
}

function socketFor(sock, { onText = () => {}, onBinary = () => {}, onClose = () => {} }) {
  let buf = Buffer.alloc(0);
  let parts = [];
  let partOp = 1;
  let closed = false;
  const finish = (code, reason) => {
    if (closed) return;
    closed = true;
    onClose(code, reason);
  };
  const ws = {
    send(value) {
      if (closed) return false;
      if (Buffer.isBuffer(value) || value instanceof Uint8Array)
        sock.write(encodeFrame(2, Buffer.from(value)));
      else sock.write(encodeFrame(1, typeof value === 'string' ? value : JSON.stringify(value)));
      return true;
    },
    close(code = 1000, reason = '') {
      if (closed) return;
      const body = Buffer.alloc(2 + Buffer.byteLength(reason));
      body.writeUInt16BE(code, 0);
      body.write(reason, 2);
      try {
        sock.write(encodeFrame(8, body));
      } catch {}
      sock.end();
      setTimeout(() => sock.destroy(), 50);
      finish(code, reason);
    },
    destroy() {
      sock.destroy();
      finish(1006, '');
    },
    get closed() {
      return closed;
    },
  };
  sock.on('data', (chunk) => {
    buf = Buffer.concat([buf, chunk]);
    while (buf.length >= 2) {
      const fin = (buf[0] & 0x80) !== 0;
      const op = buf[0] & 0x0f;
      const masked = (buf[1] & 0x80) !== 0;
      let len = buf[1] & 0x7f;
      let at = 2;
      if (len === 126) {
        if (buf.length < 4) return;
        len = buf.readUInt16BE(2);
        at = 4;
      } else if (len === 127) {
        if (buf.length < 10) return;
        len = Number(buf.readBigUInt64BE(2));
        at = 10;
      }
      const maskAt = at;
      if (masked) at += 4;
      if (buf.length < at + len) return;
      const payload = Buffer.from(buf.subarray(at, at + len));
      if (masked) for (let i = 0; i < len; i++) payload[i] ^= buf[maskAt + (i % 4)];
      buf = buf.subarray(at + len);
      if (op === 8) {
        const code = payload.length >= 2 ? payload.readUInt16BE(0) : 1005;
        ws.close(code === 1005 ? 1000 : code);
        return;
      }
      if (op === 9) {
        sock.write(encodeFrame(10, payload));
        continue;
      }
      if (op === 10) continue;
      if (op !== 0) partOp = op;
      parts.push(payload);
      if (!fin) continue;
      const whole = Buffer.concat(parts);
      parts = [];
      if (partOp === 1) onText(whole.toString('utf8'));
      else onBinary(whole);
    }
  });
  sock.on('close', () => finish(1006, ''));
  sock.on('error', () => {});
  return ws;
}

function accept(req, sock, protocol) {
  const key = req.headers['sec-websocket-key'];
  const digest = createHash('sha1')
    .update(key + GUID)
    .digest('base64');
  const extra = protocol ? `Sec-WebSocket-Protocol: ${protocol}\r\n` : '';
  sock.write(
    `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${digest}\r\n${extra}\r\n`
  );
}

function json(res, status, value, headers = {}) {
  res.writeHead(status, { 'content-type': 'application/json', ...headers });
  res.end(JSON.stringify(value));
}

function fail(res, status, code, error, headers = {}) {
  json(res, status, { error, code }, headers);
}

function readBody(req) {
  return new Promise((resolve) => {
    const chunks = [];
    req.on('data', (c) => chunks.push(c));
    req.on('end', () => resolve(Buffer.concat(chunks)));
  });
}

function credential(req) {
  const auth = req.headers.authorization ?? '';
  if (auth.startsWith('Bearer ')) return auth.slice(7).trim();
  const offered = (req.headers['sec-websocket-protocol'] ?? '').split(',').map((v) => v.trim());
  const bearer = offered.find((v) => v.startsWith('bearer.'));
  return bearer ? bearer.slice(7) : null;
}

function validEntry(entry) {
  return (
    typeof entry === 'string' &&
    entry.startsWith('/') &&
    entry.length <= 2048 &&
    !/[\\\u0000-\u001f\u007f]/.test(entry)
  );
}

function decodedPath(encoded) {
  let segments;
  try {
    segments = encoded.split('/').map(decodeURIComponent);
  } catch {
    return null;
  }
  if (segments.some((s) => !s || s === '.' || s === '..' || s.includes('\\') || s.includes('\0')))
    return null;
  return segments.join('/');
}

function entryFile(entry) {
  const path = entry.split(/[?#]/)[0].slice(1);
  const file = path === '' || path.endsWith('/') ? `${path}index.html` : path;
  return decodedPath(file);
}

function visitorHeaders(rawHeaders, bridged) {
  const pairs = [];
  for (let i = 0; i < rawHeaders.length; i += 2)
    pairs.push([rawHeaders[i].toLowerCase(), rawHeaders[i + 1]]);
  const listed = new Set(
    pairs
      .filter(([n]) => n === 'connection')
      .flatMap(([, v]) => v.split(','))
      .map((v) => v.trim().toLowerCase())
  );
  const out = pairs.filter(
    ([n]) =>
      !HOP.has(n) &&
      !listed.has(n) &&
      !PROXY.has(n) &&
      !PROXY_PREFIX.test(n) &&
      !(bridged && n === 'accept-encoding')
  );
  if (bridged) out.push(['accept-encoding', 'identity']);
  return out;
}

function hostOnly(cookie) {
  return cookie
    .split(';')
    .filter((part, index) => index === 0 || !/^\s*domain\s*=/i.test(part))
    .join(';');
}

function originHeaders(pairs) {
  const listed = new Set(
    pairs
      .filter(([n]) => n.toLowerCase() === 'connection')
      .flatMap(([, v]) => v.split(','))
      .map((v) => v.trim().toLowerCase())
  );
  const flat = [];
  for (const [name, value] of pairs) {
    const n = name.toLowerCase();
    if (HOP.has(n) || listed.has(n) || n === 'alt-svc' || n.startsWith('x-sliccy-')) continue;
    flat.push(n, n === 'set-cookie' ? hostOnly(value) : value);
  }
  return flat;
}

function without(flat, name) {
  const out = [];
  for (let i = 0; i < flat.length; i += 2) if (flat[i] !== name) out.push(flat[i], flat[i + 1]);
  return out;
}

function inject(html, previewToken, host) {
  const tag = `<script src="/__slicc/preview-bridge.js" data-slicc-token="${previewToken}" data-slicc-ws="ws://${host}/__slicc/bridge"></script>`;
  for (const re of [/<head[^>]*>/i, /<body[^>]*>/i]) {
    if (re.test(html)) return html.replace(re, (m) => `${m}${tag}`);
  }
  return `${html}${tag}`;
}

function frameOf(id, bytes) {
  const out = Buffer.alloc(bytes.length + 4);
  out.writeUInt32BE(id, 0);
  bytes.copy(out, 4);
  return out;
}

export async function fakeHub({ headMs = 30_000 } = {}) {
  const hosts = new Map();
  const requests = [];
  const injections = [];
  let origin = '';
  let port = 0;

  function injected(method, path, upgrade) {
    const index = injections.findIndex(
      (rule) =>
        (!rule.method || rule.method === method) &&
        (rule.path instanceof RegExp ? rule.path.test(path) : rule.path === path) &&
        Boolean(rule.upgrade) === upgrade
    );
    if (index === -1) return null;
    const rule = injections[index];
    rule.times -= 1;
    if (rule.times <= 0) injections.splice(index, 1);
    rule.hits = (rule.hits ?? 0) + 1;
    return rule;
  }

  function findPreview(token) {
    const host = hosts.get(token.slice(0, 20));
    const preview = host && !host.deleted && host.previews.get(token);
    return preview ? { host, preview } : null;
  }

  function record(p) {
    return {
      previewToken: p.previewToken,
      url: p.url,
      entry: p.entry,
      tag: p.tag,
      mode: p.mode,
      state: p.state,
      bridge: p.bridge,
      maxTabs: p.maxTabs,
      quiet: p.quiet,
      createdAt: p.createdAt,
      expiresAt: p.expiresAt,
      files: p.files.size,
      bytes: [...p.files.values()].reduce((n, f) => n + f.bytes.length, 0),
    };
  }

  function toPublisher(host, message) {
    host.socket?.send(message);
  }

  function cancelStream(host, stream, reason, status) {
    if (stream.done) return;
    stream.done = true;
    clearTimeout(stream.timer);
    host.streams.delete(stream.id);
    if (!stream.res.headersSent) {
      stream.res.writeHead(status, { 'content-type': 'text/plain' });
      stream.res.end(reason);
    } else stream.res.destroy();
  }

  function stopPreview(host, preview) {
    host.previews.delete(preview.previewToken);
    if (preview.mode !== 'live') return;
    for (const stream of [...host.streams.values()]) {
      if (stream.token !== preview.previewToken) continue;
      toPublisher(host, { type: 'http.cancel', id: stream.id, reason: 'the preview was stopped' });
      cancelStream(host, stream, 'the preview was stopped', 404);
    }
    for (const tab of preview.tabs.values()) tab.close();
    toPublisher(host, { type: 'preview.revoked', previewToken: preview.previewToken });
  }

  async function mint(req, res, host) {
    let body;
    try {
      body = JSON.parse((await readBody(req)).toString());
    } catch {
      return fail(res, 400, 'INVALID_BODY', 'the body must be a JSON object');
    }
    if (!body || typeof body !== 'object' || Array.isArray(body))
      return fail(res, 400, 'INVALID_BODY', 'the body must be a JSON object');
    const { entry = '/', tag = null, bridge = false, maxTabs = 20, quiet = false, ttlMs } = body;
    if (!validEntry(entry)) return fail(res, 400, 'INVALID_ENTRY', 'invalid entry');
    if (tag !== null && (typeof tag !== 'string' || tag.length > 256))
      return fail(res, 400, 'INVALID_TAG', 'tag must be a string of at most 256 characters');
    if (!Number.isInteger(maxTabs) || maxTabs < 1 || maxTabs > 100)
      return fail(res, 400, 'INVALID_MAX_TABS', 'maxTabs must be 1 to 100');
    const snapshot = ttlMs !== undefined;
    if (snapshot && (!Number.isSafeInteger(ttlMs) || ttlMs <= 0 || ttlMs > 30 * 86_400_000))
      return fail(res, 400, 'INVALID_TTL', 'ttlMs must be a positive duration of at most 30 days');
    if (snapshot && bridge) return fail(res, 400, 'INVALID_BODY', 'a snapshot cannot be bridged');
    const previewToken = `${host.hostId}-${hex(16)}`;
    const preview = {
      previewToken,
      url: `http://${previewToken}.localhost:${port}${entry}`,
      entry,
      tag,
      mode: snapshot ? 'snapshot' : 'live',
      state: snapshot ? 'uploading' : 'ready',
      bridge,
      maxTabs,
      quiet,
      announced: false,
      createdAt: new Date().toISOString(),
      expiresAt: null,
      ttlMs,
      files: new Map(),
      tabs: new Map(),
      mint: body,
    };
    host.previews.set(previewToken, preview);
    json(res, 201, record(preview));
  }

  async function upload(req, res, preview, encoded) {
    if (preview.mode !== 'snapshot') return fail(res, 404, 'PREVIEW_NOT_FOUND', 'not a snapshot');
    if (preview.state !== 'uploading')
      return fail(res, 409, 'SNAPSHOT_FINALIZED', 'the snapshot is finalized');
    const path = decodedPath(encoded);
    if (!path) return fail(res, 400, 'INVALID_PATH', 'invalid file path');
    const header = req.headers['content-length'];
    if (header === undefined)
      return fail(res, 411, 'LENGTH_REQUIRED', 'Content-Length is required');
    const size = Number(header);
    const old = preview.files.get(path);
    const total = [...preview.files.values()].reduce((n, f) => n + f.bytes.length, 0);
    if (size > 25 * 1024 * 1024)
      return fail(res, 413, 'FILE_TOO_LARGE', 'a file is at most 25 MiB');
    if (!old && preview.files.size + 1 > 1000)
      return fail(res, 413, 'TOO_MANY_FILES', 'a snapshot has at most 1,000 files');
    if (total - (old?.bytes.length ?? 0) + size > 50 * 1024 * 1024)
      return fail(res, 413, 'SNAPSHOT_TOO_LARGE', 'a snapshot is at most 50 MiB');
    const bytes = await readBody(req);
    preview.files.set(path, {
      bytes,
      mime: req.headers['content-type'] ?? 'application/octet-stream',
    });
    json(res, 200, { path, size: bytes.length });
  }

  function finalize(res, preview) {
    if (preview.mode !== 'snapshot') return fail(res, 404, 'PREVIEW_NOT_FOUND', 'not a snapshot');
    if (preview.state !== 'uploading')
      return fail(res, 409, 'SNAPSHOT_FINALIZED', 'the snapshot is finalized');
    const entry = entryFile(preview.entry);
    if (!preview.files.has(entry))
      return fail(res, 400, 'ENTRY_MISSING', `the entry file ${entry} was not uploaded`);
    preview.state = 'ready';
    preview.expiresAt = new Date(Date.now() + preview.ttlMs).toISOString();
    json(res, 200, record(preview));
  }

  function authorize(req, hostId) {
    const host = hosts.get(hostId);
    if (!host) return { status: 404, code: 'HOST_NOT_FOUND', error: 'no such host' };
    if (host.deleted) return { status: 410, code: 'HOST_GONE', error: 'this host was deleted' };
    if (credential(req) !== host.secret)
      return {
        status: 401,
        code: 'UNAUTHORIZED',
        error: 'wrong or missing secret',
        headers: { 'www-authenticate': 'Bearer' },
      };
    return { host };
  }

  async function api(req, res, url) {
    if (url.pathname === '/api/serve/host') {
      if (req.method !== 'POST') return fail(res, 405, 'METHOD_NOT_ALLOWED', 'method not allowed');
      await readBody(req);
      const hostId = hex(10);
      const host = {
        hostId,
        secret: hex(32),
        deleted: false,
        socket: null,
        previews: new Map(),
        streams: new Map(),
        nextId: 0,
        messages: [],
      };
      hosts.set(hostId, host);
      return json(res, 201, { hostId, secret: host.secret });
    }
    const m = url.pathname.match(/^\/api\/serve\/host\/([0-9a-f]{20})(\/.*)?$/);
    if (!m) return fail(res, 404, 'NOT_FOUND', 'not found');
    const auth = authorize(req, m[1]);
    if (!auth.host) return fail(res, auth.status, auth.code, auth.error, auth.headers);
    const { host } = auth;
    const rest = m[2] ?? '';
    if (rest === '') {
      if (req.method !== 'DELETE')
        return fail(res, 405, 'METHOD_NOT_ALLOWED', 'method not allowed');
      for (const p of [...host.previews.values()]) stopPreview(host, p);
      host.socket?.close(4001, 'host deleted');
      host.deleted = true;
      return json(res, 200, { deleted: true });
    }
    if (rest === '/previews') {
      if (req.method === 'GET')
        return json(res, 200, { previews: [...host.previews.values()].map(record) });
      if (req.method === 'POST') return mint(req, res, host);
      return fail(res, 405, 'METHOD_NOT_ALLOWED', 'method not allowed');
    }
    const pm = rest.match(/^\/previews\/([^/]+)(\/finalize|\/files\/(.+))?$/);
    const preview = pm && host.previews.get(pm[1]);
    if (!preview) return fail(res, 404, 'PREVIEW_NOT_FOUND', 'no such preview');
    if (!pm[2]) {
      if (req.method !== 'DELETE')
        return fail(res, 405, 'METHOD_NOT_ALLOWED', 'method not allowed');
      stopPreview(host, preview);
      return json(res, 200, { stopped: true });
    }
    if (pm[2] === '/finalize')
      return req.method === 'POST'
        ? finalize(res, preview)
        : fail(res, 405, 'METHOD_NOT_ALLOWED', 'no');
    return req.method === 'PUT'
      ? upload(req, res, preview, pm[3])
      : fail(res, 405, 'METHOD_NOT_ALLOWED', 'no');
  }

  function serveSnapshot(req, res, preview, url) {
    const plain = { 'content-type': 'text/plain' };
    if (req.method !== 'GET' && req.method !== 'HEAD') {
      res.writeHead(405, { ...plain, allow: 'GET, HEAD' });
      return res.end('method not allowed');
    }
    let path = decodedPath(url.pathname.slice(1) || 'index.html');
    if (url.pathname.endsWith('/')) path = decodedPath(`${url.pathname.slice(1)}index.html`);
    if (path && !preview.files.has(path) && preview.files.has(`${path}/index.html`)) {
      res.writeHead(301, { location: `${url.pathname}/${url.search}` });
      return res.end();
    }
    const file = path && preview.files.get(path);
    if (!file) {
      res.writeHead(404, plain);
      return res.end('not found');
    }
    res.writeHead(200, {
      'content-type': file.mime,
      'content-length': file.bytes.length,
      'cache-control': 'no-cache',
    });
    res.end(req.method === 'HEAD' ? undefined : file.bytes);
  }

  function serveLive(req, res, host, preview, url) {
    if (!host.socket) {
      res.writeHead(502, { 'content-type': 'text/plain' });
      return res.end('the publisher of this preview is offline');
    }
    const id = ++host.nextId;
    const hasBody =
      req.headers['transfer-encoding'] !== undefined ||
      Number(req.headers['content-length'] ?? 0) > 0;
    const stream = {
      id,
      token: preview.previewToken,
      req,
      res,
      bridged: preview.bridge,
      host: req.headers.host,
      sent: 0,
      acked: 0,
      done: false,
      html: null,
      method: req.method,
    };
    host.streams.set(id, stream);
    stream.timer = setTimeout(() => {
      toPublisher(host, {
        type: 'http.cancel',
        id,
        reason: 'the publisher did not answer in time',
      });
      cancelStream(host, stream, `the publisher didn't answer within ${headMs / 1000} s`, 504);
    }, headMs);
    res.on('close', () => {
      if (stream.done) return;
      toPublisher(host, { type: 'http.cancel', id, reason: 'the visitor went away' });
      stream.done = true;
      clearTimeout(stream.timer);
      host.streams.delete(id);
    });
    toPublisher(host, {
      type: 'http.request',
      id,
      previewToken: preview.previewToken,
      method: req.method,
      url: `http://${req.headers.host}${req.url}`,
      path: req.url,
      headers: visitorHeaders(req.rawHeaders, preview.bridge),
      body: hasBody,
    });
    if (!hasBody) return;
    stream.credit = () => {
      if (stream.sent - stream.acked < WINDOW) req.resume();
    };
    req.on('data', (chunk) => {
      if (stream.done) return;
      for (let at = 0; at < chunk.length; at += CHUNK) {
        const part = chunk.subarray(at, at + CHUNK);
        host.socket?.send(frameOf(id, part));
        stream.sent += part.length;
      }
      if (stream.sent - stream.acked >= WINDOW) req.pause();
    });
    req.on('end', () => {
      if (!stream.done) toPublisher(host, { type: 'http.request.end', id });
    });
  }

  function publisherMessage(host, ws, text) {
    if (text === 'ping') {
      host.messages.push('ping');
      ws.send('pong');
      return;
    }
    let message;
    try {
      message = JSON.parse(text);
    } catch {
      ws.close(1008, 'not JSON');
      return;
    }
    host.messages.push(message.type);
    const stream = host.streams.get(message.id);
    if (message.type === 'http.response' && stream && !stream.res.headersSent) {
      clearTimeout(stream.timer);
      const flat = originHeaders(message.headers ?? []);
      const names = flat.filter((_, i) => i % 2 === 0);
      const type = (flat[names.indexOf('content-type') * 2 + 1] ?? '').toLowerCase();
      const encoded = names.includes('content-encoding');
      stream.status = message.status;
      if (stream.bridged && message.status === 200 && type.includes('text/html') && !encoded) {
        stream.html = { flat: without(flat, 'content-length'), chunks: [] };
      } else {
        stream.res.writeHead(message.status, flat);
      }
      if (!message.body) {
        if (stream.html) stream.res.writeHead(message.status, stream.html.flat);
        stream.done = true;
        host.streams.delete(stream.id);
        stream.res.end();
      }
    } else if (message.type === 'http.response.end' && stream) {
      stream.done = true;
      host.streams.delete(stream.id);
      if (stream.html) {
        const html = inject(
          Buffer.concat(stream.html.chunks).toString('utf8'),
          stream.token,
          stream.host
        );
        stream.res.writeHead(stream.status, stream.html.flat);
        stream.res.end(html);
      } else stream.res.end();
    } else if (message.type === 'http.ack' && stream) {
      stream.acked += message.bytes;
      stream.credit?.();
    } else if (message.type === 'http.cancel' && stream) {
      cancelStream(host, stream, message.reason ?? 'cancelled', 502);
    } else if (message.type === 'preview.state.update') {
      const p = host.previews.get(message.previewToken);
      if (p) p.announced = message.announced;
    } else if (message.type === 'bridge.cdp.request') {
      const tab = [...host.previews.values()].map((p) => p.tabs.get(message.connId)).find(Boolean);
      if (!tab) {
        ws.send({
          type: 'bridge.cdp.response',
          connId: message.connId,
          id: message.id,
          error: { code: -32000, message: 'Preview bridge connection is gone' },
        });
      } else {
        tab.ws.send({
          t: 'cdp.req',
          id: message.id,
          method: message.method,
          params: message.params,
          sessionId: message.sessionId,
        });
      }
    } else if (message.type === 'bridge.close') {
      for (const p of host.previews.values()) p.tabs.get(message.connId)?.close();
    }
  }

  function publisherBody(host, data) {
    if (data.length < 5) return;
    const id = data.readUInt32BE(0);
    const stream = host.streams.get(id);
    if (!stream || (!stream.res.headersSent && !stream.html)) return;
    const part = data.subarray(4);
    if (stream.html) {
      stream.html.chunks.push(Buffer.from(part));
      host.socket?.send({ type: 'http.ack', id, bytes: part.length });
      return;
    }
    stream.received = (stream.received ?? 0) + part.length;
    stream.passed ??= 0;
    host.maxUnacked = Math.max(host.maxUnacked ?? 0, stream.received - stream.passed);
    stream.res.write(part, () => {
      stream.passed += part.length;
      const ack = { type: 'http.ack', id, bytes: part.length };
      if (host.held) host.held.push(ack);
      else host.socket?.send(ack);
    });
  }

  function publisherSocket(req, sock, host, refuse) {
    const offered = (req.headers['sec-websocket-protocol'] ?? '').split(',').map((v) => v.trim());
    accept(req, sock, offered.includes('slicc-serve.v1') ? 'slicc-serve.v1' : null);
    const previous = host.socket;
    const ws = socketFor(sock, {
      onText: (text) => publisherMessage(host, ws, text),
      onBinary: (data) => publisherBody(host, data),
      onClose: () => {
        if (host.socket !== ws) return;
        host.socket = null;
        for (const stream of [...host.streams.values()])
          cancelStream(host, stream, 'the publisher went away', 502);
      },
    });
    host.socket = ws;
    previous?.close(4000, 'replaced');
    ws.send({ type: 'hello', hostId: host.hostId });
    for (const p of host.previews.values()) {
      if (p.mode !== 'live' || !p.bridge) continue;
      ws.send({
        type: 'preview.state',
        previewToken: p.previewToken,
        quiet: p.quiet,
        announced: p.announced,
      });
      for (const tab of p.tabs.values())
        ws.send({ type: 'bridge.connected', ...tab.meta, replay: true });
    }
    return refuse;
  }

  function bridgeTab(req, sock, refuse, found) {
    if (!found) return refuse(404, 'not found');
    const { host, preview } = found;
    if (!preview.bridge || preview.mode !== 'live') return refuse(501, 'not relayed');
    if (preview.tabs.size >= preview.maxTabs)
      return refuse(429, { error: 'Too many bridged tabs', code: 'BRIDGE_CAP' });
    accept(req, sock, null);
    const connId = randomUUID();
    const meta = {
      previewToken: preview.previewToken,
      connId,
      origin: req.headers.origin ?? '',
      userAgent: req.headers['user-agent'] ?? '',
      connectedAt: new Date().toISOString(),
    };
    let gone = false;
    const forget = () => {
      if (gone) return;
      gone = true;
      preview.tabs.delete(connId);
      toPublisher(host, {
        type: 'bridge.disconnected',
        previewToken: preview.previewToken,
        connId,
      });
    };
    const ws = socketFor(sock, {
      onText: (text) => {
        if (text === 'ping') return ws.send('pong');
        let frame;
        try {
          frame = JSON.parse(text);
        } catch {
          return;
        }
        if (frame.t === 'cdp.res') {
          toPublisher(host, {
            type: 'bridge.cdp.response',
            connId,
            id: frame.id,
            result: frame.result,
            error: frame.error,
          });
        } else if (frame.t === 'emit' && text.length <= 16 * 1024) {
          toPublisher(host, {
            type: 'bridge.emit',
            previewToken: preview.previewToken,
            connId,
            name: frame.name,
            detail: frame.detail,
            at: new Date().toISOString(),
          });
        }
      },
      onClose: forget,
    });
    preview.tabs.set(connId, {
      meta,
      ws,
      close() {
        forget();
        ws.close();
      },
    });
    ws.send({ t: 'welcome', connId });
    toPublisher(host, { type: 'bridge.connected', ...meta });
  }

  async function visit(req, res, url, token) {
    const found = findPreview(token);
    if (found?.preview.state !== 'ready') {
      res.writeHead(404, { 'content-type': 'text/plain' });
      return res.end('Preview not found');
    }
    const { host, preview } = found;
    if (preview.bridge && url.pathname === '/__slicc/preview-bridge.js') {
      res.writeHead(200, { 'content-type': 'text/javascript; charset=utf-8' });
      return res.end(bootstrap());
    }
    if (preview.bridge && url.pathname === '/__slicc/emit' && req.method === 'POST') {
      const body = await readBody(req);
      try {
        const frame = JSON.parse(body.toString());
        toPublisher(host, {
          type: 'bridge.emit',
          previewToken: preview.previewToken,
          connId: null,
          name: frame.name,
          detail: frame.detail,
          at: new Date().toISOString(),
        });
      } catch {}
      res.writeHead(204);
      return res.end();
    }
    if (preview.mode === 'snapshot') return serveSnapshot(req, res, preview, url);
    serveLive(req, res, host, preview, url);
  }

  function applyRule(res, rule) {
    const headers = { ...(rule.headers ?? {}) };
    let body = rule.body ?? '';
    if (typeof body === 'object') {
      headers['content-type'] ??= 'application/json';
      body = JSON.stringify(body);
    }
    res.writeHead(rule.status, headers);
    res.end(body);
  }

  const server = createServer(async (req, res) => {
    const url = new URL(req.url, origin);
    const label = (req.headers.host ?? '').match(PREVIEW_HOST);
    if (label) return visit(req, res, url, label[1]);
    requests.push({
      method: req.method,
      path: url.pathname,
      search: url.search,
      headers: req.headers,
    });
    const rule = injected(req.method, url.pathname, false);
    if (rule) return applyRule(res, rule);
    if (url.pathname.startsWith('/api/serve/host')) return api(req, res, url);
    fail(res, 404, 'NOT_FOUND', 'not found');
  });

  server.on('upgrade', (req, sock) => {
    const url = new URL(req.url, origin);
    const refuse = (status, body, headers = {}) => {
      const text = typeof body === 'object' ? JSON.stringify(body) : (body ?? '');
      const lines = [
        `HTTP/1.1 ${status} Refused`,
        `content-length: ${Buffer.byteLength(text)}`,
        'connection: close',
      ];
      for (const [k, v] of Object.entries(headers)) lines.push(`${k}: ${v}`);
      if (typeof body === 'object') lines.push('content-type: application/json');
      sock.end(`${lines.join('\r\n')}\r\n\r\n${text}`);
    };
    const label = (req.headers.host ?? '').match(PREVIEW_HOST);
    if (label) {
      if (url.pathname === '/__slicc/bridge')
        return bridgeTab(req, sock, refuse, findPreview(label[1]));
      return refuse(501, 'WebSockets are not relayed');
    }
    requests.push({
      method: 'GET',
      path: url.pathname,
      search: url.search,
      headers: req.headers,
      upgrade: true,
    });
    const rule = injected('GET', url.pathname, true);
    if (rule) return refuse(rule.status, rule.body, rule.headers);
    const m = url.pathname.match(/^\/api\/serve\/host\/([0-9a-f]{20})\/ws$/);
    if (!m) return refuse(404, { error: 'not found', code: 'NOT_FOUND' });
    const auth = authorize(req, m[1]);
    if (!auth.host)
      return refuse(auth.status, { error: auth.error, code: auth.code }, auth.headers);
    publisherSocket(req, sock, auth.host, refuse);
  });

  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  port = server.address().port;
  origin = `http://127.0.0.1:${port}`;

  return {
    get origin() {
      return origin;
    },
    port,
    hosts,
    requests,
    inject(rule) {
      const entry = { times: 1, ...rule };
      injections.push(entry);
      return entry;
    },
    host() {
      return [...hosts.values()].at(-1);
    },
    previews() {
      return [...hosts.values()].flatMap((h) => [...h.previews.values()]);
    },
    hits(method, path) {
      return requests.filter(
        (r) => r.method === method && (path instanceof RegExp ? path.test(r.path) : r.path === path)
      ).length;
    },
    holdAcks(host) {
      host.held = [];
    },
    releaseAcks(host) {
      const held = host.held ?? [];
      host.held = null;
      for (const ack of held) host.socket?.send(ack);
    },
    async createHost() {
      const res = await fetch(`${origin}/api/serve/host`, { method: 'POST' });
      return res.json();
    },
    tab(
      previewUrl,
      { origin: from = 'https://visitor.example', userAgent = 'Visitor/1.0', answer } = {}
    ) {
      const u = new URL(previewUrl);
      return new Promise((resolve, reject) => {
        const r = request({
          host: '127.0.0.1',
          port,
          path: '/__slicc/bridge',
          headers: {
            host: u.host,
            origin: from,
            'user-agent': userAgent,
            connection: 'Upgrade',
            upgrade: 'websocket',
            'sec-websocket-version': '13',
            'sec-websocket-key': randomBytes(16).toString('base64'),
          },
        });
        r.on('response', (res) => {
          const chunks = [];
          res.on('data', (c) => chunks.push(c));
          res.on('end', () =>
            resolve({ refused: res.statusCode, body: Buffer.concat(chunks).toString() })
          );
        });
        r.on('upgrade', (_res, sock, head) => {
          if (head?.length) sock.unshift(head);
          const tab = { requests: [], closed: false, connId: null };
          let ready;
          const welcomed = new Promise((r2) => {
            ready = r2;
          });
          const ws = socketFor(sock, {
            onText: async (text) => {
              const frame = JSON.parse(text);
              if (frame.t === 'welcome') {
                tab.connId = frame.connId;
                ready();
                return;
              }
              if (frame.t !== 'cdp.req') return;
              tab.requests.push(frame);
              const reply = answer ? await answer(frame) : { result: {} };
              if (reply === null) return;
              ws.send({ t: 'cdp.res', id: frame.id, ...reply });
            },
            onClose: () => {
              tab.closed = true;
            },
          });
          tab.emit = (name, detail) => ws.send({ t: 'emit', name, detail });
          tab.send = (value) => ws.send(value);
          tab.close = () => ws.close();
          welcomed.then(() => resolve(tab), reject);
        });
        r.on('error', reject);
        r.end();
      });
    },
    visit(previewUrl, headers = {}, { method = 'GET', body } = {}) {
      const u = new URL(previewUrl);
      return new Promise((resolve, reject) => {
        const r = request(
          {
            host: '127.0.0.1',
            port,
            method,
            path: `${u.pathname}${u.search}`,
            headers: { host: u.host, ...headers },
          },
          (res) => {
            const chunks = [];
            res.on('data', (c) => chunks.push(c));
            res.on('end', () =>
              resolve({
                status: res.statusCode,
                headers: res.headers,
                rawHeaders: res.rawHeaders,
                body: Buffer.concat(chunks),
              })
            );
            res.on('error', (error) =>
              resolve({
                status: res.statusCode,
                headers: res.headers,
                body: Buffer.concat(chunks),
                error,
              })
            );
          }
        );
        r.on('error', reject);
        if (body !== undefined) r.end(body);
        else r.end();
      });
    },
    close() {
      for (const host of hosts.values()) {
        for (const p of host.previews.values()) for (const tab of p.tabs.values()) tab.close();
        host.socket?.close();
      }
      server.closeAllConnections?.();
      return new Promise((resolve) => server.close(resolve));
    },
  };
}
