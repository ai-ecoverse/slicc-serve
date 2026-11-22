import assert from 'node:assert/strict';
import { mkdirSync, statSync, truncateSync, writeFileSync } from 'node:fs';
import { createServer, request } from 'node:http';
import { createServer as createTcp } from 'node:net';
import { join } from 'node:path';
import { after, test } from 'node:test';
import { fakeHub } from './fake-hub.mjs';
import { alive, eventually, sandbox } from './helpers.mjs';

const hub = await fakeHub();
after(() => hub.close());

function previewOf(url) {
  return hub.previews().find((p) => p.url.startsWith(new URL(url).origin));
}

const TOKEN_URL = /^http:\/\/[0-9a-f]{20}-[0-9a-f]{32}\.localhost:\d+\/$/;

async function listen(handler) {
  const app = createServer(handler);
  await new Promise((r) => app.listen(0, '127.0.0.1', r));
  after(() => app.close());
  return app;
}

function hostOf(box) {
  return hub.hosts.get(box.host().hostId);
}

test('scserve DIR publishes a live preview and returns', async () => {
  const box = sandbox(hub);
  const site = box.site({
    'index.html': '<h1>hello</h1>\n',
    'app.js': 'console.log(1)\n',
    'img.bin': Buffer.from([0, 1, 2, 255]),
    'sub/index.html': '<p>sub</p>',
    '.env': 'SECRET=1',
  });
  const res = await box.run([site]);
  assert.equal(res.code, 0, res.stderr);
  const url = res.stdout.trim();
  assert.match(url, TOKEN_URL);
  const token = previewOf(url).previewToken;
  assert.ok(url.startsWith(`http://${token}.localhost:`));
  assert.match(
    res.stderr,
    /site is public at .* \(here: http:\/\/127\.0\.0\.1:\d+\/\)\nscserve: stop it with: scserve --stop /
  );
  assert.ok(res.stderr.endsWith(`--stop ${token}\n`));

  const page = await hub.visit(url);
  assert.equal(page.status, 200);
  assert.equal(page.body.toString(), '<h1>hello</h1>\n');
  assert.equal(page.headers['content-type'], 'text/html; charset=utf-8');
  assert.equal(page.headers['cache-control'], 'no-cache');
  const js = await hub.visit(new URL('app.js', url).href);
  assert.equal(js.body.toString(), 'console.log(1)\n');
  const bin = await hub.visit(new URL('img.bin', url).href);
  assert.deepEqual([...bin.body], [0, 1, 2, 255]);
  const dir = await hub.visit(new URL('sub?x=1', url).href);
  assert.equal(dir.status, 301);
  assert.equal(dir.headers.location, '/sub/?x=1');
  assert.equal((await hub.visit(new URL('sub/', url).href)).body.toString(), '<p>sub</p>');
  assert.equal((await hub.visit(new URL('nope.html', url).href)).status, 404);
  assert.equal((await hub.visit(new URL('.env', url).href)).status, 404);
  const post = await hub.visit(
    url,
    { 'content-type': 'text/plain' },
    { method: 'POST', body: 'x' }
  );
  assert.equal(post.status, 405);
  assert.equal(post.headers.allow, 'GET, HEAD');
  const head = await hub.visit(url, {}, { method: 'HEAD' });
  assert.equal(head.status, 200);
  assert.equal(head.headers['content-length'], '15');

  const p = previewOf(url);
  assert.match(p.tag, /^\/srv\/serve\/[0-9a-f]{16}$/);
  assert.equal(p.entry, '/');
  assert.equal(p.mode, 'live');
  assert.deepEqual(Object.keys(p.mint).sort(), ['entry', 'tag']);

  const relay = box.relay();
  assert.ok(alive(relay.pid));
  const stop = await box.run(['--stop', url]);
  assert.equal(stop.code, 0, stop.stderr);
  assert.equal(stop.stdout, `stopped ${token}\n`);
  assert.equal(previewOf(url), undefined);
  await eventually(() => !alive(relay.pid), 15_000);
  assert.equal(box.relay(), null);
});

