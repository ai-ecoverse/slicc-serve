import assert from 'node:assert/strict';
import { mkdirSync, rmSync, symlinkSync, truncateSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { after, test } from 'node:test';
import { fakeHub } from './fake-hub.mjs';
import { sandbox } from './helpers.mjs';

const hub = await fakeHub();
after(() => hub.close());

const files = /^\/api\/serve\/host\/[0-9a-f]{20}\/previews\/[^/]+\/files\/.+$/;
const finalize = /^\/api\/serve\/host\/[0-9a-f]{20}\/previews\/[^/]+\/finalize$/;
const stop = /^\/api\/serve\/host\/[0-9a-f]{20}\/previews\/[^/]+$/;
const mint = /^\/api\/serve\/host\/[0-9a-f]{20}\/previews$/;

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
    'a b/c%d.txt': 'odd',
    '.env': 'SECRET=1',
    'sub/.cache/x': 'no',
  });
  const res = await box.run(['--ttl', '2h', '--json', site]);
  assert.equal(res.code, 0, res.stderr);
  const out = JSON.parse(res.stdout);
  assert.equal(out.mode, 'persistent');
  assert.equal(out.files, 6);
  assert.match(out.url, /^http:\/\/[0-9a-f]{20}-[0-9a-f]{32}\.localhost:\d+\/$/);
  const expires = Date.parse(out.expiresAt) - Date.now();
  assert.ok(expires > 7_100_000 && expires <= 7_200_000, String(expires));
  const p = find(out.previewToken);
  assert.equal(p.state, 'ready');
  assert.equal(p.mode, 'snapshot');
  assert.equal(p.entry, '/');
  assert.equal(p.mint.ttlMs, 7_200_000);
  assert.match(p.tag, /^\/srv\/serve\/[0-9a-f]{16}$/);
  assert.ok(hub.requests.some((r) => r.path.endsWith('/files/a%20b/c%25d.txt')));
  assert.deepEqual([...p.files.keys()].sort(), [
    'a b/c%d.txt',
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
  assert.equal((await hub.visit(new URL('a%20b/c%25d.txt', out.url).href)).body.toString(), 'odd');

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
  const before = hub.hits('POST', mint);
  const res = await box.run(['--ttl', '1h', site]);
  assert.equal(res.code, 1);
  assert.match(
    res.stderr,
    /has no index\.html; a --ttl snapshot needs its entry file \(give one with --entry\)\n$/
  );
  assert.equal(hub.hits('POST', mint), before);
});

test('--ttl refuses more than 1000 files before uploading anything', async () => {
  const box = sandbox(hub);
  const entries = { 'index.html': 'x' };
  for (let i = 0; i < 1000; i++) entries[`f/${i}.txt`] = '';
  const site = box.site(entries);
  const before = hub.hits('POST', mint);
  const res = await box.run(['--ttl', '1h', site]);
  assert.equal(res.code, 1);
  assert.equal(res.stderr, 'scserve: the snapshot exceeds 1000 files (hidden files excluded)\n');
  assert.equal(hub.hits('POST', mint), before);
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
  const stops = hub.hits('DELETE', stop);
  hub.inject({
    method: 'PUT',
    path: files,
    status: 413,
    body: { error: 'a file is at most 25 MiB', code: 'FILE_TOO_LARGE' },
  });
  const res = await box.run(['--ttl', '1h', site]);
  assert.equal(res.code, 1);
  assert.match(
    res.stderr,
    /^scserve: uploading the snapshot: the serve hub answered 413: a file is at most 25 MiB \(FILE_TOO_LARGE\) \((a\.txt|index\.html)\)\n$/
  );
  assert.equal(hub.hits('DELETE', stop) - stops, 1);
  assert.equal(hub.previews().filter((p) => p.state === 'uploading').length, 0);
});

