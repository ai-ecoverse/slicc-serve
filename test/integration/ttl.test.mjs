import assert from 'node:assert/strict';
import { mkdirSync, symlinkSync, truncateSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { after, test } from 'node:test';
import { fakeHub } from './fake-hub.mjs';
import { sandbox } from './helpers.mjs';

const hub = await fakeHub();
after(() => hub.close());

const files = /^\/api\/tray\/[^/]+\/preview\/[^/]+\/file$/;
const finalize = /^\/api\/tray\/[^/]+\/preview\/[^/]+\/finalize$/;
const stop = /^\/api\/tray\/[^/]+\/preview\/stop$/;

function find(token) {
  return hub.previews().find((p) => p.previewToken === token);
}

test('--ttl uploads a snapshot, finalizes it and leaves nothing running', async () => {
  const box = sandbox(hub);
  const site = box.site({
    'index.html': '<h1>snap</h1>',
    'style.css': 'body{}',
    'sub/page.html': '<p>sub</p>',
    'sub/deep/data.json': '{"a":1}',
    'img/pic.webp': Buffer.from([1, 2, 3]),
    '.env': 'SECRET=1',
    'sub/.cache/x': 'no',
  });
  const res = await box.run(['--ttl', '2h', '--json', site]);
  assert.equal(res.code, 0, res.stderr);
  const out = JSON.parse(res.stdout);
  assert.equal(out.mode, 'persistent');
  assert.equal(out.files, 5);
  assert.match(out.url, /\.localhost:\d+\/$/);
  const expires = Date.parse(out.expiresAt) - Date.now();
  assert.ok(expires > 7_100_000 && expires <= 7_200_000, String(expires));
  const p = find(out.previewToken);
  assert.equal(p.state, 'ready');
  assert.equal(p.allowLive, false);
  assert.deepEqual([...p.files.keys()].sort(), [
    'img/pic.webp',
    'index.html',
    'style.css',
    'sub/deep/data.json',
    'sub/page.html',
  ]);
  assert.equal(p.files.get('style.css').mime, 'text/css; charset=utf-8');
  assert.equal(p.files.get('img/pic.webp').mime, 'image/webp');
  assert.deepEqual([...p.files.get('img/pic.webp').bytes], [1, 2, 3]);
  assert.equal(box.relay(), null);
  const page = await hub.visit(out.url);
  assert.equal(page.body.toString(), '<h1>snap</h1>');

  const list = JSON.parse((await box.run(['--list', '--json'])).stdout);
  assert.equal(list.length, 1);
  assert.equal(list[0].mode, 'snapshot');
  assert.equal(list[0].previewToken, out.previewToken);
  assert.match(list[0].root, /site$/);
  assert.ok(list[0].expiresAt);

  const stopped = await box.run(['--stop', out.previewToken]);
  assert.equal(stopped.code, 0, stopped.stderr);
  assert.equal(stopped.stdout, `stopped ${out.previewToken}\n`);
  assert.equal(find(out.previewToken), undefined);
  assert.equal((await box.run(['--list'])).stdout, 'no previews\n');
});

test('--ttl with --entry keeps the entry in the URL', async () => {
  const box = sandbox(hub);
  const site = box.site({ 'docs/start.html': 'start' });
  const res = await box.run(['--ttl', '10m', '--entry', 'docs/start.html', site]);
  assert.equal(res.code, 0, res.stderr);
  assert.match(res.stdout, /\/docs\/start\.html\n$/);
  assert.match(
    res.stderr,
    /a snapshot of .*site \(1 files, 5 B\) until .*; stop it sooner with: scserve --stop /
  );
});

test('--ttl refuses a directory without its entry file', async () => {
  const box = sandbox(hub);
  const site = box.site({ 'other.html': 'x' });
  const before = hub.hits('POST', /\/preview$/);
  const res = await box.run(['--ttl', '1h', site]);
  assert.equal(res.code, 1);
  assert.match(
    res.stderr,
    /has no index\.html; a --ttl snapshot needs its entry file \(give one with --entry\)\n$/
  );
  assert.equal(hub.hits('POST', /\/preview$/), before);
});

test('--ttl refuses more than 1000 files before uploading anything', async () => {
  const box = sandbox(hub);
  const entries = { 'index.html': 'x' };
  for (let i = 0; i < 1000; i++) entries[`f/${i}.txt`] = '';
  const site = box.site(entries);
  const before = hub.hits('POST', /\/preview$/);
  const res = await box.run(['--ttl', '1h', site]);
  assert.equal(res.code, 1);
  assert.equal(res.stderr, 'scserve: the snapshot exceeds 1000 files (hidden files excluded)\n');
  assert.equal(hub.hits('POST', /\/preview$/), before);
});

test('--ttl refuses a file over 25 MiB and a snapshot over 50 MiB', async () => {
  const box = sandbox(hub);
  const big = box.site({ 'index.html': 'x', 'big.bin': '' }, 'big');
  truncateSync(join(big, 'big.bin'), 25 * 1024 * 1024 + 1);
  let res = await box.run(['--ttl', '1h', big]);
  assert.equal(res.code, 1);
  assert.equal(res.stderr, 'scserve: big.bin exceeds the 25 MiB limit per file\n');
  const many = box.site({ 'index.html': 'x' }, 'many');
  for (const name of ['a.bin', 'b.bin', 'c.bin']) {
    writeFileSync(join(many, name), '');
    truncateSync(join(many, name), 20 * 1024 * 1024);
  }
  res = await box.run(['--ttl', '1h', many]);
  assert.equal(res.code, 1);
  assert.equal(res.stderr, 'scserve: the snapshot exceeds 50 MiB in total\n');
});

test('a failed upload revokes the snapshot', async () => {
  const box = sandbox(hub);
  const site = box.site({ 'index.html': 'x', 'a.txt': 'a' });
  const stops = hub.hits('POST', stop);
  hub.inject({
    method: 'PUT',
    path: files,
    status: 413,
    body: { error: 'preview file exceeds 25 MiB limit' },
  });
  const res = await box.run(['--ttl', '1h', site]);
  assert.equal(res.code, 1);
  assert.match(
    res.stderr,
    /^scserve: uploading the snapshot: the tray hub answered 413: preview file exceeds 25 MiB limit \((a\.txt|index\.html)\)\n$/
  );
  assert.equal(hub.hits('POST', stop) - stops, 1);
  assert.equal(hub.previews().filter((p) => p.state === 'pending').length, 0);
});

test('a finalize that keeps failing revokes the snapshot', async () => {
  const box = sandbox(hub);
  const site = box.site({ 'index.html': 'x' });
  const stops = hub.hits('POST', stop);
  hub.inject({ method: 'POST', path: finalize, status: 500, body: { error: 'boom' }, times: 4 });
  const res = await box.run(['--ttl', '1h', site]);
  assert.equal(res.code, 1);
  assert.match(res.stderr, /answered 500 4 times: boom\n$/);
  assert.equal(hub.hits('POST', stop) - stops, 1);
});

test('a refused upload capability exits 3 and revokes', async () => {
  const box = sandbox(hub);
  const site = box.site({ 'index.html': 'x' });
  hub.inject({
    method: 'PUT',
    path: files,
    status: 403,
    body: { error: 'invalid upload capability' },
  });
  const res = await box.run(['--ttl', '1h', site]);
  assert.equal(res.code, 3);
  assert.match(res.stderr, /refused it \(403: invalid upload capability\)/);
});

test('the snapshot quota exits 4 with the remedy', async () => {
  const box = sandbox(hub);
  const site = box.site({ 'index.html': 'x' });
  hub.inject({
    method: 'POST',
    path: /\/preview$/,
    status: 429,
    body: { error: 'Snapshot limit reached', code: 'PREVIEW_LIMIT', active: 10, limit: 10 },
  });
  const res = await box.run(['--ttl', '1h', site]);
  assert.equal(res.code, 4);
  assert.match(
    res.stderr,
    /Snapshot limit reached \(10 of 10 --ttl snapshots in use\); list them with `scserve --list` and free one with `scserve --stop <token>`/
  );
});

test('snapshots on a tray that was replaced stay listed and stoppable', async () => {
  const box = sandbox(hub);
  const site = box.site({ 'index.html': 'x' });
  const first = JSON.parse((await box.run(['--ttl', '1h', '--json', site])).stdout);
  const old = box.tray().current.trayId;
  hub.trays.get(old).expired = true;
  hub.inject({
    method: 'POST',
    path: new RegExp(`^/api/tray/${old}/preview$`),
    status: 410,
    body: { error: 'Tray expired', code: 'TRAY_EXPIRED' },
  });
  const second = JSON.parse((await box.run(['--ttl', '1h', '--json', site])).stdout);
  const tray = box.tray();
  assert.notEqual(tray.current.trayId, old);
  assert.deepEqual(
    tray.retired.map((t) => t.trayId),
    [old]
  );
  const list = JSON.parse((await box.run(['--list', '--json'])).stdout);
  assert.deepEqual(
    list.map((r) => r.previewToken).sort(),
    [first.previewToken, second.previewToken].sort()
  );
  assert.equal((await box.run(['--stop', first.url])).code, 0);
  await box.run(['--list']);
  assert.deepEqual(box.tray().retired, []);
});

test('--ttl leaves out symlinks to hidden files and survives directories that link to each other', {
  skip: process.platform === 'win32' && 'symlinks need privileges on Windows',
}, async () => {
  const box = sandbox(hub);
  const site = box.site({
    'index.html': 'x',
    '.env': 'SECRET=1',
    'a/one.txt': '1',
    'b/two.txt': '2',
  });
  symlinkSync(join(site, '.env'), join(site, 'public.txt'));
  symlinkSync('../b', join(site, 'a', 'tob'));
  symlinkSync('../a', join(site, 'b', 'toa'));
  mkdirSync(join(site, 'c'));
  const res = await box.run(['--ttl', '1h', '--json', site]);
  assert.equal(res.code, 0, res.stderr);
  const out = JSON.parse(res.stdout);
  const p = find(out.previewToken);
  const names = [...p.files.keys()].sort();
  assert.deepEqual(names, ['a/one.txt', 'a/tob/two.txt', 'index.html']);
  assert.ok(![...p.files.values()].some((f) => f.bytes.toString().includes('SECRET')));
});
