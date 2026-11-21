import assert from 'node:assert/strict';
import { request } from 'node:http';
import { after, test } from 'node:test';
import { fakeHub } from './fake-hub.mjs';
import { eventually, sandbox } from './helpers.mjs';

const hub = await fakeHub();
after(() => hub.close());

function previewOf(url) {
  return hub.previews().find((p) => p.url.startsWith(new URL(url).origin));
}

function raw(port, path, headers = {}) {
  return new Promise((resolve, reject) => {
    const r = request({ host: '127.0.0.1', port, path, headers }, (res) => {
      const chunks = [];
      res.on('data', (c) => chunks.push(c));
      res.on('end', () =>
        resolve({ status: res.statusCode, body: Buffer.concat(chunks).toString() })
      );
    });
    r.on('error', reject);
    r.end();
  });
}

async function cdp(wsUrl) {
  const ws = new WebSocket(wsUrl);
  const events = [];
  const waiting = new Map();
  const watchers = [];
  let next = 1;
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = reject;
  });
  ws.onmessage = (m) => {
    const msg = JSON.parse(m.data);
    if (msg.id !== undefined) {
      waiting.get(msg.id)?.(msg);
      waiting.delete(msg.id);
      return;
    }
    events.push(msg);
    for (const w of [...watchers]) {
      if (w.test(msg)) {
        watchers.splice(watchers.indexOf(w), 1);
        w.resolve(msg);
      }
    }
  };
  return {
    events,
    send(method, params = {}, sessionId) {
      const id = next++;
      return new Promise((resolve) => {
        waiting.set(id, resolve);
        ws.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
      });
    },
    event(method, test = () => true) {
      const found = events.find((e) => e.method === method && test(e));
      if (found) return Promise.resolve(found);
      return new Promise((resolve) =>
        watchers.push({ test: (e) => e.method === method && test(e), resolve })
      );
    },
    close: () => ws.close(),
  };
}

async function bridged(box, args = [], files = { 'index.html': '<h1>bridged</h1>' }) {
  const res = await box.run(['--bridge', '--json', ...args, box.site(files)]);
  assert.equal(res.code, 0, res.stderr);
  const out = JSON.parse(res.stdout);
  const cdpUrl = new URL(out.cdpUrl);
  return {
    out,
    url: out.url,
    cdpUrl,
    port: Number(cdpUrl.port),
    secret: cdpUrl.pathname.split('/')[1],
  };
}

