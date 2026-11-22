import assert from 'node:assert/strict';
import { mkdirSync, readdirSync, symlinkSync, truncateSync, writeFileSync } from 'node:fs';
import { request } from 'node:http';
import { join } from 'node:path';
import { test } from 'node:test';
import { alive, eventually, get, sandbox } from './helpers.mjs';

function raw(port, path, method = 'GET', headers = {}) {
  return new Promise((resolve, reject) => {
    const r = request({ host: '127.0.0.1', port, path, method, headers }, (res) => {
      const chunks = [];
      res.on('data', (c) => chunks.push(c));
      res.on('end', () =>
        resolve({ status: res.statusCode, headers: res.headers, body: Buffer.concat(chunks) })
      );
    });
    r.on('error', reject);
    r.end();
  });
}

async function foreground(box, site, extra = []) {
  const proc = box.start(['--local', '--foreground', ...extra, site]);
  const [url] = await proc.stdout(/^http:\/\/127\.0\.0\.1:\d+\/\S*$/m);
  return { proc, url, port: Number(new URL(url).port) };
}

const odd = process.platform === 'win32' ? 'files/b & c.txt' : 'files/b & <c>.txt';

test('--local --foreground serves the directory with the built-in server', async () => {
  const box = sandbox();
  const site = box.site({
    'index.html': '<h1>home</h1>\n',
    'style.css': 'body{}\n',
    'data.json': '{}\n',
    'pic.png': Buffer.from([0x89, 0x50, 0x4e, 0x47]),
    'docs/index.html': '<h1>docs</h1>\n',
    'files/a.txt': 'alpha\n',
    [odd]: 'beta\n',
    'files/.hidden': 'no\n',
    '.env': 'SECRET=1\n',
    '.git/config': '[core]\n',
    '.well-known/security.txt': 'Contact: x\n',
    noext: 'raw\n',
  });
  const { proc, url, port } = await foreground(box, site);
  assert.equal(url, `http://127.0.0.1:${port}/`);
  await proc.stderr(/serving .* Ctrl-C stops it/);

  const home = await get(url);
  assert.equal(home.status, 200);
  assert.equal(home.body.toString(), '<h1>home</h1>\n');
  assert.equal(home.headers.get('content-type'), 'text/html; charset=utf-8');
  assert.equal(home.headers.get('accept-ranges'), 'bytes');
  assert.equal(home.headers.get('x-content-type-options'), 'nosniff');
  assert.ok(home.headers.get('last-modified'));

  const types = {
    'style.css': 'text/css; charset=utf-8',
    'data.json': 'application/json',
    'pic.png': 'image/png',
    noext: 'application/octet-stream',
    '.well-known/security.txt': 'text/plain; charset=utf-8',
  };
  for (const [path, type] of Object.entries(types)) {
    const res = await get(`${url}${path}`);
    assert.equal(res.status, 200, path);
    assert.equal(res.headers.get('content-type'), type, path);
  }

  const docs = await raw(port, '/docs?x=1');
  assert.equal(docs.status, 301);
  assert.equal(docs.headers.location, '/docs/?x=1');
  assert.equal((await get(`${url}docs/`)).body.toString(), '<h1>docs</h1>\n');

  const listing = await get(`${url}files/`);
  assert.equal(listing.status, 200);
  const html = listing.body.toString();
  assert.match(html, /<a href="a\.txt">a\.txt<\/a>/);
  assert.doesNotMatch(html, /hidden/);
  if (process.platform === 'win32') {
    assert.match(html, /<a href="b%20&%20c\.txt">b &amp; c\.txt<\/a>/);
    assert.equal((await get(`${url}files/b%20%26%20c.txt`)).body.toString(), 'beta\n');
  } else {
    assert.match(html, /<a href="b%20&%20%3Cc%3E\.txt">b &amp; &lt;c&gt;\.txt<\/a>/);
    assert.equal((await get(`${url}files/b%20%26%20%3Cc%3E.txt`)).body.toString(), 'beta\n');
  }

  for (const path of ['/.env', '/.git/config', '/files/.hidden', '/missing.html']) {
    assert.equal((await raw(port, path)).status, 404, path);
  }
  for (const path of [
    '/../etc/passwd',
    '/%2e%2e/x',
    '/files/..%2f..%2fx',
    '/a%00b',
    '/a%5cb',
    '/%zz',
  ]) {
    assert.equal((await raw(port, path)).status, 400, path);
  }

  const head = await raw(port, '/style.css', 'HEAD');
  assert.equal(head.status, 200);
  assert.equal(head.headers['content-length'], '7');
  assert.equal(head.body.length, 0);
  const post = await raw(port, '/', 'POST');
  assert.equal(post.status, 405);
  assert.equal(post.headers.allow, 'GET, HEAD');

  await proc.stderr(/GET \/style\.css 200 7/);
  proc.child.kill();
});

