import assert from 'node:assert/strict';
import { chmodSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { after, test } from 'node:test';
import { fakeHub } from './fake-hub.mjs';
import { sandbox } from './helpers.mjs';

const hub = await fakeHub();
after(() => hub.close());

const preview = /^\/api\/tray\/[^/]+\/preview$/;
const controller = /^\/controller\/[^/]+$/;

async function publish(box, args = []) {
  return box.run([...args, box.site()]);
}

test('a redirect of POST /tray to another origin asks for a sign-in and exits 3', async () => {
  const box = sandbox(hub);
  hub.inject({
    method: 'POST',
    path: '/tray',
    status: 302,
    headers: { location: 'https://auth.example.test/login?next=%2Ftray' },
  });
  const res = await publish(box);
  assert.equal(res.code, 3, res.stderr);
  assert.equal(res.stdout, '');
  assert.equal(
    res.stderr,
    'scserve: the tray hub wants you to sign in: https://auth.example.test/login?next=%2Ftray\n'
  );
});

test('a same-origin redirect to a page that is not the API asks for a sign-in too', async () => {
  const box = sandbox(hub);
  hub.inject({ method: 'POST', path: '/tray', status: 303, headers: { location: '/login' } });
  const res = await publish(box, ['--json']);
  assert.equal(res.code, 3);
  assert.equal(res.stderr, `scserve: the tray hub wants you to sign in: ${hub.origin}/login\n`);
  const out = JSON.parse(res.stdout);
  assert.equal(out.exit, 3);
  assert.equal(out.signIn, `${hub.origin}/login`);
});

test('--open opens the sign-in page with the platform opener', {
  skip: process.platform === 'win32' && 'the opener is explorer.exe',
}, async () => {
  const box = sandbox(hub);
  const bin = join(box.dir, 'bin');
  box.site({ x: '' }, 'bin');
  const log = join(box.dir, 'opened.txt');
  const opener = process.platform === 'darwin' ? 'open' : 'xdg-open';
  writeFileSync(join(bin, opener), `#!/bin/sh\nprintf '%s\\n' "$@" >> '${log}'\n`);
  chmodSync(join(bin, opener), 0o755);
  hub.inject({
    method: 'POST',
    path: '/tray',
    status: 302,
    headers: { location: 'https://auth.example.test/login' },
  });
  const res = await box.run(['--open', box.site()], { PATH: `${bin}:${process.env.PATH}` });
  assert.equal(res.code, 3);
  const { readFileSync } = await import('node:fs');
  assert.equal(readFileSync(log, 'utf8'), 'https://auth.example.test/login\n');
});

test('same-origin API redirects are followed up to 5 times, keeping the method', async () => {
  const box = sandbox(hub);
  const before = hub.hits('POST', '/tray');
  for (let i = 0; i < 5; i++)
    hub.inject({
      method: 'POST',
      path: '/tray',
      status: 307,
      headers: { location: `/tray?hop=${i}` },
    });
  const res = await publish(box);
  assert.equal(res.code, 0, res.stderr);
  assert.equal(hub.hits('POST', '/tray') - before, 6);
});

test('a sixth redirect exits 1', async () => {
  const box = sandbox(hub);
  hub.inject({
    method: 'POST',
    path: '/tray',
    status: 308,
    headers: { location: '/tray' },
    times: 6,
  });
  const res = await publish(box);
  assert.equal(res.code, 1);
  assert.equal(res.stderr, 'scserve: publishing: the tray hub redirected more than 5 times\n');
});

test('the bearer goes only to the hub and its API redirects', async () => {
  const box = sandbox(hub);
  await box.saveTray(hub);
  hub.inject({
    method: 'POST',
    path: preview,
    status: 307,
    headers: { location: '/api/tray/elsewhere/preview' },
  });
  await publish(box);
  const hop = hub.requests.findLast((r) => r.path === '/api/tray/elsewhere/preview');
  assert.match(hop.headers.authorization, /^Bearer /);
});

test('401 with WWW-Authenticate exits 3 and names the sign-in page', async () => {
  const box = sandbox(hub);
  hub.inject({
    method: 'POST',
    path: '/tray',
    status: 401,
    headers: {
      'www-authenticate':
        'Bearer realm="sliccy", authorization_uri="https://login.example.test/authorize"',
    },
    body: { error: 'sign in to create trays', code: 'SIGN_IN_REQUIRED' },
  });
  const res = await publish(box);
  assert.equal(res.code, 3);
  assert.equal(
    res.stderr,
    'scserve: publishing: the tray hub refused it (401: sign in to create trays (SIGN_IN_REQUIRED))\nscserve: sign in at https://login.example.test/authorize\n'
  );
});

test('403 with a Link rel="login" exits 3 and names it', async () => {
  const box = sandbox(hub);
  hub.inject({
    method: 'POST',
    path: '/tray',
    status: 403,
    headers: {
      link: '<https://www.example.test/docs>; rel="help", <https://login.example.test/>; rel="login"',
    },
    body: { error: 'forbidden' },
  });
  const res = await publish(box);
  assert.equal(res.code, 3);
  assert.match(
    res.stderr,
    /refused it \(403: forbidden\)\nscserve: sign in at https:\/\/login\.example\.test\/\n$/
  );
});

test('a saved tray whose controller token is refused is recreated once', async () => {
  const box = sandbox(hub);
  const saved = await box.saveTray(hub);
  const before = hub.hits('POST', '/tray');
  hub.inject({
    method: 'POST',
    path: controller,
    status: 403,
    body: { error: 'Invalid controller capability', code: 'INVALID_CONTROLLER_CAPABILITY' },
  });
  const res = await publish(box);
  assert.equal(res.code, 0, res.stderr);
  assert.equal(hub.hits('POST', '/tray') - before, 1);
  const tray = box.tray();
  assert.notEqual(tray.current.trayId, saved.trayId);
  assert.deepEqual(tray.retired, []);
  assert.match(res.stdout, new RegExp(tray.current.trayId.replaceAll('-', '')));
});

test('a recreated tray that is refused again exits 3', async () => {
  const box = sandbox(hub);
  await box.saveTray(hub);
  hub.inject({ method: 'POST', path: controller, status: 401, body: { error: 'nope' }, times: 2 });
  const res = await publish(box);
  assert.equal(res.code, 3);
  assert.match(res.stderr, /^scserve: publishing: the tray hub refused it \(401: nope\)\n$/);
});

test('a refused mint on a saved tray recreates the tray and mints again', async () => {
  const box = sandbox(hub);
  const saved = await box.saveTray(hub);
  hub.inject({
    method: 'POST',
    path: preview,
    status: 403,
    body: { error: 'Invalid controller capability' },
  });
  const res = await publish(box);
  assert.equal(res.code, 0, res.stderr);
  assert.notEqual(box.tray().current.trayId, saved.trayId);
  const page = await hub.visit(res.stdout.trim());
  assert.equal(page.status, 200);
});

test('402 exits 4 and shows Retry-After', async () => {
  const box = sandbox(hub);
  hub.inject({
    method: 'POST',
    path: '/tray',
    status: 402,
    headers: { 'retry-after': '30' },
    body: { error: 'Payment required', code: 'PLAN_REQUIRED' },
  });
  const res = await publish(box);
  assert.equal(res.code, 4);
  assert.equal(
    res.stderr,
    "scserve: publishing: the tray hub's limit is reached (402: Payment required (PLAN_REQUIRED))\nscserve: retry after 30 s\n"
  );
});

test('429 LIVE_PREVIEW_LIMIT exits 4 with the remedy', async () => {
  const box = sandbox(hub);
  hub.inject({
    method: 'POST',
    path: preview,
    status: 429,
    headers: { 'retry-after': 'Wed, 21 Oct 2026 07:28:00 GMT' },
    body: { error: 'Too many live previews', code: 'LIVE_PREVIEW_LIMIT', active: 200, limit: 200 },
  });
  const res = await publish(box);
  assert.equal(res.code, 4);
  assert.match(
    res.stderr,
    /Too many live previews \(200 of 200 live previews in use\); list them with `scserve --list`/
  );
  assert.match(res.stderr, /retry after Wed, 21 Oct 2026 07:28:00 GMT\n$/);
});

test('an expired saved tray (410) is recreated once', async () => {
  const box = sandbox(hub);
  const saved = await box.saveTray(hub);
  hub.trays.get(saved.trayId).expired = true;
  const res = await publish(box);
  assert.equal(res.code, 0, res.stderr);
  assert.notEqual(box.tray().current.trayId, saved.trayId);
});

test('a saved tray the hub does not know (404) is recreated once', async () => {
  const box = sandbox(hub);
  const saved = await box.saveTray(hub);
  hub.inject({
    method: 'POST',
    path: controller,
    status: 404,
    body: { error: 'Not found', code: 'TRAY_NOT_INITIALIZED' },
  });
  const res = await publish(box);
  assert.equal(res.code, 0, res.stderr);
  assert.notEqual(box.tray().current.trayId, saved.trayId);
});

test('404 on a fresh tray is not retried', async () => {
  const box = sandbox(hub);
  hub.inject({
    method: 'POST',
    path: controller,
    status: 404,
    body: { error: 'Not found', code: 'TRAY_NOT_INITIALIZED' },
  });
  const before = hub.hits('POST', '/tray');
  const res = await publish(box);
  assert.equal(res.code, 1);
  assert.equal(
    res.stderr,
    'scserve: publishing: the tray hub answered 404: Not found (TRAY_NOT_INITIALIZED)\n'
  );
  assert.equal(hub.hits('POST', '/tray') - before, 1);
});

test('other 4xx exit 1 with the hub JSON error', async () => {
  const box = sandbox(hub);
  hub.inject({
    method: 'POST',
    path: preview,
    status: 400,
    body: { error: 'invalid preview entry path' },
  });
  const res = await publish(box);
  assert.equal(res.code, 1);
  assert.equal(
    res.stderr,
    'scserve: publishing: the tray hub answered 400: invalid preview entry path\n'
  );
});

test('5xx is retried after 1, 2 and 5 s, then exits 1', async () => {
  const box = sandbox(hub);
  const before = hub.hits('POST', '/tray');
  hub.inject({ method: 'POST', path: '/tray', status: 503, body: 'Service Unavailable', times: 4 });
  const started = Date.now();
  const res = await publish(box);
  assert.equal(res.code, 1);
  assert.equal(
    res.stderr,
    'scserve: publishing: the tray hub answered 503 4 times: Service Unavailable\n'
  );
  assert.equal(hub.hits('POST', '/tray') - before, 4);
  assert.ok(Date.now() - started >= 7_500);
});

test('5xx that clears up within the retries succeeds', async () => {
  const box = sandbox(hub);
  hub.inject({ method: 'POST', path: preview, status: 500, body: { error: 'boom' }, times: 2 });
  const res = await publish(box);
  assert.equal(res.code, 0, res.stderr);
});

test('the WebSocket upgrade: a redirect to another origin exits 3', async () => {
  const box = sandbox(hub);
  hub.inject({
    upgrade: true,
    path: controller,
    status: 302,
    headers: { location: 'https://auth.example.test/ws-login' },
  });
  const res = await publish(box);
  assert.equal(res.code, 3);
  assert.equal(
    res.stderr,
    'scserve: the tray hub wants you to sign in: https://auth.example.test/ws-login\n'
  );
});

test('the WebSocket upgrade: a refused saved tray is recreated once', async () => {
  const box = sandbox(hub);
  const saved = await box.saveTray(hub);
  hub.inject({
    upgrade: true,
    path: controller,
    status: 403,
    body: { error: 'Only the elected leader may open the tray WebSocket', code: 'LEADER_ONLY' },
  });
  const res = await publish(box);
  assert.equal(res.code, 0, res.stderr);
  assert.notEqual(box.tray().current.trayId, saved.trayId);
});

test('the WebSocket upgrade: 429 exits 4', async () => {
  const box = sandbox(hub);
  hub.inject({
    upgrade: true,
    path: controller,
    status: 429,
    headers: { 'retry-after': '5' },
    body: { error: 'slow down' },
  });
  const res = await publish(box);
  assert.equal(res.code, 4);
  assert.match(res.stderr, /limit is reached \(429: slow down\)\nscserve: retry after 5 s\n$/);
});

test('the WebSocket upgrade: a same-origin API redirect is followed', async () => {
  const box = sandbox(hub);
  hub.inject({
    upgrade: true,
    path: controller,
    status: 307,
    headers: { location: '/controller/moved' },
  });
  const res = await publish(box);
  assert.equal(res.code, 3);
  assert.match(res.stderr, /refused it \(403: Invalid controller capability/);
  assert.ok(hub.requests.some((r) => r.upgrade && r.path === '/controller/moved'));
});

test('the WebSocket upgrade: 5xx is retried', async () => {
  const box = sandbox(hub);
  hub.inject({ upgrade: true, path: controller, status: 502, body: 'bad gateway', times: 1 });
  const res = await publish(box);
  assert.equal(res.code, 0, res.stderr);
});

test('an unreachable hub exits 1 after the retries', async () => {
  const box = sandbox();
  const res = await box.run([box.site()], { SLICC_TRAY_HUB: 'http://127.0.0.1:9' });
  assert.equal(res.code, 1);
  assert.match(res.stderr, /^scserve: publishing: the tray hub is unreachable: /);
});