test('--bridge mints with the bridge, and serves a guarded CDP endpoint on the loopback', async () => {
  const box = sandbox(hub);
  const site = box.site({ 'index.html': 'x' });
  const plain = await box.run(['--bridge', site]);
  assert.equal(plain.code, 0, plain.stderr);
  assert.match(
    plain.stderr,
    /\nscserve: its visitor tabs are CDP targets at http:\/\/127\.0\.0\.1:\d+\/[0-9a-f]{32}\/\n/
  );
  const { out, url, port, secret } = await bridged(box, ['--max-tabs', '3', '--quiet']);
  assert.equal(out.bridge, true);
  assert.match(out.cdpUrl, new RegExp(`^http://127\\.0\\.0\\.1:${port}/${secret}/$`));
  assert.ok(port < 9222 || port > 9229);
  const mint = previewOf(url).mint;
  assert.equal(mint.bridge, true);
  assert.equal(mint.maxTabs, 3);
  assert.equal(mint.quiet, true);
  assert.match(mint.webhookId, /^scserve-[0-9a-f]{16}$/);
  assert.equal(previewOf(plain.stdout.trim()).mint.maxTabs, 20);
  assert.equal(box.relay().cdpSecret, secret);

  const version = await raw(port, `/${secret}/json/version/`, { host: `127.0.0.1:${port}` });
  assert.equal(version.status, 200);
  const v = JSON.parse(version.body);
  assert.equal(v.webSocketDebuggerUrl, `ws://127.0.0.1:${port}/${secret}/devtools/browser`);
  assert.match(v.Browser, /^scserve\//);
  assert.deepEqual(
    JSON.parse((await raw(port, `/${secret}/json/list`, { host: `localhost:${port}` })).body),
    []
  );
  assert.equal((await raw(port, '/json/version', { host: `127.0.0.1:${port}` })).status, 404);
  assert.equal(
    (await raw(port, `/${'0'.repeat(32)}/json/list`, { host: `127.0.0.1:${port}` })).status,
    404
  );
  assert.equal(
    (await raw(port, `/${secret}/json/list`, { host: `evil.example:${port}` })).status,
    403
  );
  const page = await raw(port, `/${secret}/devtools/browser`, {
    host: `127.0.0.1:${port}`,
    origin: 'https://evil.example',
    connection: 'Upgrade',
    upgrade: 'websocket',
    'sec-websocket-version': '13',
    'sec-websocket-key': 'dGhlIHNhbXBsZSBub25jZQ==',
  });
  assert.equal(page.status, 403);
  const list = JSON.parse((await box.run(['--list', '--json'])).stdout);
  assert.ok(list.every((r) => r.cdpUrl === out.cdpUrl));
});

test('visitor tabs become CDP targets that a client drives through the hub', async () => {
  const box = sandbox(hub);
  const { out, url, cdpUrl } = await bridged(box);
  const token = out.previewToken;
  const tab = await hub.tab(url, {
    origin: new URL(url).origin,
    answer: (frame) =>
      frame.method === 'Runtime.evaluate'
        ? { result: { result: { type: 'number', value: 42 } } }
        : frame.method === 'Page.navigate'
          ? { result: { frameId: 'cherry-frame', loaderId: 'cherry-loader' } }
          : frame.method === 'Never.answers'
            ? null
            : { error: { code: -32601, message: `${frame.method} is not supported` } },
  });
  const target = `preview:${token}:${tab.connId}`;
  const listed = await eventually(async () => {
    const l = JSON.parse(
      (await raw(cdpUrl.port, `${cdpUrl.pathname}json/list`, { host: cdpUrl.host })).body
    );
    return l.length === 1 && l;
  });
  assert.equal(listed[0].id, target);
  assert.equal(listed[0].type, 'page');
  assert.match(listed[0].description, /Visitor\/1\.0/);

  const version = JSON.parse(
    (await raw(cdpUrl.port, `${cdpUrl.pathname}json/version`, { host: cdpUrl.host })).body
  );
  const c = await cdp(version.webSocketDebuggerUrl);
  assert.equal(
    (await c.send('Target.setDiscoverTargets', { discover: true })).result !== undefined,
    true
  );
  const created = await c.event('Target.targetCreated');
  assert.equal(created.params.targetInfo.targetId, target);
  const targets = await c.send('Target.getTargets');
  assert.deepEqual(
    targets.result.targetInfos.map((t) => t.targetId),
    [target]
  );
  const attached = await c.send('Target.attachToTarget', { targetId: target, flatten: true });
  const session = attached.result.sessionId;
  assert.ok(session);
  await c.event('Target.attachedToTarget', (e) => e.params.sessionId === session);
  assert.deepEqual((await c.send('Page.enable', {}, session)).result, {});
  await c.send('Runtime.enable', {}, session);
  await c.event('Runtime.executionContextCreated', (e) => e.sessionId === session);
  const tree = await c.send('Page.getFrameTree', {}, session);
  assert.equal(tree.result.frameTree.frame.url, url);
  const evaluated = await c.send('Runtime.evaluate', { expression: '6*7' }, session);
  assert.deepEqual(evaluated.result, { result: { type: 'number', value: 42 } });
  assert.equal(evaluated.sessionId, session);
  const sent = tab.requests.at(-1);
  assert.equal(sent.method, 'Runtime.evaluate');
  assert.equal(sent.sessionId, session);
  assert.ok(sent.params.expression.startsWith('Promise.resolve((0, eval)("6*7"))'));
  const unsupported = await c.send('DOM.describeNode', {}, session);
  assert.deepEqual(unsupported.error, {
    code: -32601,
    message: 'DOM.describeNode is not supported',
  });
  const navigated = await c.send('Page.navigate', { url: `${url}next.html` }, session);
  assert.equal(navigated.result.frameId, 'preview-frame');
  const frame = await c.event('Page.frameNavigated', (e) => e.sessionId === session);
  assert.equal(frame.params.frame.url, `${url}next.html`);
  await c.event('Page.loadEventFired', (e) => e.sessionId === session);
  assert.equal((await c.send('Target.getTargets')).result.targetInfos[0].url, `${url}next.html`);
  assert.equal((await c.send('Target.createTarget', { url: 'about:blank' })).error.code, -32000);
  assert.equal((await c.send('Nope.nothing')).error.code, -32601);
  assert.equal((await c.send('Runtime.evaluate', {}, 'no-such-session')).error.code, -32001);

  const hanging = c.send('Never.answers', {}, session);
  await eventually(() => tab.requests.some((r) => r.method === 'Never.answers'));
  tab.close();
  assert.deepEqual((await hanging).error, { code: -32000, message: "the visitor's tab is gone" });
  await c.event('Target.detachedFromTarget', (e) => e.params.sessionId === session);
  await c.event('Target.targetDestroyed', (e) => e.params.targetId === target);
  assert.deepEqual((await c.send('Target.getTargets')).result.targetInfos, []);
  c.close();
});

test('auto-attach, closeTarget and --stop end the visitor tabs', async () => {
  const box = sandbox(hub);
  const { url, cdpUrl } = await bridged(box);
  const c = await cdp(`ws://${cdpUrl.host}${cdpUrl.pathname}devtools/browser`);
  await c.send('Target.setAutoAttach', {
    autoAttach: true,
    flatten: true,
    waitForDebuggerOnStart: false,
  });
  const first = await hub.tab(url);
  const auto = await c.event('Target.attachedToTarget', (e) =>
    e.params.targetInfo.targetId.endsWith(first.connId)
  );
  assert.ok(auto.params.sessionId);
  const closed = await c.send('Target.closeTarget', { targetId: auto.params.targetInfo.targetId });
  assert.deepEqual(closed.result, { success: true });
  await eventually(() => first.closed);
  const second = await hub.tab(url);
  await c.event('Target.attachedToTarget', (e) =>
    e.params.targetInfo.targetId.endsWith(second.connId)
  );
  const stop = await box.run(['--stop', url]);
  assert.equal(stop.code, 0, stop.stderr);
  await eventually(() => second.closed);
  await c.event('Target.detachedFromTarget', (e) => e.params.targetId.endsWith(second.connId));
  c.close();
});

test('a --foreground --bridge scserve reports connects, emits and the first visit', async () => {
  const box = sandbox(hub);
  const proc = box.start(['--bridge', '--foreground', '--json', box.site({ 'index.html': 'fg' })]);
  const [line] = await proc.stdout(/^\{.*\}$/m);
  const record = JSON.parse(line);
  assert.match(record.cdpUrl, /^http:\/\/127\.0\.0\.1:\d+\/[0-9a-f]{32}\/$/);
  const tab = await hub.tab(record.url, { origin: 'https://a.example', userAgent: 'UA-A' });
  await proc.stderr(
    new RegExp(
      `first visitor on ${record.url.replaceAll('.', '\\.')}: https://a\\.example \\(UA-A\\)`
    )
  );
  await proc.stderr(new RegExp(`scserve: connected ${tab.connId} https://a\\.example UA-A`));
  await eventually(() => previewOf(record.url).announced === true);
  tab.emit('clicked', { button: 'buy', n: 2 });
  await proc.stderr(new RegExp(`scserve: emit ${tab.connId} clicked \\{"button":"buy","n":2\\}`));
  tab.emit('\u001b]0;pwned\u0007', '\u009b2J');
  await proc.stderr(/scserve: emit \S+ \\u001b\]0;pwned\\u0007 "\\u009b2J"\n/);
  assert.doesNotMatch(proc.err, /\u001b|\u0007|\u009b/);
  const again = await hub.tab(record.url, { origin: 'https://b.example', userAgent: 'UA-B' });
  await proc.stderr(new RegExp(`scserve: connected ${again.connId}`));
  again.close();
  await proc.stderr(new RegExp(`scserve: disconnected ${again.connId}`));
  const events = await eventually(() => {
    const lines = proc.out
      .trim()
      .split('\n')
      .slice(1)
      .map((l) => JSON.parse(l));
    return lines.length >= 5 && lines;
  });
  assert.deepEqual(
    events.map((e) => [e.event, e.connId, e.announced]),
    [
      ['connected', tab.connId, true],
      ['emit', tab.connId, false],
      ['emit', tab.connId, false],
      ['connected', again.connId, false],
      ['disconnected', again.connId, false],
    ]
  );
  assert.equal(events[1].name, 'clicked');
  assert.equal(events[1].detail, '{"button":"buy","n":2}');
  assert.equal((proc.err.match(/first visitor/g) ?? []).length, 1);

  const logs = await box.run(['--logs', record.url]);
  assert.equal(logs.code, 0, logs.stderr);
  const rows = logs.stdout.trim().split('\n');
  assert.equal(rows[0], 'Preview lifecycle records (oldest to newest):');
  assert.match(
    rows[1],
    new RegExp(
      `connected\\s+announced\\s+preview=${record.previewToken.replace('.', '\\.')} conn=${tab.connId} origin=https://a\\.example userAgent=UA-A$`
    )
  );
  assert.match(rows[2], /emit\s+-\s+preview=.* name=clicked detail=\{"button":"buy","n":2\}$/);
  assert.match(rows[3], /emit\s+-\s+.* name=\\u001b\]0;pwned\\u0007 detail="\\u009b2J"$/);
  assert.doesNotMatch(logs.stdout, /\u001b|\u0007|\u009b/);
  assert.match(rows[4], /connected\s+suppressed\s/);
  assert.match(rows[5], /disconnected\s+-\s+.*reason=-$/);
  const last = JSON.parse((await box.run(['--logs', '--lines', '1', '--json'])).stdout);
  assert.deepEqual(
    last.map((r) => r.event),
    ['disconnected']
  );
  const truncated = await box.run(['--truncate', record.previewToken]);
  assert.equal(truncated.stdout, 'cleared 5 records, re-armed 1 previews\n');
  await eventually(() => previewOf(record.url).announced === false);
  assert.match((await box.run(['--logs'])).stdout, /^no lifecycle records/);
  const third = await hub.tab(record.url, { origin: 'https://c.example', userAgent: 'UA-C' });
  await proc.stderr(/first visitor on .*: https:\/\/c\.example \(UA-C\)/);
  assert.deepEqual(JSON.parse((await box.run(['--truncate', '--json'])).stdout), {
    cleared: 1,
    rearmed: 1,
  });
  third.close();
  proc.child.kill();
  await proc.exited;
});