test('the relay streams ranges and large files, holding to the window', async () => {
  const box = sandbox(hub);
  const site = box.site({ 'index.html': 'x', 'ten.txt': '0123456789', 'huge.bin': '' });
  truncateSync(join(site, 'huge.bin'), 30 * 1024 * 1024);
  const res = await box.run([site]);
  const url = res.stdout.trim();
  const part = await hub.visit(new URL('ten.txt', url).href, { range: 'bytes=2-5' });
  assert.equal(part.status, 206);
  assert.equal(part.body.toString(), '2345');
  assert.equal(part.headers['content-range'], 'bytes 2-5/10');
  const none = await hub.visit(new URL('ten.txt', url).href, { range: 'bytes=20-' });
  assert.equal(none.status, 416);
  assert.equal(none.headers['content-range'], 'bytes */10');
  const huge = await hub.visit(new URL('huge.bin', url).href);
  assert.equal(huge.status, 200);
  assert.equal(huge.body.length, 30 * 1024 * 1024);
  assert.equal(huge.headers['content-length'], String(30 * 1024 * 1024));
  const host = hostOf(box);
  assert.ok(host.maxUnacked > 0 && host.maxUnacked <= 1024 * 1024, String(host.maxUnacked));
  hub.holdAcks(host);
  const held = hub.visit(new URL('huge.bin', url).href);
  await new Promise((r) => setTimeout(r, 500));
  const stream = [...host.streams.values()].at(-1);
  assert.ok(
    stream.received > 960 * 1024 && stream.received <= 1024 * 1024,
    String(stream.received)
  );
  hub.releaseAcks(host);
  assert.equal((await held).body.length, 30 * 1024 * 1024);
});

test('a second scserve reuses the saved host and the running relay', async () => {
  const box = sandbox(hub);
  const first = await box.run([box.site({ 'index.html': 'one' }, 'one')]);
  const relay = box.relay();
  const second = await box.run([box.site({ 'index.html': 'two' }, 'two')]);
  assert.equal(first.code, 0);
  assert.equal(second.code, 0);
  assert.equal(box.relay().pid, relay.pid);
  const host = box.host();
  assert.equal(host.hub, hub.origin);
  assert.match(host.secret, /^[0-9a-f]{64}$/);
  assert.ok(
    first.stdout.includes(`//${host.hostId}-`) && second.stdout.includes(`//${host.hostId}-`)
  );
  assert.equal((await hub.visit(first.stdout.trim())).body.toString(), 'one');
  assert.equal((await hub.visit(second.stdout.trim())).body.toString(), 'two');
  if (process.platform !== 'win32') {
    assert.equal(statSync(join(box.configDir, 'host.json')).mode & 0o777, 0o600);
    assert.equal(statSync(join(box.stateDir, 'relay.json')).mode & 0o777, 0o600);
  }
  const auth = hub.requests.filter((r) => r.path.startsWith(`/api/serve/host/${host.hostId}`));
  assert.ok(auth.length > 0);
  assert.ok(auth.every((r) => r.headers.authorization === `Bearer ${host.secret}`));
  const list = JSON.parse((await box.run(['--list', '--json'])).stdout);
  assert.equal(list.length, 2);
  assert.ok(list.every((r) => r.mode === 'live' && r.localUrl.startsWith('http://127.0.0.1:')));
  const table = (await box.run(['--list'])).stdout;
  assert.match(table, /^TOKEN\s+MODE\s+EXPIRES\s+URL\s+ROOT\n/);
  assert.equal(table.trim().split('\n').length, 3);
});

