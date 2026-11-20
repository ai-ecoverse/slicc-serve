import assert from 'node:assert/strict';
import { statSync, truncateSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { join } from 'node:path';
import { after, test } from 'node:test';
import { fakeHub } from './fake-hub.mjs';
import { alive, eventually, sandbox } from './helpers.mjs';

const hub = await fakeHub();
after(() => hub.close());

function previewOf(url) {
  return hub.previews().find((p) => p.url.startsWith(new URL(url).origin));
}

test('scserve DIR publishes a live preview and returns', async () => {
  const box = sandbox(hub);
  const site = box.site({
    'index.html': '<h1>hello</h1>\n',
    'app.js': 'console.log(1)\n',
    'img.bin': Buffer.from([0, 1, 2, 255]),
    'bad.txt': Buffer.from([0xff, 0xfe, 0x41]),
    'sub/index.html': '<p>sub</p>',
    '.env': 'SECRET=1',
  });
  const res = await box.run([site]);
  assert.equal(res.code, 0, res.stderr);
  const url = res.stdout.trim();
  assert.match(url, /^http:\/\/[0-9a-f]{32}--[0-9a-f]+\.localhost:\d+\/$/);
  const token = previewOf(url).previewToken;
  assert.match(
    res.stderr,
    /site is public at .* \(here: http:\/\/127\.0\.0\.1:\d+\/\)\nscserve: stop it with: scserve --stop /
  );
  assert.ok(res.stderr.endsWith(`--stop ${token}\n`));

  const page = await hub.visit(url);
  assert.equal(page.status, 200);
  assert.equal(page.body.toString(), '<h1>hello</h1>\n');
  assert.equal(page.headers['content-type'], 'text/html; charset=utf-8');
  const js = await hub.visit(new URL('app.js', url).href);
  assert.equal(js.body.toString(), 'console.log(1)\n');
  assert.equal(js.headers['x-encoding'], 'utf-8');
  const bin = await hub.visit(new URL('img.bin', url).href);
  assert.deepEqual([...bin.body], [0, 1, 2, 255]);
  assert.equal(bin.headers['x-encoding'], 'base64');
  const bad = await hub.visit(new URL('bad.txt', url).href);
  assert.deepEqual([...bad.body], [0xff, 0xfe, 0x41]);
  assert.equal(bad.headers['x-encoding'], 'base64');
  assert.equal((await hub.visit(new URL('sub', url).href)).body.toString(), '<p>sub</p>');
  assert.equal((await hub.visit(new URL('nope.html', url).href)).status, 404);
  assert.equal((await hub.visit(new URL('.env', url).href)).status, 404);

  const p = previewOf(url);
  assert.match(p.servedRoot, /^\/srv\/serve\/[0-9a-f]{16}$/);
  assert.equal(p.entryPath, `${p.servedRoot}/index.html`);
  assert.equal(p.allowLive, false);

  const relay = box.relay();
  assert.ok(alive(relay.pid));
  const stop = await box.run(['--stop', url]);
  assert.equal(stop.code, 0, stop.stderr);
  assert.equal(stop.stdout, `stopped ${token}\n`);
  assert.equal(previewOf(url), undefined);
  await eventually(() => !alive(relay.pid), 15_000);
  assert.equal(box.relay(), null);
});

test('the relay passes ranges and refuses whole files over 25 MiB', async () => {
  const box = sandbox(hub);
  const site = box.site({ 'index.html': 'x', 'ten.txt': '0123456789', 'huge.bin': '' });
  truncateSync(join(site, 'huge.bin'), 26 * 1024 * 1024);
  writeFileSync(join(site, 'media.bin'), Buffer.alloc(9 * 1024 * 1024, 7));
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
  assert.equal(huge.status, 413);
  assert.equal(huge.body.toString(), 'preview file exceeds 25 MiB limit');
  const window = await hub.visit(new URL('huge.bin', url).href, { range: 'bytes=0-' });
  assert.equal(window.status, 206);
  assert.equal(
    window.headers['content-range'],
    `bytes 0-${8 * 1024 * 1024 - 1}/${26 * 1024 * 1024}`
  );
  const media = await hub.visit(new URL('media.bin', url).href);
  assert.equal(media.status, 200);
  assert.equal(media.body.length, 9 * 1024 * 1024);
  assert.ok(Number(media.headers['x-chunks']) > 100);
});

test('a second scserve reuses the saved tray and the running relay', async () => {
  const box = sandbox(hub);
  const first = await box.run([box.site({ 'index.html': 'one' }, 'one')]);
  const relay = box.relay();
  const second = await box.run([box.site({ 'index.html': 'two' }, 'two')]);
  assert.equal(first.code, 0);
  assert.equal(second.code, 0);
  assert.equal(box.relay().pid, relay.pid);
  const tray = box.tray();
  const label = tray.current.trayId.replaceAll('-', '');
  assert.ok(first.stdout.includes(label) && second.stdout.includes(label));
  assert.equal((await hub.visit(first.stdout.trim())).body.toString(), 'one');
  assert.equal((await hub.visit(second.stdout.trim())).body.toString(), 'two');
  if (process.platform !== 'win32') {
    assert.equal(statSync(join(box.configDir, 'tray.json')).mode & 0o777, 0o600);
    assert.equal(statSync(join(box.stateDir, 'relay.json')).mode & 0o777, 0o600);
  }
  const list = JSON.parse((await box.run(['--list', '--json'])).stdout);
  assert.equal(list.length, 2);
  assert.ok(list.every((r) => r.mode === 'live' && r.localUrl.startsWith('http://127.0.0.1:')));
  const table = (await box.run(['--list'])).stdout;
  assert.match(table, /^TOKEN\s+MODE\s+EXPIRES\s+URL\s+ROOT\n/);
  assert.equal(table.trim().split('\n').length, 3);
});

test('--port publishes a server that already runs', async () => {
  const box = sandbox(hub);
  const seen = [];
  const app = createServer((req, res) => {
    seen.push(req.url);
    if (req.url === '/old') {
      res.writeHead(302, { location: '/new' });
      return res.end();
    }
    res.writeHead(req.url === '/missing' ? 404 : 200, { 'content-type': 'text/plain' });
    res.end(`you asked for ${req.url}`);
  });
  await new Promise((r) => app.listen(0, '127.0.0.1', r));
  after(() => app.close());
  const port = app.address().port;
  const res = await box.run(['--port', String(port), '--json']);
  assert.equal(res.code, 0, res.stderr);
  const out = JSON.parse(res.stdout);
  assert.match(out.url, /\.localhost:\d+\/$/);
  assert.equal(out.port, port);
  assert.equal(out.localUrl, `http://127.0.0.1:${port}/`);
  assert.equal(previewOf(out.url).allowLive, true);
  assert.equal((await hub.visit(out.url)).body.toString(), 'you asked for /');
  assert.equal(
    (await hub.visit(new URL('api/items.json', out.url).href)).body.toString(),
    'you asked for /api/items.json'
  );
  assert.equal(
    (await hub.visit(new URL('old', out.url).href)).body.toString(),
    'you asked for /new'
  );
  const missing = await hub.visit(new URL('missing', out.url).href);
  assert.equal(missing.status, 404);
  assert.equal(missing.body.toString(), 'you asked for /missing');
  assert.equal(seen[0], '/');
  app.close();
  await new Promise((r) => setTimeout(r, 100));
  const down = await hub.visit(out.url);
  assert.equal(down.status, 502);
  assert.match(down.body.toString(), new RegExp(`nothing answers on 127\\.0\\.0\\.1:${port}`));
});

test('--port with --entry keeps the entry in the URL', async () => {
  const box = sandbox(hub);
  const app = createServer((req, res) => res.end(req.url));
  await new Promise((r) => app.listen(0, '127.0.0.1', r));
  after(() => app.close());
  const res = await box.run(['--port', String(app.address().port), '--entry', 'docs/start.html']);
  assert.equal(res.code, 0, res.stderr);
  const url = res.stdout.trim();
  assert.match(url, /\/docs\/start\.html$/);
  assert.equal((await hub.visit(url)).body.toString(), '/docs/start.html');
  assert.equal((await hub.visit(new URL('/', url).href)).body.toString(), '/docs/start.html');
});

test('--foreground stays attached, logs visits, and revokes when it ends', async () => {
  const box = sandbox(hub);
  const proc = box.start(['--foreground', box.site({ 'index.html': 'fg' })]);
  const [url] = await proc.stdout(/^http:\/\/\S+$/m);
  await proc.stderr(/is public at .*; Ctrl-C stops it/);
  assert.equal((await hub.visit(url)).body.toString(), 'fg');
  await proc.stderr(/scserve: GET \/ 200 2/);
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
  assert.match(proc.err, /scserve: stopped: revoked on the tray hub\n$/);
});

test('the relay reconnects when the hub drops its socket', async () => {
  const box = sandbox(hub);
  const res = await box.run([box.site({ 'index.html': 'again' })]);
  const url = res.stdout.trim();
  const tray = hub.trays.get(box.tray().current.trayId);
  const before = hub.hits('POST', /^\/controller\//);
  tray.socket.close();
  await eventually(() => hub.hits('POST', /^\/controller\//) > before && tray.socket, 15_000);
  assert.equal((await hub.visit(url)).body.toString(), 'again');
});

test('the relay stops a preview the hub revoked', async () => {
  const box = sandbox(hub);
  const res = await box.run([box.site()]);
  const url = res.stdout.trim();
  const tray = hub.trays.get(box.tray().current.trayId);
  const p = previewOf(url);
  const relay = box.relay();
  await fetch(`${hub.origin}/api/tray/${tray.trayId}/preview/stop`, {
    method: 'POST',
    headers: { authorization: `Bearer ${tray.controllerToken}` },
    body: JSON.stringify({ previewToken: p.previewToken }),
  });
  await eventually(() => !alive(relay.pid), 15_000);
  assert.equal((await box.run(['--list'])).stdout, 'no previews\n');
});

test('a starting relay revokes live previews nothing serves any more', async () => {
  const box = sandbox(hub);
  const saved = await box.saveTray(hub);
  const stale = await fetch(`${hub.origin}/api/tray/${saved.trayId}/preview`, {
    method: 'POST',
    headers: {
      authorization: `Bearer ${saved.controllerToken}`,
      'content-type': 'application/json',
    },
    body: JSON.stringify({
      servedRoot: '/srv/serve/dead',
      entryPath: '/srv/serve/dead/index.html',
      allowLive: false,
    }),
  }).then((r) => r.json());
  const res = await box.run([box.site()]);
  assert.equal(res.code, 0, res.stderr);
  await eventually(() => !hub.previews().some((p) => p.previewToken === stale.previewToken));
  assert.ok(previewOf(res.stdout.trim()));
});

test('--list drops a tray whose token the hub refuses and exits 3', async () => {
  const box = sandbox(hub);
  await box.saveTray(hub);
  hub.inject({ method: 'GET', path: /\/previews$/, status: 401, body: { error: 'sign in' } });
  const res = await box.run(['--list']);
  assert.equal(res.code, 3);
  assert.match(
    res.stderr,
    /^scserve: listing previews: the tray hub refused it \(401: sign in\)\n$/
  );
  assert.equal(box.tray().current, null);
});
