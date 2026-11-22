import { createHash, randomBytes, randomUUID } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { createServer, request } from 'node:http';
import { createRequire } from 'node:module';

const GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';
const PREVIEW_HOST = /^([0-9a-f]{32})--(?:[0-9a-f]{8}-)?([0-9a-f]+)\.localhost(?::\d+)?$/i;

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

function frame(opcode, payload) {
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

function socketFor(sock, onText, onClose) {
  let buf = Buffer.alloc(0);
  let parts = [];
  let closed = false;
  const ws = {
    send(value) {
      if (closed) return false;
      sock.write(frame(1, typeof value === 'string' ? value : JSON.stringify(value)));
      return true;
    },
    close() {
      if (closed) return;
      closed = true;
      try {
        sock.write(frame(8, Buffer.alloc(0)));
      } catch {}
      sock.end();
      setTimeout(() => sock.destroy(), 50);
      onClose();
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
        ws.close();
        return;
      }
      if (op === 9) {
        sock.write(frame(10, payload));
        continue;
      }
      if (op === 1 || op === 0) {
        parts.push(payload);
        if (fin) {
          const text = Buffer.concat(parts).toString('utf8');
          parts = [];
          onText(text);
        }
      }
    }
  });
  sock.on('close', () => {
    if (!closed) {
      closed = true;
      onClose();
    }
  });
  sock.on('error', () => {});
  return ws;
}

function json(res, status, value, headers = {}) {
  res.writeHead(status, { 'content-type': 'application/json', ...headers });
  res.end(JSON.stringify(value));
}

function bearer(req) {
  const auth = req.headers.authorization ?? '';
  return auth.startsWith('Bearer ') ? auth.slice(7) : '';
}

function readBody(req) {
  return new Promise((resolve) => {
    const chunks = [];
    req.on('data', (c) => chunks.push(c));
    req.on('end', () => resolve(Buffer.concat(chunks)));
  });
}

function relative(servedRoot, entryPath) {
  const prefix = `${servedRoot.replace(/\/$/, '')}/`;
  return entryPath.startsWith(prefix) ? entryPath.slice(prefix.length) : null;
}

function validPath(value) {
  if (!value || value.startsWith('/') || value.includes('\\')) return false;
  return !value.split('/').some((p) => !p || p === '.' || p === '..');
}