test('--port relays the whole request: method, query, headers and body, and the whole answer', async () => {
  const box = sandbox(hub);
  const seen = [];
  const app = await listen(async (req, res) => {
    const chunks = [];
    for await (const c of req) chunks.push(c);
    const body = Buffer.concat(chunks);
    seen.push({ method: req.method, url: req.url, headers: req.headers, body });
    if (req.url === '/old') {
      res.writeHead(302, { location: `http://127.0.0.1:${app.address().port}/new?from=old` });
      return res.end();
    }
    if (req.url === '/net') {
      res.writeHead(303, { location: `//localhost:${app.address().port}/login` });
      return res.end();
    }
    if (req.url === '/away') {
      res.writeHead(301, { location: 'https://example.com/elsewhere' });
      return res.end();
    }
    if (req.url === '/chunked') {
      res.writeHead(200, { 'content-type': 'text/plain' });
      res.write('one,');
      setTimeout(() => res.end('two'), 50);
      return;
    }
    if (req.url === '/missing') {
      res.writeHead(404, { 'content-type': 'text/html', 'x-app': 'yes' });
      return res.end('<p>gone</p>');
    }
    res.setHeader('set-cookie', ['a=1; Path=/', 'b=2; Domain=sliccy.now; Path=/']);
    res.setHeader('cache-control', 'private, max-age=60');
    res.writeHead(201, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ method: req.method, url: req.url, bytes: body.length }));
  });
  const port = app.address().port;
  const res = await box.run(['--port', String(port), '--json']);
  assert.equal(res.code, 0, res.stderr);
  const out = JSON.parse(res.stdout);
  assert.match(out.url, TOKEN_URL);
  assert.equal(out.port, port);
  assert.equal(out.localUrl, `http://127.0.0.1:${port}/`);
  assert.equal(out.hostId, box.host().hostId);

  const big = Buffer.alloc(3 * 1024 * 1024 + 17, 9);
  const post = await hub.visit(
    new URL('search?q=a%20b&x=1', out.url).href,
    { 'content-type': 'application/octet-stream', cookie: 'sid=42', 'x-custom': 'v' },
    { method: 'POST', body: big }
  );
  assert.equal(post.status, 201);
  assert.deepEqual(JSON.parse(post.body.toString()), {
    method: 'POST',
    url: '/search?q=a%20b&x=1',
    bytes: big.length,
  });
  assert.deepEqual(post.headers['set-cookie'], ['a=1; Path=/', 'b=2; Path=/']);
  assert.equal(post.headers['cache-control'], 'private, max-age=60');
  const got = seen.at(-1);
  assert.equal(got.headers.host, `127.0.0.1:${port}`);
  assert.equal(got.headers['x-forwarded-host'], new URL(out.url).host);
  assert.equal(got.headers['x-forwarded-proto'], 'http');
  assert.equal(got.headers.cookie, 'sid=42');
  assert.equal(got.headers['x-custom'], 'v');
  assert.ok(got.body.equals(big));

  const streamed = await hub.visit(
    new URL('upload', out.url).href,
    { 'transfer-encoding': 'chunked' },
    { method: 'PUT', body: 'streamed body' }
  );
  assert.equal(JSON.parse(streamed.body.toString()).bytes, 13);
  assert.equal(seen.at(-1).body.toString(), 'streamed body');

  const old = await hub.visit(new URL('old', out.url).href);
  assert.equal(old.status, 302);
  assert.equal(old.headers.location, '/new?from=old');
  const net = await hub.visit(new URL('net', out.url).href);
  assert.equal(net.status, 303);
  assert.equal(net.headers.location, '/login');
  const away = await hub.visit(new URL('away', out.url).href);
  assert.equal(away.status, 301);
  assert.equal(away.headers.location, 'https://example.com/elsewhere');
  assert.ok(!seen.some((s) => s.url.startsWith('/new')));

  const missing = await hub.visit(new URL('missing', out.url).href);
  assert.equal(missing.status, 404);
  assert.equal(missing.headers['content-type'], 'text/html');
  assert.equal(missing.headers['x-app'], 'yes');
  assert.equal(missing.body.toString(), '<p>gone</p>');
  const chunked = await hub.visit(new URL('chunked', out.url).href);
  assert.equal(chunked.body.toString(), 'one,two');

  app.close();
  app.closeAllConnections();
  await new Promise((r) => setTimeout(r, 100));
  const down = await hub.visit(out.url);
  assert.equal(down.status, 502);
  assert.match(down.body.toString(), new RegExp(`nothing answers on 127\\.0\\.0\\.1:${port}`));
});

test('a visitor that goes away cancels the stream, and the origin is let go', async () => {
  const box = sandbox(hub);
  let closed = false;
  const app = await listen((req, res) => {
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    const timer = setInterval(() => res.write('data: tick\n\n'), 20);
    res.on('close', () => {
      clearInterval(timer);
      closed = true;
    });
  });
  const res = await box.run(['--port', String(app.address().port)]);
  const url = new URL(res.stdout.trim());
  await new Promise((resolve, reject) => {
    const r = request(
      { host: '127.0.0.1', port: hub.port, path: '/events', headers: { host: url.host } },
      (answer) => {
        answer.once('data', () => {
          r.destroy();
          resolve();
        });
      }
    );
    r.on('error', reject);
    r.end();
  });
  await eventually(() => closed, 10_000);
  assert.ok(hostOf(box).messages.includes('http.response'));
});

test('--port with --entry keeps the entry in the URL', async () => {
  const box = sandbox(hub);
  const app = await listen((req, res) => res.end(req.url));
  const res = await box.run([
    '--port',
    String(app.address().port),
    '--entry',
    'docs/start page.html',
  ]);
  assert.equal(res.code, 0, res.stderr);
  const url = res.stdout.trim();
  assert.match(url, /\/docs\/start%20page\.html$/);
  assert.equal(previewOf(url).entry, '/docs/start%20page.html');
  assert.equal((await hub.visit(url)).body.toString(), '/docs/start%20page.html');
  assert.equal((await hub.visit(new URL('/', url).href)).body.toString(), '/');
});