test('--quiet records the first visit without announcing it, and --max-tabs caps the tabs', async () => {
  const box = sandbox(hub);
  const proc = box.start(['--bridge', '--quiet', '--max-tabs', '1', '--foreground', box.site()]);
  const [url] = await proc.stdout(/^http:\/\/\S+$/m);
  const tab = await hub.tab(url);
  await proc.stderr(new RegExp(`scserve: connected ${tab.connId}`));
  assert.doesNotMatch(proc.err, /first visitor/);
  await eventually(() => previewOf(url).announced === true);
  const refused = await hub.tab(url);
  assert.equal(refused.refused, 429);
  assert.match(refused.body, /BRIDGE_CAP/);
  const logs = JSON.parse((await box.run(['--logs', '--json'])).stdout);
  assert.deepEqual(
    logs.map((r) => [r.event, r.announced]),
    [['connected', false]]
  );
  assert.deepEqual(await box.run(['--logs', 'nothing']), {
    code: 1,
    stdout: '',
    stderr: 'scserve: no preview matches nothing\n',
  });
  proc.child.kill();
  await proc.exited;
});

test('--logs and --truncate without a relay find nothing', async () => {
  const box = sandbox(hub);
  assert.deepEqual(await box.run(['--logs', '--json']), { code: 0, stdout: '[]\n', stderr: '' });
  assert.deepEqual(await box.run(['--truncate']), {
    code: 0,
    stdout: 'cleared 0 records, re-armed 0 previews\n',
    stderr: '',
  });
});

test('a preview without --bridge gets no bridge socket', async () => {
  const box = sandbox(hub);
  const res = await box.run([box.site()]);
  const tab = await hub.tab(res.stdout.trim());
  assert.equal(tab.refused, 403);
  assert.doesNotMatch(res.stderr, /CDP/);
});