export async function fakeHub() {
  const trays = new Map();
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

  function trayByController(token) {
    for (const tray of trays.values()) if (tray.controllerToken === token) return tray;
    return null;
  }

  function previewOfHost(host) {
    const match = (host ?? '').match(PREVIEW_HOST);
    if (!match) return null;
    const compact = match[1].toLowerCase();
    const trayId = [
      compact.slice(0, 8),
      compact.slice(8, 12),
      compact.slice(12, 16),
      compact.slice(16, 20),
      compact.slice(20),
    ].join('-');
    return findPreview(`${trayId}.${match[2].toLowerCase()}`);
  }

  function bridgeTab(req, sock, refuse) {
    const found = previewOfHost(req.headers.host);
    if (!found) return refuse(404, { error: 'Preview not found' });
    const { tray, preview } = found;
    if (!preview.bridge)
      return refuse(403, { error: 'Bridge not enabled', code: 'BRIDGE_DISABLED' });
    if (preview.tabs.size >= preview.maxTabs)
      return refuse(429, { error: 'Too many bridged tabs', code: 'BRIDGE_CAP' });
    const key = req.headers['sec-websocket-key'];
    const accept = createHash('sha1')
      .update(key + GUID)
      .digest('base64');
    sock.write(
      `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`
    );
    const connId = randomUUID();
    const meta = {
      connId,
      previewToken: preview.previewToken,
      origin: req.headers.origin ?? '',
      userAgent: req.headers['user-agent'] ?? '',
      connectedAt: new Date().toISOString(),
    };
    let gone = false;
    const forget = () => {
      if (gone) return;
      gone = true;
      preview.tabs.delete(connId);
      tray.socket?.send({ type: 'bridge.disconnected', connId });
    };
    const ws = socketFor(
      sock,
      (text) => {
        let frame;
        try {
          frame = JSON.parse(text);
        } catch {
          return;
        }
        if (frame.t === 'cdp.res') {
          tray.socket?.send({
            type: 'bridge.cdp.response',
            connId,
            id: frame.id,
            result: frame.result,
            error: frame.error,
          });
        } else if (frame.t === 'emit' && text.length <= 16 * 1024 && preview.webhookId) {
          tray.socket?.send({
            type: 'webhook.event',
            webhookId: preview.webhookId,
            headers: {
              'x-slicc-preview-conn': connId,
              'x-slicc-preview-token': preview.previewToken,
            },
            body: { name: frame.name, detail: frame.detail },
            timestamp: new Date().toISOString(),
          });
        }
      },
      forget
    );
    preview.tabs.set(connId, {
      meta,
      ws,
      close() {
        forget();
        ws.close();
      },
    });
    ws.send({ t: 'welcome', connId });
    tray.socket?.send({ type: 'bridge.connected', ...meta });
  }

  function findPreview(token) {
    for (const tray of trays.values()) {
      const p = tray.previews.get(token);
      if (p) return { tray, preview: p };
    }
    return null;
  }

  function createTray(res) {
    const trayId = randomUUID();
    const tray = {
      trayId,
      controllerToken: `${trayId}.${hex(18)}`,
      joinToken: `${trayId}.${hex(18)}`,
      leader: null,
      socket: null,
      previews: new Map(),
      pending: new Map(),
      expired: false,
      messages: [],
    };
    trays.set(trayId, tray);
    json(res, 201, {
      trayId,
      createdAt: new Date().toISOString(),
      capabilities: {
        join: { token: tray.joinToken, url: `${origin}/join/${tray.joinToken}` },
        controller: {
          token: tray.controllerToken,
          url: `${origin}/controller/${tray.controllerToken}`,
        },
        webhook: { token: `${trayId}.${hex(18)}`, url: `${origin}/webhook/${trayId}.x` },
      },
    });
  }

  async function attach(req, res, token) {
    const tray = trayByController(token);
    if (!tray)
      return json(res, 403, {
        error: 'Invalid controller capability',
        code: 'INVALID_CONTROLLER_CAPABILITY',
      });
    if (tray.expired) return json(res, 410, { error: 'Tray expired', code: 'TRAY_EXPIRED' });
    const body = JSON.parse((await readBody(req)).toString() || '{}');
    const controllerId = body.controllerId ?? randomUUID();
    let role = 'follower';
    let leaderKey;
    if (!tray.leader) {
      leaderKey = hex(16);
      tray.leader = { controllerId, leaderKey };
      role = 'leader';
    } else if (body.leaderKey === tray.leader.leaderKey) {
      if (tray.socket && tray.leader.controllerId !== controllerId) {
        return json(res, 409, {
          error: 'Leader is already connected',
          code: 'LEADER_ALREADY_CONNECTED',
        });
      }
      tray.leader.controllerId = controllerId;
      leaderKey = tray.leader.leaderKey;
      role = 'leader';
    }
    json(res, 200, {
      trayId: tray.trayId,
      controllerId,
      role,
      leaderKey,
      leader: null,
      websocket:
        role === 'leader'
          ? {
              url: `ws://127.0.0.1:${port}/controller/${token}?controllerId=${encodeURIComponent(controllerId)}&leaderKey=${leaderKey}`,
            }
          : null,
    });
  }

  async function api(req, res, url) {
    const m = url.pathname.match(/^\/api\/tray\/([^/]+)\/(.+)$/);
    const tray = m && trays.get(m[1]);
    if (!tray) return json(res, 404, { error: 'Not found', code: 'TRAY_NOT_INITIALIZED' });
    const rest = m[2];
    const fileMatch = rest.match(/^preview\/([^/]+)\/(file|finalize)$/);
    if (fileMatch) {
      const preview = tray.previews.get(decodeURIComponent(fileMatch[1]));
      if (preview?.mode !== 'persistent' || preview.state !== 'pending') {
        return json(res, 404, { error: 'persistent preview is not accepting uploads' });
      }
      if (bearer(req) !== preview.uploadToken)
        return json(res, 403, { error: 'invalid upload capability' });
      if (fileMatch[2] === 'file') {
        const path = url.searchParams.get('path') ?? '';
        if (!validPath(path)) return json(res, 400, { error: 'invalid preview file path' });
        const bytes = await readBody(req);
        preview.files.set(path, {
          bytes,
          mime: req.headers['content-type'] ?? 'application/octet-stream',
        });
        res.writeHead(204);
        return res.end();
      }
      if (!preview.files.has(preview.entryRel)) {
        return json(res, 400, { error: 'persistent preview entry file was not uploaded' });
      }
      preview.state = 'ready';
      return json(res, 200, { previewToken: preview.previewToken, url: preview.url });
    }
    if (bearer(req) !== tray.controllerToken)
      return json(res, 403, { error: 'Invalid controller capability' });
    if (rest === 'preview' && req.method === 'POST') {
      const body = JSON.parse((await readBody(req)).toString() || '{}');
      const entryRel = relative(body.servedRoot, body.entryPath);
      if (!validPath(body.servedRoot?.slice(1)) || !validPath(entryRel)) {
        return json(res, 400, { error: 'invalid preview entry path' });
      }
      const secret = hex(10);
      const previewToken = `${tray.trayId}.${secret}`;
      const persistent = body.ttlMs !== undefined;
      const preview = {
        previewToken,
        servedRoot: body.servedRoot,
        entryPath: body.entryPath,
        entryRel,
        allowLive: body.allowLive,
        createdAt: new Date().toISOString(),
        url: `http://${tray.trayId.replaceAll('-', '')}--${secret}.localhost:${port}/${entryRel}`,
        mode: persistent ? 'persistent' : 'live',
        state: persistent ? 'pending' : 'ready',
        expiresAt: persistent ? new Date(Date.now() + body.ttlMs).toISOString() : undefined,
        uploadToken: persistent ? `${tray.trayId}.${hex(18)}` : undefined,
        files: new Map(),
        bridge: persistent ? false : body.bridge === true,
        maxTabs: body.maxTabs ?? 20,
        webhookId: body.webhookId,
        quiet: body.quiet ?? false,
        announced: false,
        mint: body,
        tabs: new Map(),
      };
      tray.previews.set(previewToken, preview);
      return json(res, 200, { previewToken, url: preview.url, uploadToken: preview.uploadToken });
    }
    if (rest === 'preview/stop' && req.method === 'POST') {
      const body = JSON.parse((await readBody(req)).toString() || '{}');
      if (!tray.previews.has(body.previewToken)) return json(res, 200, { revoked: false });
      const gone = tray.previews.get(body.previewToken);
      tray.previews.delete(body.previewToken);
      for (const tab of gone.tabs.values()) tab.close('preview revoked');
      tray.socket?.send({ type: 'preview.revoked', previewToken: body.previewToken });
      return json(res, 200, { revoked: true });
    }
    if (rest === 'previews' && req.method === 'GET') {
      return json(res, 200, {
        previews: [...tray.previews.values()].map((p) => ({
          previewToken: p.previewToken,
          url: p.url,
          servedRoot: p.servedRoot,
          entryPath: p.entryPath,
          allowLive: p.allowLive,
          createdAt: p.createdAt,
          mode: p.mode,
          state: p.state,
          expiresAt: p.expiresAt,
        })),
      });
    }
    return json(res, 404, { error: 'Not found' });
  }

  function asBytes(result) {
    return result.encoding === 'base64'
      ? Buffer.from(result.content, 'base64')
      : Buffer.from(result.content, 'utf8');
  }

  async function visit(req, res, url, match) {
    const compact = match[1].toLowerCase();
    const trayId = [
      compact.slice(0, 8),
      compact.slice(8, 12),
      compact.slice(12, 16),
      compact.slice(16, 20),
      compact.slice(20),
    ].join('-');
    const token = `${trayId}.${match[2].toLowerCase()}`;
    const found = findPreview(token);
    if (found?.preview.state !== 'ready') {
      res.writeHead(404);
      return res.end('Preview not found');
    }
    const { tray, preview } = found;
    if (preview.bridge && url.pathname === '/__slicc/preview-bridge.js') {
      res.writeHead(200, { 'content-type': 'text/javascript; charset=utf-8' });
      return res.end(bootstrap());
    }
    if (preview.mode === 'persistent') {
      const path =
        url.pathname === '/' ? preview.entryRel : decodeURIComponent(url.pathname.slice(1));
      const file = preview.files.get(path);
      if (!file) {
        res.writeHead(404);
        return res.end('Not found');
      }
      res.writeHead(200, { 'content-type': file.mime });
      return res.end(file.bytes);
    }
    const vfsPath =
      url.pathname === '/' ? preview.entryPath : `${preview.servedRoot}${url.pathname}`;
    const asText = /\.(html?|css|js|mjs|json|svg|txt|xml|md)$/i.test(vfsPath);
    if (!tray.socket) {
      res.writeHead(502);
      return res.end('Bad gateway: leader disconnected');
    }
    const reqId = randomUUID();
    const done = new Promise((resolve) => {
      const chunks = new Map();
      const timer = setTimeout(
        () => resolve({ ok: false, status: 504, reason: 'leader timeout' }),
        10_000
      );
      tray.pending.set(reqId, (chunk) => {
        if (!chunk.ok) {
          clearTimeout(timer);
          return resolve(chunk);
        }
        chunks.set(chunk.chunkIndex ?? 0, chunk);
        if (chunks.size === (chunk.totalChunks ?? 1)) {
          clearTimeout(timer);
          const ordered = [...chunks.keys()].sort((a, b) => a - b).map((k) => chunks.get(k));
          resolve({ ...chunk, body: Buffer.concat(ordered.map(asBytes)) });
        }
      });
    });
    const range = req.headers.range;
    tray.socket.send({
      type: 'preview.request',
      reqId,
      servedRoot: preview.servedRoot,
      vfsPath,
      asText,
      ...(range ? { range } : {}),
    });
    const result = await done;
    tray.pending.delete(reqId);
    if (!result.ok) {
      const headers = {};
      if (result.status === 416 && result.size !== undefined)
        headers['content-range'] = `bytes */${result.size}`;
      res.writeHead(result.status, headers);
      return res.end(result.reason ?? 'error');
    }
    if (result.status === 206) {
      res.writeHead(206, {
        'content-type': result.mime,
        'content-range': `bytes ${result.range.start}-${result.range.end}/${result.size}`,
        'x-encoding': result.encoding,
      });
      return res.end(result.body);
    }
    let body = result.body;
    if (preview.bridge && /^text\/html/i.test(result.mime ?? '')) {
      const script = `<script src="/__slicc/preview-bridge.js" data-slicc-token="${preview.previewToken}" data-slicc-ws="ws://${req.headers.host}/__slicc/bridge"></script>`;
      const html = body.toString('utf8');
      const at = html.search(/<head[^>]*>/i);
      body = Buffer.from(
        at === -1 ? `${script}${html}` : html.replace(/<head[^>]*>/i, (head) => `${head}${script}`)
      );
    }
    res.writeHead(200, {
      'content-type': result.mime,
      'x-encoding': result.encoding,
      'x-chunks': String(result.totalChunks),
    });
    return res.end(body);
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
    const host = req.headers.host ?? '';
    const preview = host.match(PREVIEW_HOST);
    if (preview) return visit(req, res, url, preview);
    requests.push({
      method: req.method,
      path: url.pathname,
      search: url.search,
      headers: req.headers,
    });
    const rule = injected(req.method, url.pathname, false);
    if (rule) return applyRule(res, rule);
    if (url.pathname === '/tray' && req.method === 'POST') {
      await readBody(req);
      return createTray(res);
    }
    const controller = url.pathname.match(/^\/controller\/([^/]+)$/);
    if (controller && req.method === 'POST') return attach(req, res, controller[1]);
    if (url.pathname.startsWith('/api/tray/')) return api(req, res, url);
    json(res, 404, { error: 'Not found', code: 'NOT_FOUND' });
  });

  server.on('upgrade', (req, sock) => {
    const url = new URL(req.url, origin);
    requests.push({
      method: 'GET',
      path: url.pathname,
      search: url.search,
      headers: req.headers,
      upgrade: true,
    });
    const rule = injected('GET', url.pathname, true);
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
    if (rule) return refuse(rule.status, rule.body, rule.headers);
    if (url.pathname === '/__slicc/bridge') return bridgeTab(req, sock, refuse);
    const m = url.pathname.match(/^\/controller\/([^/]+)$/);
    const tray = m && trayByController(m[1]);
    if (!tray)
      return refuse(403, {
        error: 'Invalid controller capability',
        code: 'INVALID_CONTROLLER_CAPABILITY',
      });
    if (tray.expired) return refuse(410, { error: 'Tray expired', code: 'TRAY_EXPIRED' });
    if (
      !tray.leader ||
      url.searchParams.get('leaderKey') !== tray.leader.leaderKey ||
      url.searchParams.get('controllerId') !== tray.leader.controllerId
    ) {
      return refuse(403, {
        error: 'Only the elected leader may open the tray WebSocket',
        code: 'LEADER_ONLY',
      });
    }
    const key = req.headers['sec-websocket-key'];
    const accept = createHash('sha1')
      .update(key + GUID)
      .digest('base64');
    sock.write(
      `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`
    );
    const previous = tray.socket;
    const ws = socketFor(
      sock,
      (text) => {
        let message;
        try {
          message = JSON.parse(text);
        } catch {
          return;
        }
        tray.messages.push(message.type);
        if (message.type === 'ping') ws.send({ type: 'pong', trayId: tray.trayId });
        if (message.type === 'preview.response') tray.pending.get(message.reqId)?.(message);
        if (message.type === 'preview.state.update') {
          const p = tray.previews.get(message.previewToken);
          if (p) p.announced = message.announced;
        }
        if (message.type === 'bridge.cdp.request') {
          const tab = [...tray.previews.values()]
            .map((p) => p.tabs.get(message.connId))
            .find(Boolean);
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
        }
        if (message.type === 'bridge.close') {
          for (const p of tray.previews.values()) p.tabs.get(message.connId)?.close();
        }
      },
      () => {
        if (tray.socket === ws) tray.socket = null;
      }
    );
    tray.socket = ws;
    previous?.close();
    ws.send({
      type: 'leader.connected',
      trayId: tray.trayId,
      controllerId: tray.leader.controllerId,
    });
    for (const p of tray.previews.values()) {
      if (p.mode !== 'live') continue;
      ws.send({
        type: 'preview.state',
        previewToken: p.previewToken,
        quiet: p.quiet,
        announced: p.announced,
      });
      for (const tab of p.tabs.values())
        ws.send({ type: 'bridge.connected', ...tab.meta, replay: true });
    }
  });

  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  port = server.address().port;
  origin = `http://127.0.0.1:${port}`;

  return {
    get origin() {
      return origin;
    },
    port,
    trays,
    requests,
    inject(rule) {
      const entry = { times: 1, ...rule };
      injections.push(entry);
      return entry;
    },
    tray() {
      return [...trays.values()].at(-1);
    },
    previews() {
      return [...trays.values()].flatMap((t) => [...t.previews.values()]);
    },
    hits(method, path) {
      return requests.filter(
        (r) => r.method === method && (path instanceof RegExp ? path.test(r.path) : r.path === path)
      ).length;
    },
    tab(
      previewUrl,
      { origin = 'https://visitor.example', userAgent = 'Visitor/1.0', answer } = {}
    ) {
      const u = new URL(previewUrl);
      return new Promise((resolve, reject) => {
        const r = request({
          host: '127.0.0.1',
          port,
          path: '/__slicc/bridge',
          headers: {
            host: u.host,
            origin,
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
          const ws = socketFor(
            sock,
            async (text) => {
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
            () => {
              tab.closed = true;
            }
          );
          tab.emit = (name, detail) => ws.send({ t: 'emit', name, detail });
          tab.send = (value) => ws.send(value);
          tab.close = () => ws.close();
          welcomed.then(() => resolve(tab), reject);
        });
        r.on('error', reject);
        r.end();
      });
    },
    visit(previewUrl, headers = {}) {
      const u = new URL(previewUrl);
      return new Promise((resolve, reject) => {
        const r = request(
          {
            host: '127.0.0.1',
            port,
            path: `${u.pathname}${u.search}`,
            headers: { host: u.host, ...headers },
          },
          (res) => {
            const chunks = [];
            res.on('data', (c) => chunks.push(c));
            res.on('end', () =>
              resolve({ status: res.statusCode, headers: res.headers, body: Buffer.concat(chunks) })
            );
          }
        );
        r.on('error', reject);
        r.end();
      });
    },
    close() {
      for (const tray of trays.values()) {
        for (const p of tray.previews.values())
          for (const tab of p.tabs?.values() ?? []) tab.close();
        tray.socket?.close();
      }
      server.closeAllConnections?.();
      return new Promise((resolve) => server.close(resolve));
    },
  };
}
