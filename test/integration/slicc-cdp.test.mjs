import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { after, before, test } from 'node:test';
import { fakeHub } from './fake-hub.mjs';
import { eventually, sandbox } from './helpers.mjs';
import { run, tool } from './slicc-cdp.mjs';
import { visitor } from './visitor.mjs';

const hub = await fakeHub();
after(() => hub.close());

let playwright;
let curlwright;

before(async () => {
  [playwright, curlwright] = await Promise.all([tool('playwright-cli'), tool('curlwright')]);
});

async function bridgedTab(box, page = {}) {
  const res = await box.run([
    '--bridge',
    '--json',
    box.site({ 'index.html': '<h1>hi</h1>', 'data.json': '{"answer":42}' }),
  ]);
  assert.equal(res.code, 0, res.stderr);
  const out = JSON.parse(res.stdout);
  const fetchVia = async (url) => {
    const got = await hub.visit(url);
    return new Response(got.body, {
      status: got.status,
      headers: { 'content-type': got.headers['content-type'] ?? 'text/plain' },
    });
  };
  const v = visitor({ href: out.url, fetch: fetchVia, ...page });
  const tab = await hub.tab(out.url, {
    origin: new URL(out.url).origin,
    answer: (f) => v.answer(f),
  });
  const cdp = new URL(out.cdpUrl);
  const ws = `ws://${cdp.host}${cdp.pathname}devtools/browser`;
  const target = `preview:${out.previewToken}:${tab.connId}`;
  await eventually(async () => {
    const list = await (await fetch(`${out.cdpUrl}json/list`)).json();
    return list.some((t) => t.id === target);
  });
  return { out, tab, v, ws, target };
}

test("slicc-cdp's playwright-cli drives a visitor tab through the bridge", async () => {
  const box = sandbox(hub);
  const snapshot = {
    json: [{ role: 'button', name: 'Buy', ref: 'e1' }],
    refs: [{ ref: 'e1', selector: '#buy' }],
  };
  const { out, v, ws, target } = await bridgedTab(box, {
    title: 'Bridged page',
    snapshot,
    elements: { '#buy': {} },
  });
  const pw = (...args) => run(playwright, ['--cdp', ws, ...args], box.dir);

  const list = await pw('tab-list');
  assert.equal(list.code, 0, list.stderr);
  assert.match(
    list.stdout,
    new RegExp(`\\[${target.replaceAll('.', '\\.')}\\] ${out.url.replaceAll('.', '\\.')} `)
  );

  const title = await pw('eval', 'document.title');
  assert.equal(title.code, 0, title.stderr);
  assert.equal(title.stdout.trim(), 'Bridged page');
  const value = await pw('eval', '() => ({ a: [1, 2], b: "x" })');
  assert.equal(value.code, 0, value.stderr);
  assert.deepEqual(JSON.parse(value.stdout), { a: [1, 2], b: 'x' });

  const snap = await pw('snapshot');
  assert.equal(snap.code, 0, snap.stderr);
  assert.match(
    snap.stdout,
    new RegExp(`- Page URL: ${out.url.replaceAll('.', '\\.')}\\n- Page Title: Bridged page`)
  );
  assert.match(snap.stdout, /- button "Buy" \[ref=e1\]/);

  const file = join(box.dir, 'shot.png');
  const shot = await pw('screenshot', '--filename', file);
  assert.equal(shot.code, 0, shot.stderr);
  assert.ok(readFileSync(file).subarray(0, 8).equals(Buffer.from('89504e470d0a1a0a', 'hex')));

  const next = `${out.url}next.html`;
  const goto = await pw('goto', next);
  assert.equal(goto.code, 0, goto.stderr);
  assert.match((await pw('tab-list')).stdout, new RegExp(next.replaceAll('.', '\\.')));

  for (const method of ['Runtime.evaluate', 'Page.captureScreenshot', 'Page.navigate']) {
    assert.ok(v.seen.includes(method), method);
  }
  assert.ok(!v.seen.includes('Network.getAllCookies'));
  assert.ok(!v.seen.includes('Fetch.enable'));
});

test("slicc-cdp's curlwright fetches from inside a visitor tab", async () => {
  const box = sandbox(hub);
  const { out, ws } = await bridgedTab(box);
  const res = await run(curlwright, ['--cdp', ws, '-i', `${out.url}data.json`], box.dir);
  assert.equal(res.code, 0, res.stderr);
  assert.match(res.stdout, /^HTTP\/1\.1 200\r?\n/);
  assert.match(res.stdout, /\{"answer":42\}\s*$/);
});

test('a bridged tab keeps its target id across a relay reconnect, and a tab gone meanwhile is dropped', async () => {
  const box = sandbox(hub);
  const { out, ws, target, tab } = await bridgedTab(box);
  const other = await hub.tab(out.url, { origin: new URL(out.url).origin });
  const second = `preview:${out.previewToken}:${other.connId}`;
  await eventually(async () => (await (await fetch(`${out.cdpUrl}json/list`)).json()).length === 2);
  const host = hub.hosts.get(box.host().hostId);
  const path = `/api/serve/host/${host.hostId}/ws`;
  const before = hub.hits('GET', path);
  host.socket.close();
  await eventually(() => !host.socket);
  other.close();
  await eventually(() => hub.hits('GET', path) > before && host.socket, 20_000);
  const ids = await eventually(async () => {
    const list = await (await fetch(`${out.cdpUrl}json/list`)).json();
    return list.length === 1 && list.map((t) => t.id);
  }, 20_000);
  assert.deepEqual(ids, [target]);
  assert.ok(!tab.closed);
  const res = await run(playwright, ['--cdp', ws, 'eval', 'document.title'], box.dir);
  assert.equal(res.code, 0, res.stderr);
  assert.equal(res.stdout.trim(), 'Bridged page');
  const logs = JSON.parse((await box.run(['--logs', '--json'])).stdout);
  const gone = logs.find((r) => r.event === 'disconnected' && r.connId === other.connId);
  assert.ok(gone, JSON.stringify(logs));
  assert.match(gone.reason, /no longer had it/);
  assert.equal(logs.filter((r) => r.event === 'connected' && r.connId === tab.connId).length, 1);
  assert.ok(!ids.includes(second));
});