test('the built-in server answers ranges', async () => {
  const box = sandbox();
  const site = box.site({ 'index.html': 'x', 'ten.txt': '0123456789' });
  const { proc, url } = await foreground(box, site);
  const cases = [
    ['bytes=2-4', 206, '234', 'bytes 2-4/10'],
    ['bytes=7-', 206, '789', 'bytes 7-9/10'],
    ['bytes=-3', 206, '789', 'bytes 7-9/10'],
    ['bytes=5-100', 206, '56789', 'bytes 5-9/10'],
    ['bytes=-100', 206, '0123456789', 'bytes 0-9/10'],
    ['bytes=10-', 416, '', 'bytes */10'],
    ['bytes=-0', 416, '', 'bytes */10'],
    ['bytes=5-2', 416, '', 'bytes */10'],
    ['bytes=0-1,4-5', 200, '0123456789', null],
    ['items=0-1', 200, '0123456789', null],
  ];
  for (const [range, status, body, contentRange] of cases) {
    const res = await get(`${url}ten.txt`, { range });
    assert.equal(res.status, status, range);
    assert.equal(res.body.toString(), body, range);
    assert.equal(res.headers.get('content-range'), contentRange, range);
  }
  const ignored = await get(`${url}ten.txt`, { range: 'bytes=2-4', 'if-range': '"x"' });
  assert.equal(ignored.status, 200);
  proc.child.kill();
});

test('the built-in server keeps a big file whole and a symlink out of the root', {
  skip: process.platform === 'win32' && 'symlinks need privileges on Windows',
}, async () => {
  const box = sandbox();
  const outside = box.site({ 'secret.txt': 'secret\n' }, 'outside');
  const site = box.site({ 'index.html': 'x', 'inside.txt': 'inside\n' });
  symlinkSync(join(outside, 'secret.txt'), join(site, 'leak.txt'));
  symlinkSync(outside, join(site, 'leakdir'));
  symlinkSync(join(site, 'inside.txt'), join(site, 'alias.txt'));
  writeFileSync(join(site, '.env'), 'SECRET=1\n');
  symlinkSync(join(site, '.env'), join(site, 'public.txt'));
  symlinkSync('.git', join(site, 'gitlink'));
  box.site({ '.git/config': '[core]\n' }, 'site');
  writeFileSync(join(site, 'big.bin'), '');
  truncateSync(join(site, 'big.bin'), 30 * 1024 * 1024);
  const { proc, url, port } = await foreground(box, site);
  assert.equal((await raw(port, '/leak.txt')).status, 404);
  assert.equal((await raw(port, '/leakdir/secret.txt')).status, 404);
  assert.equal((await get(`${url}alias.txt`)).body.toString(), 'inside\n');
  assert.equal((await raw(port, '/public.txt')).status, 404);
  assert.equal((await raw(port, '/gitlink/config')).status, 404);
  const big = await raw(port, '/big.bin', 'HEAD');
  assert.equal(big.headers['content-length'], String(30 * 1024 * 1024));
  proc.child.kill();
});