test('--foreground stays attached, logs visits, and stops the preview when it ends', async () => {
  const box = sandbox(hub);
  const proc = box.start(['--foreground', box.site({ 'index.html': 'fg' })]);
  const [url] = await proc.stdout(/^http:\/\/\S+$/m);
  await proc.stderr(/is public at .*; Ctrl-C stops it/);
  assert.equal((await hub.visit(`${url}?a=1`)).body.toString(), 'fg');
  await proc.stderr(/scserve: GET \/\?a=1 200 2/);
  const list = JSON.parse((await box.run(['--list', '--json'])).stdout);
  assert.equal(list[0].foreground, true);
  assert.ok(previewOf(url));
  proc.child.kill();
  await proc.exited;
  await eventually(() => !previewOf(url));
  assert.equal((await hub.visit(url)).status, 404);
});

test('--stop from elsewhere ends a --foreground scserve', async () => {
  const box = sandbox(hub);
  const proc = box.start(['--foreground', box.site()]);
  const [url] = await proc.stdout(/^http:\/\/\S+$/m);
  const stop = await box.run(['--stop', url]);
  assert.equal(stop.code, 0, stop.stderr);
  const { code } = await proc.exited;
  assert.equal(code, 0);
  assert.match(proc.err, /scserve: stopped: stopped on the serve hub\n$/);
});

test('the relay reconnects when the hub drops its socket, and pings it', async () => {
  const box = sandbox(hub);
  const res = await box.run([box.site({ 'index.html': 'again' })]);
  const url = res.stdout.trim();
  const host = hostOf(box);
  const before = hub.hits('GET', `/api/serve/host/${host.hostId}/ws`);
  const first = host.socket;
  first.destroy();
  await eventually(
    () =>
      hub.hits('GET', `/api/serve/host/${host.hostId}/ws`) > before &&
      host.socket &&
      host.socket !== first,
    15_000
  );
  assert.equal((await hub.visit(url)).body.toString(), 'again');
  const ws = hub.requests.filter((r) => r.upgrade).at(-1);
  assert.equal(ws.headers.authorization, `Bearer ${host.secret}`);
});

test('the relay stops a preview stopped through the API', async () => {
  const box = sandbox(hub);
  const res = await box.run([box.site()]);
  const url = res.stdout.trim();
  const host = hostOf(box);
  const p = previewOf(url);
  const relay = box.relay();
  await fetch(`${hub.origin}/api/serve/host/${host.hostId}/previews/${p.previewToken}`, {
    method: 'DELETE',
    headers: { authorization: `Bearer ${host.secret}` },
  });
  await eventually(() => !alive(relay.pid), 15_000);
  assert.equal((await box.run(['--list'])).stdout, 'no previews\n');
});

test('deleting the host (close 4001) stops its previews and forgets it', async () => {
  const box = sandbox(hub);
  const res = await box.run([box.site()]);
  assert.equal(res.code, 0, res.stderr);
  const host = hostOf(box);
  const relay = box.relay();
  await fetch(`${hub.origin}/api/serve/host/${host.hostId}`, {
    method: 'DELETE',
    headers: { authorization: `Bearer ${host.secret}` },
  });
  await eventually(() => !alive(relay.pid), 15_000);
  assert.equal(box.host(), null);
});

