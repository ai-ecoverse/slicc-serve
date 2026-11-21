import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { after, before, beforeEach, describe, it } from 'node:test';
import { fakeHub } from './fake-hub.mjs';
import { eventually, root } from './helpers.mjs';
import { boot } from './kernel.mjs';

const skip = !existsSync(join(root, 'bin', 'serve.wasm')) && 'run npm run build:wasm first';
const STATE = '/root/.local/state/slicc/serve';
const hint =
  "serve: if seven asked, answer in seven's tab (a click allows it), then run serve again\n";
const PATH = /^(?!\/)(?:(?!\.\.?(?:\/|$))(?!\.(?!well-known(?:\/|$)))[^/\\:\0]+(?:\/|$))+$/;
const page = { calls: [], refuse: undefined, hold: undefined, snapshots: new Map(), seen: [] };
let hub;
let tray;

function refusal(name, message) {
  return Object.assign(new Error(message), { name });
}

function reader(stream) {
  const it = stream.getReader();
  let buffer = Buffer.alloc(0);
  let done = false;
  async function fill() {
    if (done) return false;
    const got = await it.read();
    if (got.done) {
      done = true;
      return false;
    }
    buffer = Buffer.concat([buffer, Buffer.from(got.value)]);
    return true;
  }
  return {
    async line() {
      for (;;) {
        const nl = buffer.indexOf(10);
        if (nl !== -1) {
          const text = buffer.subarray(0, nl).toString();
          buffer = buffer.subarray(nl + 1);
          return text;
        }
        if (buffer.length > 4096) throw new Error('a file line over 4 KiB');
        if (!(await fill())) {
          if (buffer.length) throw new Error('the body ends inside a file line');
          return null;
        }
      }
    },
    async exact(n) {
      while (buffer.length < n) {
        if (!(await fill())) throw new Error('the body ends inside a file');
      }
      const out = buffer.subarray(0, n);
      buffer = buffer.subarray(n);
      return out;
    },
  };
}

async function call(method, path, token, body, type = 'application/json') {
  const res = await fetch(`${hub.origin}${path}`, {
    method,
    headers: { authorization: `Bearer ${token}`, 'content-type': type },
    body:
      body === undefined
        ? undefined
        : typeof body === 'string' || Buffer.isBuffer(body)
          ? body
          : JSON.stringify(body),
  });
  const text = await res.text();
  let json = {};
  try {
    json = JSON.parse(text);
  } catch {}
  return { ok: res.ok, status: res.status, json };
}

function failed(answer, extra) {
  const { error = 'failed', code, active, limit } = answer.json;
  return `${JSON.stringify({ error, status: answer.status, code, active, limit, ...extra })}\n`;
}

async function snapshot(request) {
  const { options, signal } = request;
  if (page.refuse) throw page.refuse;
  const api = `/api/tray/${tray.trayId}`;
  const servedRoot = `/srv/serve/${randomBytes(16).toString('hex')}`;
  const minted = await call('POST', `${api}/preview`, tray.controllerToken, {
    servedRoot,
    entryPath: `${servedRoot}/${options.entry ?? 'index.html'}`,
    allowLive: false,
    ttlMs: options.ttlMs,
  });
  if (!minted.ok) return failed(minted, { during: 'mint' });
  const token = minted.json.previewToken;
  let finalized = false;
  const stop = () =>
    call('POST', `${api}/preview/stop`, tray.controllerToken, { previewToken: token });
  signal.addEventListener('abort', () => {
    if (!finalized) void stop();
  });
  const body = reader(request.body);
  const paths = new Set();
  let total = 0;
  try {
    for (;;) {
      const text = await body.line();
      if (text === null) break;
      const file = JSON.parse(text);
      if (typeof file.path !== 'string' || !PATH.test(file.path))
        throw new Error(`bad path ${file.path}`);
      if (paths.has(file.path)) throw new Error(`${file.path} twice`);
      if (!Number.isInteger(file.size) || file.size < 0 || file.size > 25 << 20)
        throw new Error('bad size');
      paths.add(file.path);
      total += file.size;
      if (paths.size > options.files || total > options.bytes)
        throw new Error('more than the header said');
      const bytes = await body.exact(file.size);
      page.seen.push(file);
      const put = await call(
        'PUT',
        `${api}/preview/${token}/file?path=${encodeURIComponent(file.path)}`,
        minted.json.uploadToken,
        bytes,
        file.type
      );
      if (!put.ok) {
        await stop();
        return failed(put, { during: 'upload', path: file.path });
      }
    }
  } catch (e) {
    await stop();
    throw new TypeError(`bad snapshot body: ${e.message}`);
  }
  if (paths.size !== options.files || total !== options.bytes) {
    await stop();
    throw new TypeError('bad snapshot body: fewer files than the header said');
  }
  if (page.hold) await page.hold;
  if (signal.aborted) return '';
  const done = await call('POST', `${api}/preview/${token}/finalize`, minted.json.uploadToken);
  if (!done.ok) {
    await stop();
    return failed(done, { during: 'finalize' });
  }
  finalized = true;
  const expiresAt = Date.now() + options.ttlMs;
  page.snapshots.set(token, { url: done.json.url, root: options.root, expiresAt });
  return `${JSON.stringify({ url: done.json.url, previewToken: token, trayId: tray.trayId, expiresAt, files: paths.size, bytes: total })}\n`;
}