test('a finalize that keeps failing revokes the snapshot', async () => {
  const box = sandbox(hub);
  const site = box.site({ 'index.html': 'x' });
  const stops = hub.hits('DELETE', stop);
  hub.inject({ method: 'POST', path: finalize, status: 500, body: { error: 'boom' }, times: 4 });
  const res = await box.run(['--ttl', '1h', site]);
  assert.equal(res.code, 1);
  assert.match(res.stderr, /answered 500 4 times: boom\n$/);
  assert.equal(hub.hits('DELETE', stop) - stops, 1);
});

test('a refused upload exits 3 and stops the snapshot', async () => {
  const box = sandbox(hub);
  const site = box.site({ 'index.html': 'x' });
  const stops = hub.hits('DELETE', stop);
  hub.inject({
    method: 'PUT',
    path: files,
    status: 401,
    body: { error: 'wrong or missing secret', code: 'UNAUTHORIZED' },
  });
  const res = await box.run(['--ttl', '1h', site]);
  assert.equal(res.code, 3);
  assert.match(res.stderr, /refused it \(401: wrong or missing secret \(UNAUTHORIZED\)\)/);
  assert.equal(hub.hits('DELETE', stop) - stops, 1);
});

test('a finalize without the entry file (ENTRY_MISSING) exits 1 and stops the snapshot', async () => {
  const box = sandbox(hub);
  const site = box.site({ 'index.html': 'x' });
  hub.inject({
    method: 'POST',
    path: finalize,
    status: 400,
    body: { error: 'the entry file index.html was not uploaded', code: 'ENTRY_MISSING' },
  });
  const res = await box.run(['--ttl', '1h', site]);
  assert.equal(res.code, 1);
  assert.equal(
    res.stderr,
    'scserve: uploading the snapshot: the serve hub answered 400: the entry file index.html was not uploaded (ENTRY_MISSING)\n'
  );
});

test('a snapshot the hub finalizes without a URL is a failure, and is stopped', async () => {
  const box = sandbox(hub);
  const site = box.site({ 'index.html': 'x' });
  const saved = await box.saveHost(hub);
  const token = `${saved.hostId}-${'a'.repeat(32)}`;
  hub.inject({ method: 'POST', path: mint, status: 201, body: { previewToken: token } });
  hub.inject({ method: 'PUT', path: files, status: 200, body: { path: 'index.html', size: 1 } });
  hub.inject({ method: 'POST', path: finalize, status: 200, body: { previewToken: token } });
  const stops = hub.hits('DELETE', stop);
  const res = await box.run(['--ttl', '1h', site]);
  assert.equal(res.code, 1);
  assert.equal(res.stdout, '');
  assert.equal(
    res.stderr,
    "scserve: uploading the snapshot: the serve hub's finalize answer has no url\n"
  );
  assert.equal(hub.hits('DELETE', stop) - stops, 1);
  assert.equal((await box.run(['--list'])).stdout, 'no previews\n');
});

test('snapshots share the saved host, and --stop of one the hub lost exits 1', async () => {
  const box = sandbox(hub);
  const site = box.site({ 'index.html': 'x' });
  const first = JSON.parse((await box.run(['--ttl', '1h', '--json', site])).stdout);
  const second = JSON.parse((await box.run(['--ttl', '1h', '--json', site])).stdout);
  assert.equal(first.hostId, second.hostId);
  assert.equal(first.hostId, box.host().hostId);
  const list = JSON.parse((await box.run(['--list', '--json'])).stdout);
  assert.deepEqual(
    list.map((r) => r.previewToken).sort(),
    [first.previewToken, second.previewToken].sort()
  );
  hub.hosts.get(first.hostId).previews.delete(first.previewToken);
  const gone = await box.run(['--stop', first.previewToken]);
  assert.equal(gone.code, 1);
  assert.equal(
    gone.stderr,
    `scserve: the serve hub has no preview ${first.previewToken} (already stopped or expired)\n`
  );
  assert.equal(box.host().hostId, first.hostId);
  const other = `${'e'.repeat(20)}-${'f'.repeat(32)}`;
  const foreign = await box.run(['--stop', other]);
  assert.equal(foreign.code, 1);
  assert.equal(foreign.stderr, `scserve: ${other} is not a preview of this machine's host\n`);
  assert.equal((await box.run(['--stop', second.url])).code, 0);
  assert.equal((await box.run(['--list'])).stdout, 'no previews\n');
});