test('another socket for the host (close 4000) takes over its previews', async () => {
  const box = sandbox(hub);
  const proc = box.start(['--foreground', box.site()]);
  await proc.stdout(/^http:\/\/\S+$/m);
  const host = box.host();
  const other = new WebSocket(`ws://127.0.0.1:${hub.port}/api/serve/host/${host.hostId}/ws`, [
    'slicc-serve.v1',
    `bearer.${host.secret}`,
  ]);
  const hello = await new Promise((resolve, reject) => {
    other.onmessage = (e) => resolve(JSON.parse(e.data));
    other.onerror = reject;
  });
  assert.deepEqual(hello, { type: 'hello', hostId: host.hostId });
  assert.equal(other.protocol, 'slicc-serve.v1');
  const { code } = await proc.exited;
  assert.equal(code, 0);
  assert.match(proc.err, /scserve: stopped: another scserve took over this machine's host\n$/);
  other.close();
});

test('a starting relay stops live previews nothing serves any more, by tag', async () => {
  const box = sandbox(hub);
  const saved = await box.saveHost(hub);
  const mint = (body) =>
    fetch(`${hub.origin}/api/serve/host/${saved.hostId}/previews`, {
      method: 'POST',
      headers: { authorization: `Bearer ${saved.secret}`, 'content-type': 'application/json' },
      body: JSON.stringify(body),
    }).then((r) => r.json());
  const stale = await mint({ tag: '/srv/serve/dead' });
  const snapshot = await mint({ tag: '/srv/serve/kept', ttlMs: 60_000 });
  const res = await box.run([box.site()]);
  assert.equal(res.code, 0, res.stderr);
  await eventually(() => !hub.previews().some((p) => p.previewToken === stale.previewToken));
  assert.ok(previewOf(res.stdout.trim()));
  assert.ok(hub.previews().some((p) => p.previewToken === snapshot.previewToken));
});

test('--list drops a host whose secret the hub refuses and exits 3', async () => {
  const box = sandbox(hub);
  await box.saveHost(hub);
  hub.inject({ method: 'GET', path: /\/previews$/, status: 401, body: { error: 'sign in' } });
  const res = await box.run(['--list']);
  assert.equal(res.code, 3);
  assert.match(
    res.stderr,
    /^scserve: listing previews: the serve hub refused it \(401: sign in\)\n$/
  );
  assert.equal(box.host(), null);
});

test("a relay of an older scserve for six's hub is named, not used", async () => {
  const box = sandbox(hub);
  const old = createTcp((sock) => {
    sock.once('data', () =>
      sock.end(`${JSON.stringify({ ok: true, pid: 4242, hub: 'https://www.sliccy.ai' })}\n`)
    );
  });
  await new Promise((r) => old.listen(0, '127.0.0.1', r));
  after(() => old.close());
  mkdirSync(box.stateDir, { recursive: true });
  writeFileSync(
    join(box.stateDir, 'relay.json'),
    JSON.stringify({ pid: 4242, port: old.address().port, token: 't' })
  );
  const res = await box.run([box.site()]);
  assert.equal(res.code, 1);
  assert.equal(
    res.stderr,
    'scserve: a relay for https://www.sliccy.ai (pid 4242, likely an older scserve) still runs; its previews end when it stops (kill 4242)\n'
  );
  writeFileSync(join(box.stateDir, 'relay.json'), '{}');
});

test('an origin that answers a large upload early reaches the visitor at once', async () => {
  const box = sandbox(hub);
  const early = createTcp((sock) => {
    let head = '';
    sock.on('data', (chunk) => {
      if (head.includes('\r\n\r\n')) return;
      head += chunk.toString('latin1');
      if (!head.includes('\r\n\r\n')) return;
      sock.pause();
      sock.write(
        'HTTP/1.1 413 Too Large\r\ncontent-length: 8\r\nconnection: close\r\n\r\ntoo big\n'
      );
      setTimeout(() => sock.destroy(), 5_000).unref();
    });
  });
  await new Promise((r) => early.listen(0, '127.0.0.1', r));
  after(() => early.close());
  const res = await box.run(['--port', String(early.address().port)]);
  assert.equal(res.code, 0, res.stderr);
  const started = Date.now();
  const answer = await hub.visit(
    new URL('upload', res.stdout.trim()).href,
    { 'content-type': 'application/octet-stream' },
    { method: 'POST', body: Buffer.alloc(16 * 1024 * 1024, 1) }
  );
  assert.equal(answer.status, 413);
  assert.equal(answer.body.toString(), 'too big\n');
  assert.ok(Date.now() - started < 4_000, String(Date.now() - started));
});

test('--local goes to a running relay of another serve hub', async () => {
  const box = sandbox(hub);
  const first = await box.run([box.site({ 'index.html': 'a' }, 'a')]);
  assert.equal(first.code, 0, first.stderr);
  const other = await fakeHub();
  after(() => other.close());
  const local = await box.run(['--local', box.site({ 'index.html': 'b' }, 'b')], {
    SLICC_SERVE_HUB: other.origin,
  });
  assert.equal(local.code, 0, local.stderr);
  assert.match(local.stdout, /^http:\/\/127\.0\.0\.1:\d+\/\n$/);
  const public_ = await box.run([box.site({ 'index.html': 'c' }, 'c')], {
    SLICC_SERVE_HUB: other.origin,
  });
  assert.equal(public_.code, 1);
  assert.match(public_.stderr, new RegExp(`a relay for ${hub.origin} \\(pid \\d+`));
});
