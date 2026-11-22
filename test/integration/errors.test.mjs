import assert from 'node:assert/strict';
import { chmodSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { after, test } from 'node:test';
import { fakeHub } from './fake-hub.mjs';
import { sandbox } from './helpers.mjs';

const hub = await fakeHub();
after(() => hub.close());

const CREATE = '/api/serve/host';
const preview = /^\/api\/serve\/host\/[0-9a-f]{20}\/previews$/;
const socket = /^\/api\/serve\/host\/[0-9a-f]{20}\/ws$/;

async function publish(box, args = []) {
  return box.run([...args, box.site()]);
}

test('a redirect of POST /api/serve/host to another origin asks for a sign-in and exits 3', async () => {
  const box = sandbox(hub);
  hub.inject({
    method: 'POST',
    path: CREATE,
    status: 302,
    headers: { location: 'https://auth.example.test/login?next=%2Fhost' },
  });
  const res = await publish(box);
  assert.equal(res.code, 3, res.stderr);
  assert.equal(res.stdout, '');
  assert.equal(
    res.stderr,
    'scserve: the serve hub wants you to sign in: https://auth.example.test/login?next=%2Fhost\n'
  );
});

test('a same-origin redirect to a page that is not the API asks for a sign-in too', async () => {
  const box = sandbox(hub);
  hub.inject({ method: 'POST', path: CREATE, status: 303, headers: { location: '/login' } });
  const res = await publish(box, ['--json']);
  assert.equal(res.code, 3);
  assert.equal(res.stderr, `scserve: the serve hub wants you to sign in: ${hub.origin}/login\n`);
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
    path: CREATE,
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
  const before = hub.hits('POST', CREATE);
  for (let i = 0; i < 5; i++)
    hub.inject({
      method: 'POST',
      path: CREATE,
      status: 307,
      headers: { location: `${CREATE}?hop=${i}` },
    });
  const res = await publish(box);
  assert.equal(res.code, 0, res.stderr);
  assert.equal(hub.hits('POST', CREATE) - before, 6);
});

test('a sixth redirect exits 1', async () => {
  const box = sandbox(hub);
  hub.inject({
    method: 'POST',
    path: CREATE,
    status: 308,
    headers: { location: CREATE },
    times: 6,
  });
  const res = await publish(box);
  assert.equal(res.code, 1);
  assert.equal(res.stderr, 'scserve: publishing: the serve hub redirected more than 5 times\n');
});

test('the bearer goes only to the hub and its API redirects', async () => {
  const box = sandbox(hub);
  const saved = await box.saveHost(hub);
  hub.inject({
    method: 'POST',
    path: preview,
    status: 307,
    headers: { location: '/api/elsewhere/previews' },
  });
  await publish(box);
  const hop = hub.requests.findLast((r) => r.path === '/api/elsewhere/previews');
  assert.equal(hop.headers.authorization, `Bearer ${saved.secret}`);
  assert.equal(hub.requests.findLast((r) => r.path === CREATE)?.headers.authorization, undefined);
});

test('401 with WWW-Authenticate exits 3 and names the sign-in page', async () => {
  const box = sandbox(hub);
  hub.inject({
    method: 'POST',
    path: CREATE,
    status: 401,
    headers: {
      'www-authenticate':
        'Bearer realm="sliccy", authorization_uri="https://login.example.test/authorize"',
    },
    body: { error: 'sign in to publish', code: 'SIGN_IN_REQUIRED' },
  });
  const res = await publish(box);
  assert.equal(res.code, 3);
  assert.equal(
    res.stderr,
    'scserve: publishing: the serve hub refused it (401: sign in to publish (SIGN_IN_REQUIRED))\nscserve: sign in at https://login.example.test/authorize\n'
  );
});

test('403 with a Link rel="login" exits 3 and names it', async () => {
  const box = sandbox(hub);
  hub.inject({
    method: 'POST',
    path: CREATE,
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

test('a saved host whose secret is refused is recreated once', async () => {
  const box = sandbox(hub);
  const saved = await box.saveHost(hub, {
    hostId: (await hub.createHost()).hostId,
    secret: 'f'.repeat(64),
  });
  const before = hub.hits('POST', CREATE);
  const res = await publish(box);
  assert.equal(res.code, 0, res.stderr);
  assert.equal(hub.hits('POST', CREATE) - before, 1);
  const host = box.host();
  assert.notEqual(host.hostId, saved.hostId);
  assert.ok(res.stdout.includes(`//${host.hostId}-`));
});

test('a recreated host that is refused again exits 3', async () => {
  const box = sandbox(hub);
  await box.saveHost(hub);
  hub.inject({ upgrade: true, path: socket, status: 401, body: { error: 'nope' }, times: 2 });
  const res = await publish(box);
  assert.equal(res.code, 3);
  assert.match(res.stderr, /^scserve: publishing: the serve hub refused it \(401: nope\)\n$/);
});

test('a refused mint on a saved host recreates the host and mints again', async () => {
  const box = sandbox(hub);
  const saved = await box.saveHost(hub);
  hub.inject({
    method: 'POST',
    path: preview,
    status: 401,
    body: { error: 'wrong or missing secret', code: 'UNAUTHORIZED' },
  });
  const res = await publish(box);
  assert.equal(res.code, 0, res.stderr);
  assert.notEqual(box.host().hostId, saved.hostId);
  const page = await hub.visit(res.stdout.trim());
  assert.equal(page.status, 200);
});

test('402 exits 4 and shows Retry-After', async () => {
  const box = sandbox(hub);
  hub.inject({
    method: 'POST',
    path: CREATE,
    status: 402,
    headers: { 'retry-after': '30' },
    body: { error: 'Payment required', code: 'PLAN_REQUIRED' },
  });
  const res = await publish(box);
  assert.equal(res.code, 4);
  assert.equal(
    res.stderr,
    "scserve: publishing: the serve hub's limit is reached (402: Payment required (PLAN_REQUIRED))\nscserve: retry after 30 s\n"
  );
});

test('429 exits 4 with a date in Retry-After', async () => {
  const box = sandbox(hub);
  hub.inject({
    method: 'POST',
    path: preview,
    status: 429,
    headers: { 'retry-after': 'Wed, 21 Oct 2026 07:28:00 GMT' },
    body: { error: 'Too many previews', code: 'PREVIEW_LIMIT' },
  });
  const res = await publish(box);
  assert.equal(res.code, 4);
  assert.match(res.stderr, /limit is reached \(429: Too many previews \(PREVIEW_LIMIT\)\)\n/);
  assert.match(res.stderr, /retry after Wed, 21 Oct 2026 07:28:00 GMT\n$/);
});

test('a deleted saved host (410 HOST_GONE) is recreated once', async () => {
  const box = sandbox(hub);
  const saved = await box.saveHost(hub);
  hub.hosts.get(saved.hostId).deleted = true;
  const res = await publish(box);
  assert.equal(res.code, 0, res.stderr);
  assert.notEqual(box.host().hostId, saved.hostId);
});

test('a saved host the hub does not know (404 HOST_NOT_FOUND) is recreated once', async () => {
  const box = sandbox(hub);
  const saved = await box.saveHost(hub, { hostId: '0'.repeat(20), secret: '1'.repeat(64) });
  const res = await publish(box);
  assert.equal(res.code, 0, res.stderr);
  assert.notEqual(box.host().hostId, saved.hostId);
});

test('404 on a fresh host is not retried', async () => {
  const box = sandbox(hub);
  hub.inject({
    upgrade: true,
    path: socket,
    status: 404,
    body: { error: 'no such host', code: 'HOST_NOT_FOUND' },
  });
  const before = hub.hits('POST', CREATE);
  const res = await publish(box);
  assert.equal(res.code, 1);
  assert.equal(
    res.stderr,
    'scserve: publishing: the serve hub answered 404: no such host (HOST_NOT_FOUND)\n'
  );
  assert.equal(hub.hits('POST', CREATE) - before, 1);
});

test('other 4xx exit 1 with the hub JSON error', async () => {
  const box = sandbox(hub);
  hub.inject({
    method: 'POST',
    path: preview,
    status: 400,
    body: { error: 'invalid entry', code: 'INVALID_ENTRY' },
  });
  const res = await publish(box);
  assert.equal(res.code, 1);
  assert.equal(
    res.stderr,
    'scserve: publishing: the serve hub answered 400: invalid entry (INVALID_ENTRY)\n'
  );
});

test('5xx is retried after 1, 2 and 5 s, then exits 1', async () => {
  const box = sandbox(hub);
  const before = hub.hits('POST', CREATE);
  hub.inject({ method: 'POST', path: CREATE, status: 503, body: 'Service Unavailable', times: 4 });
  const started = Date.now();
  const res = await publish(box);
  assert.equal(res.code, 1);
  assert.equal(
    res.stderr,
    'scserve: publishing: the serve hub answered 503 4 times: Service Unavailable\n'
  );
  assert.equal(hub.hits('POST', CREATE) - before, 4);
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
    path: socket,
    status: 302,
    headers: { location: 'https://auth.example.test/ws-login' },
  });
  const res = await publish(box);
  assert.equal(res.code, 3);
  assert.equal(
    res.stderr,
    'scserve: the serve hub wants you to sign in: https://auth.example.test/ws-login\n'
  );
});

test('the WebSocket upgrade: a refused saved host is recreated once', async () => {
  const box = sandbox(hub);
  const saved = await box.saveHost(hub);
  hub.inject({
    upgrade: true,
    path: socket,
    status: 403,
    body: { error: 'forbidden', code: 'FORBIDDEN' },
  });
  const res = await publish(box);
  assert.equal(res.code, 0, res.stderr);
  assert.notEqual(box.host().hostId, saved.hostId);
});

test('the WebSocket upgrade: 429 exits 4', async () => {
  const box = sandbox(hub);
  hub.inject({
    upgrade: true,
    path: socket,
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
    path: socket,
    status: 307,
    headers: { location: '/api/serve/moved/ws' },
  });
  const res = await publish(box);
  assert.equal(res.code, 1);
  assert.match(res.stderr, /the serve hub answered 404: not found \(NOT_FOUND\)/);
  const moved = hub.requests.find((r) => r.upgrade && r.path === '/api/serve/moved/ws');
  assert.match(moved.headers.authorization, /^Bearer [0-9a-f]{64}$/);
});

test('the WebSocket upgrade: 5xx is retried', async () => {
  const box = sandbox(hub);
  hub.inject({ upgrade: true, path: socket, status: 502, body: 'bad gateway', times: 1 });
  const res = await publish(box);
  assert.equal(res.code, 0, res.stderr);
});

test('an unreachable hub exits 1 after the retries', async () => {
  const box = sandbox();
  const res = await box.run([box.site()], { SLICC_SERVE_HUB: 'http://127.0.0.1:9' });
  assert.equal(res.code, 1);
  assert.match(res.stderr, /^scserve: publishing: the serve hub is unreachable: /);
});