test('--local runs the server detached; --list shows it and --stop ends it', async () => {
  const box = sandbox();
  const site = box.site({ 'index.html': '<p>detached</p>' });
  const res = await box.run(['--local', site]);
  assert.equal(res.code, 0, res.stderr);
  const url = res.stdout.trim();
  assert.match(url, /^http:\/\/127\.0\.0\.1:\d+\/$/);
  assert.match(
    res.stderr,
    new RegExp(`serving .* at ${url.replaceAll('.', '\\.')}; stop it with: scserve --stop `)
  );
  assert.equal((await get(url)).body.toString(), '<p>detached</p>');

  const list = await box.run(['--list', '--json']);
  assert.equal(list.code, 0, list.stderr);
  const rows = JSON.parse(list.stdout);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].mode, 'local');
  assert.equal(rows[0].url, url);
  assert.match(rows[0].root, /site$/);
  const table = await box.run(['--list']);
  assert.match(table.stdout, /^TOKEN\s+MODE\s+EXPIRES\s+URL\s+ROOT\n/);
  assert.match(table.stdout, /local/);

  const relay = box.relay();
  const stop = await box.run(['--stop', url]);
  assert.equal(stop.code, 0, stop.stderr);
  assert.equal(stop.stdout, `stopped ${url}\n`);
  await eventually(async () => {
    try {
      await get(url);
      return false;
    } catch {
      return true;
    }
  });
  assert.equal((await box.run(['--list'])).stdout, 'no previews\n');
  await eventually(() => !alive(relay.pid), 15_000);
});

test('--local --port prints the URL of a server that already runs', async () => {
  const box = sandbox();
  const res = await box.run(['--local', '--port', '9', '--entry', 'app/index.html', '--json']);
  assert.equal(res.code, 0, res.stderr);
  const out = JSON.parse(res.stdout);
  assert.equal(out.localUrl, 'http://127.0.0.1:9/app/index.html');
  assert.equal(out.mode, 'local');
  assert.match(out.createdAt, /^\d{4}-\d\d-\d\dT/);
  assert.match(res.stderr, /nothing answers on 127\.0\.0\.1:9 yet/);
  assert.equal(box.relay(), null);
});

test('--stop of something unknown exits 1', async () => {
  const box = sandbox();
  let res = await box.run(['--stop', 'nothing']);
  assert.equal(res.code, 1);
  assert.equal(res.stderr, 'scserve: no preview matches nothing\n');
  res = await box.run(['--stop', `${'0'.repeat(20)}-${'a'.repeat(32)}`, '--json']);
  assert.equal(res.code, 1);
  assert.match(res.stderr, /is not a preview of this machine's host/);
  assert.equal(JSON.parse(res.stdout).exit, 1);
});

test('--stop takes the port of a local preview, or its URL on any loopback name', async () => {
  const box = sandbox();
  const urls = [];
  for (const name of ['one', 'two']) {
    const res = await box.run(['--local', box.site({ 'index.html': name }, name)]);
    assert.equal(res.code, 0, res.stderr);
    urls.push(res.stdout.trim());
  }
  const ports = urls.map((url) => new URL(url).port);
  const relay = box.relay();
  let stop = await box.run(['--stop', ports[0]]);
  assert.equal(stop.code, 0, stop.stderr);
  assert.equal(stop.stdout, `stopped ${urls[0]}\n`);
  stop = await box.run(['--stop', `http://localhost:${ports[1]}/index.html`, '--json']);
  assert.equal(stop.code, 0, stop.stderr);
  assert.deepEqual(JSON.parse(stop.stdout), { stopped: urls[1] });
  assert.equal((await box.run(['--list'])).stdout, 'no previews\n');
  await eventually(() => !alive(relay.pid), 15_000);
});

test('--stop refuses a port that matches several previews', async () => {
  const box = sandbox();
  const records = join(box.stateDir, 'previews');
  mkdirSync(records, { recursive: true });
  for (const id of ['a1', 'b2']) {
    writeFileSync(
      join(records, `${id}.json`),
      JSON.stringify({ id, mode: 'local', port: 5173, createdAt: '2026-01-01T00:00:00.000Z' })
    );
  }
  const res = await box.run(['--stop', '5173']);
  assert.equal(res.code, 1);
  assert.equal(res.stderr, 'scserve: 5173 matches 2 previews; stop one by its token\n');
  assert.deepEqual(readdirSync(records).sort(), ['a1.json', 'b2.json']);
});