test('--ttl keeps a directory under its real path and under each alias that links to it (#10, #11)', {
  skip: process.platform === 'win32' && 'symlinks need privileges on Windows',
}, async () => {
  const box = sandbox(hub);
  const site = box.site({
    'index.html': '<a href="v2/page.html">v2</a><script src="assets/app.js"></script>',
    'v2/page.html': 'page',
    'assets/app.js': 'app',
  });
  symlinkSync('v2', join(site, 'latest'));
  symlinkSync('assets', join(site, 'a-alias'));
  symlinkSync('..', join(site, 'v2', 'up'));
  const res = await box.run(['--ttl', '1h', '--json', site]);
  assert.equal(res.code, 0, res.stderr);
  const out = JSON.parse(res.stdout);
  assert.equal(out.files, 5);
  const p = find(out.previewToken);
  assert.deepEqual([...p.files.keys()].sort(), [
    'a-alias/app.js',
    'assets/app.js',
    'index.html',
    'latest/page.html',
    'v2/page.html',
  ]);
  for (const path of ['v2/page.html', 'assets/app.js', 'latest/page.html']) {
    const got = await hub.visit(new URL(path, out.url).href);
    assert.equal(got.status, 200, path);
  }
});

test('--ttl counts an aliased subtree once per path, so the file limit bounds it', {
  skip: process.platform === 'win32' && 'symlinks need privileges on Windows',
}, async () => {
  const box = sandbox(hub);
  const files = { 'index.html': 'x' };
  for (let i = 0; i < 600; i++) files[`data/${i}.txt`] = `${i}`;
  const site = box.site(files);
  symlinkSync('data', join(site, 'mirror'));
  const res = await box.run(['--ttl', '1h', site]);
  assert.equal(res.code, 1);
  assert.match(res.stderr, /the snapshot exceeds 1000 files/);
});

test('--ttl walks at most 10,000 directories, aliases counted per path, before it mints', async () => {
  const box = sandbox(hub);
  const site = box.site({ 'index.html': 'x' });
  for (let i = 0; i < 10_000; i++) mkdirSync(join(site, 'd', String(i)), { recursive: true });
  rmSync(join(site, 'd', '0'), { recursive: true });
  const before = hub.hits('POST', mint);
  const fits = await box.run(['--ttl', '1h', site]);
  assert.equal(fits.code, 0, fits.stderr);
  assert.equal(hub.hits('POST', mint), before + 1);
  mkdirSync(join(site, 'd', '0'));
  mkdirSync(join(site, 'd', 'extra'));
  const over = await box.run(['--ttl', '1h', site]);
  assert.equal(over.code, 1);
  assert.match(over.stderr, /^scserve: the snapshot walks more than 10000 directories/);
  assert.equal(hub.hits('POST', mint), before + 1);
});

test('--ttl refuses a web of directory symlinks without files instead of walking it forever', {
  skip: process.platform === 'win32' && 'symlinks need privileges on Windows',
}, async () => {
  const box = sandbox(hub);
  const site = box.site({ 'index.html': 'x' });
  for (let n = 0; n < 8; n++) mkdirSync(join(site, `d${n}`));
  for (let n = 0; n < 8; n++) {
    for (let m = 0; m < 8; m++) if (n !== m) symlinkSync(`../d${m}`, join(site, `d${n}`, `to${m}`));
  }
  const before = hub.hits('POST', mint);
  const res = await box.run(['--ttl', '1h', site]);
  assert.equal(res.code, 1);
  assert.match(res.stderr, /^scserve: the snapshot walks more than 10000 directories/);
  assert.equal(hub.hits('POST', mint), before);
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
  assert.deepEqual(names, [
    'a/one.txt',
    'a/tob/two.txt',
    'b/toa/one.txt',
    'b/two.txt',
    'index.html',
  ]);
  assert.ok(![...p.files.values()].some((f) => f.bytes.toString().includes('SECRET')));
});