const requests = {
  kinds: { 'serve.snapshot': {}, 'serve.list': {}, 'serve.stop': {}, 'open.url': {} },
  async handle(request) {
    page.calls.push({ kind: request.kind, options: request.options });
    if (request.kind === 'serve.snapshot') return snapshot(request);
    if (request.kind === 'serve.list') {
      return [...page.snapshots]
        .map(
          ([previewToken, s]) =>
            `${JSON.stringify({ previewToken, mode: 'persistent', url: s.url, root: s.root, expiresAt: s.expiresAt, since: 1 })}\n`
        )
        .join('');
    }
    if (request.kind === 'serve.stop') {
      const { previewToken } = request.options;
      const res = await call(
        'POST',
        `/api/tray/${tray.trayId}/preview/stop`,
        tray.controllerToken,
        { previewToken }
      );
      page.snapshots.delete(previewToken);
      if (!res.json.revoked) throw refusal('NotFoundError', 'the tray hub has no such preview');
      return '';
    }
    return '';
  },
};

let os;

before(async () => {
  if (skip) return;
  hub = await fakeHub();
  const created = await (await fetch(`${hub.origin}/tray`, { method: 'POST', body: '{}' })).json();
  tray = { trayId: created.trayId, controllerToken: created.capabilities.controller.token };
  os = await boot({ requests });
  await os.sh(
    [
      'mkdir -p /w/site/v2 /w/site/assets',
      "printf '<a href=v2/page.html>v2</a>' > /w/site/index.html",
      'printf page > /w/site/v2/page.html',
      'printf app > /w/site/assets/app.js',
      'printf secret > /w/site/.env',
      'ln -s v2 /w/site/latest',
      'ln -s .. /w/site/v2/up',
      'mkdir -p /w/noindex && printf x > /w/noindex/a.txt',
    ].join(' && ')
  );
});

after(async () => {
  os?.close();
  await hub?.close();
});

beforeEach(() => {
  page.calls = [];
  page.refuse = undefined;
  page.hold = undefined;
  page.seen = [];
});

const run = (argv, opts) => os.run(argv, opts);
const live = () => hub.previews().filter((p) => p.mode === 'persistent');

describe('serve --ttl in a kernel with a page', { skip }, () => {
  it('hands the files to the page, which uploads and finalizes them on the hub', async () => {
    const res = await run(['serve', '--ttl', '1h', 'site'], { cwd: '/w' });
    assert.equal(res.status, 0, res.stderr);
    const [preview] = live();
    assert.equal(preview.state, 'ready');
    assert.equal(res.stdout, `${preview.url.replace(/index\.html$/, '')}\n`);
    assert.match(
      res.stderr,
      new RegExp(
        `^serve: a snapshot of /w/site \\(4 files, 38 B\\) until \\d{4}-\\d\\d-\\d\\dT[^ ]+; stop it sooner with: serve --stop ${preview.previewToken.replace('.', '\\.')}\\n$`
      )
    );
    assert.deepEqual(page.calls, [
      {
        kind: 'serve.snapshot',
        options: { root: '/w/site', ttlMs: 3_600_000, files: 4, bytes: 38 },
      },
    ]);
    assert.deepEqual([...preview.files.keys()].sort(), [
      'assets/app.js',
      'index.html',
      'latest/page.html',
      'v2/page.html',
    ]);
    assert.equal(preview.files.get('assets/app.js').mime, 'text/javascript; charset=utf-8');
    assert.equal(
      (await hub.visit(new URL('v2/page.html', res.stdout.trim()).href)).body.toString(),
      'page'
    );

    const list = JSON.parse((await run(['serve', '--list', '--json'])).stdout);
    assert.deepEqual(
      list.map((r) => [r.previewToken, r.mode, r.root]),
      [[preview.previewToken, 'snapshot', '/w/site']]
    );
    assert.match(list[0].expiresAt, /^\d{4}-/);
    assert.match((await run(['serve', '--list'])).stdout, /snapshot/);
    const stop = await run(['serve', '--stop', res.stdout.trim()]);
    assert.equal(stop.status, 0, stop.stderr);
    assert.equal(stop.stdout, `stopped ${preview.previewToken}\n`);
    assert.deepEqual(live(), []);
    assert.equal((await run(['serve', '--list'])).stdout, 'no previews\n');
  });

  it('prints the record with --json, keeps --entry, and opens the URL with --open', async () => {
    const res = await run([
      'serve',
      '--ttl',
      '2d',
      '--entry',
      'v2/page.html',
      '--json',
      '--open',
      '/w/site',
    ]);
    assert.equal(res.status, 0, res.stderr);
    const out = JSON.parse(res.stdout);
    assert.equal(out.mode, 'persistent');
    assert.equal(out.files, 4);
    assert.equal(out.bytes, 38);
    assert.equal(out.entry, 'v2/page.html');
    assert.match(out.url, /\/v2\/page\.html$/);
    assert.ok(Date.parse(out.expiresAt) - Date.now() > 47 * 3_600_000);
    assert.equal(page.calls[0].options.entry, 'v2/page.html');
    assert.equal(page.calls[0].options.ttlMs, 2 * 86_400_000);
    assert.deepEqual(page.calls.at(-1), { kind: 'open.url', options: { url: out.url } });
    assert.equal((await run(['serve', '--stop', out.previewToken])).status, 0);
  });

  it('streams a file larger than the request window intact', async () => {
    const big = randomBytes(3 << 20);
    await os.kernel.writeFile('/w/big/index.html', 'x');
    await os.kernel.writeFile('/w/big/blob.bin', big);
    const res = await run(['serve', '--ttl', '1h', '--json', '/w/big']);
    assert.equal(res.status, 0, res.stderr);
    const token = JSON.parse(res.stdout).previewToken;
    const preview = live().find((p) => p.previewToken === token);
    assert.ok(preview.files.get('blob.bin').bytes.equals(big));
    assert.equal((await run(['serve', '--stop', token])).status, 0);
  });

  it('checks the entry and the limits before it asks the page', async () => {
    assert.deepEqual(await run(['serve', '--ttl', '1h', '/w/noindex']), {
      status: 1,
      stdout: '',
      stderr:
        'serve: /w/noindex has no index.html; a --ttl snapshot needs its entry file (give one with --entry)\n',
    });
    await os.sh(
      'mkdir -p /w/many && cd /w/many && touch index.html && for i in $(seq 1 1000); do : > f$i; done'
    );
    assert.deepEqual(await run(['serve', '--ttl', '1h', '/w/many']), {
      status: 1,
      stdout: '',
      stderr: 'serve: the snapshot exceeds 1000 files (hidden files excluded)\n',
    });
    assert.deepEqual(page.calls, []);
  });

  it("maps the hub's failures, stopping a snapshot it minted", async () => {
    hub.inject({
      method: 'POST',
      path: `/api/tray/${tray.trayId}/preview`,
      status: 429,
      body: { error: 'Snapshot limit reached', code: 'PREVIEW_LIMIT', active: 10, limit: 10 },
    });
    assert.deepEqual(await run(['serve', '--ttl', '1h', '/w/site']), {
      status: 4,
      stdout: '',
      stderr:
        "serve: publishing the snapshot: the tray hub's limit is reached (429: Snapshot limit reached (10 of 10 --ttl snapshots in use); list them with `serve --list` and free one with `serve --stop <token>`)\n",
    });
    await os.kernel.writeFile('/w/large/index.html', 'x');
    await os.kernel.writeFile('/w/large/blob.bin', randomBytes(3 << 20));
    hub.inject({
      method: 'POST',
      path: `/api/tray/${tray.trayId}/preview`,
      status: 503,
      body: { error: 'down' },
    });
    const early = await run(['serve', '--ttl', '1h', '/w/large']);
    assert.equal(early.status, 1);
    assert.equal(early.stderr, 'serve: publishing the snapshot: the tray hub answered 503: down\n');
    hub.inject({
      method: 'PUT',
      path: /\/file$/,
      status: 400,
      body: { error: 'invalid preview file path' },
    });
    const upload = await run(['serve', '--ttl', '1h', '--json', '/w/site']);
    assert.equal(upload.status, 1);
    assert.deepEqual(JSON.parse(upload.stdout).lines, [
      'serve: uploading the snapshot: the tray hub answered 400: invalid preview file path (assets/app.js)',
    ]);
    hub.inject({
      method: 'POST',
      path: /\/finalize$/,
      status: 403,
      body: { error: 'invalid upload capability' },
    });
    const final = await run(['serve', '--ttl', '1h', '/w/site']);
    assert.equal(final.status, 3);
    assert.equal(
      final.stderr,
      'serve: uploading the snapshot: the tray hub refused it (403: invalid upload capability)\n'
    );
    assert.deepEqual(live(), []);
    assert.equal((await os.sh(`ls ${STATE}/previews | wc -l`)).stdout.trim(), '0');
  });

  it("prints the page's refusals, with the hint for EACCES", async () => {
    page.refuse = refusal('NotAllowedError', 'the request to publish was dismissed');
    assert.deepEqual(await run(['serve', '--ttl', '1h', '/w/site']), {
      status: 1,
      stdout: '',
      stderr: `serve: the request to publish was dismissed\n${hint}`,
    });
    page.refuse = refusal('NotSupportedError', "seven isn't connected to the tray hub");
    assert.equal(
      (await run(['serve', '--ttl', '1h', '/w/site'])).stderr,
      "serve: seven isn't connected to the tray hub\n"
    );
    assert.deepEqual(live(), []);
  });

  it('stops a snapshot that is not finalized when serve goes away', async () => {
    let release;
    page.hold = new Promise((r) => {
      release = r;
    });
    const running = run(['serve', '--ttl', '1h', '/w/site']);
    await eventually(() => page.seen.length === 4 && live().length === 1, 20_000);
    const pid = (
      await os.sh(
        `t=-ttl; for p in /proc/[0-9]*; do c=$(tr '\\0' ' ' < $p/cmdline); case $c in *"serve -$t"*) echo \${p#/proc/};; esac; done`
      )
    ).stdout.trim();
    await os.sh(`kill ${pid}`);
    const res = await running;
    assert.notEqual(res.status, 0);
    await eventually(() => live().length === 0, 20_000);
    release();
  });
});

describe('serve --ttl without a page', { skip }, () => {
  it('gets ENOTSUP from a session no page answers', async () => {
    const bare = await boot();
    try {
      await bare.sh('mkdir -p /w/s && printf x > /w/s/index.html');
      assert.deepEqual(await bare.run(['serve', '--ttl', '1h', '/w/s']), {
        status: 1,
        stdout: '',
        stderr: 'serve: no page answers requests for this session\n',
      });
    } finally {
      bare.close();
    }
  });
});
