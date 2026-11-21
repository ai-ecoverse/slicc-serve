import assert from 'node:assert/strict';
import { join } from 'node:path';
import { after, before, test } from 'node:test';
import { installBrowser, launchBrowser } from '@ai-ecoverse/slicc-cdp';
import { fakeHub } from './fake-hub.mjs';
import { eventually, root, sandbox } from './helpers.mjs';
import { CHROME, run, tool } from './slicc-cdp.mjs';

const PAGE = `<!doctype html>
<html>
  <head><title>Shop</title></head>
  <body>
    <button id="buy" onclick="window.clicks = (window.clicks || 0) + 1">Buy</button>
    <form id="f" onsubmit="event.preventDefault(); window.submits = (window.submits || 0) + 1">
      <input id="name" aria-label="Name">
      <button id="send" type="submit">Send</button>
    </form>
    <textarea id="notes" aria-label="Notes"></textarea>
  </body>
</html>
`;

async function cdp(url) {
  const ws = new WebSocket(url);
  let next = 0;
  const waiting = new Map();
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = reject;
  });
  ws.onmessage = (m) => {
    const msg = JSON.parse(m.data);
    if (msg.id === undefined) return;
    waiting.get(msg.id)?.(msg);
    waiting.delete(msg.id);
  };
  return {
    send(method, params = {}, sessionId) {
      const id = ++next;
      return new Promise((resolve) => {
        waiting.set(id, resolve);
        ws.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
      });
    },
    close: () => ws.close(),
  };
}

const hub = await fakeHub();
let browser;
let playwright;
const cache = join(root, 'node_modules', '.cache');

before(async () => {
  playwright = await tool('playwright-cli');
  const installed = await installBrowser({ version: CHROME, root: join(cache, 'browsers') });
  browser = await launchBrowser({ executable: installed.executable });
});

after(async () => {
  await browser?.close();
  await hub.close();
});

async function visit(box) {
  const res = await box.run(['--bridge', '--json', box.site({ 'index.html': PAGE })]);
  assert.equal(res.code, 0, res.stderr);
  const out = JSON.parse(res.stdout);
  const visitor = await cdp(browser.wsUrl);
  const created = await visitor.send('Target.createTarget', { url: out.url });
  const attached = await visitor.send('Target.attachToTarget', {
    targetId: created.result.targetId,
    flatten: true,
  });
  const session = attached.result.sessionId;
  const page = async () => {
    const got = await visitor.send(
      'Runtime.evaluate',
      {
        expression:
          'JSON.stringify({ clicks: window.clicks ?? 0, submits: window.submits ?? 0, name: document.querySelector("#name").value, notes: document.querySelector("#notes").value, focused: document.activeElement && document.activeElement.id })',
        returnByValue: true,
      },
      session
    );
    return JSON.parse(got.result.result.value);
  };
  const [target] = await eventually(async () => {
    const list = await (await fetch(`${out.cdpUrl}json/list`)).json();
    return list.length === 1 && list;
  }, 30_000);
  const cdpUrl = new URL(out.cdpUrl);
  const ws = `ws://${cdpUrl.host}${cdpUrl.pathname}devtools/browser`;
  const close = async () => {
    await visitor.send('Target.closeTarget', { targetId: created.result.targetId });
    visitor.close();
  };
  return { out, ws, target, page, close };
}

function refOf(snapshot, label) {
  const line = snapshot.split('\n').find((l) => l.includes(label));
  assert.ok(line, `${label} in\n${snapshot}`);
  return line.match(/\[ref=(e\d+)\]/)[1];
}

test("playwright-cli clicks, fills and types in a real visitor's tab, each exactly once", async () => {
  const box = sandbox(hub);
  const { ws, target, page, close } = await visit(box);
  const pw = async (...args) => {
    const res = await run(playwright, ['--cdp', ws, ...args], box.dir);
    assert.equal(res.code, 0, `${args.join(' ')}: ${res.stderr}`);
    return res.stdout;
  };
  assert.match(await pw('tab-list'), new RegExp(target.id.replaceAll('.', '\\.')));
  const snap = await pw('snapshot');
  assert.match(snap, /- Page Title: Shop\n/);
  assert.match(snap, /- button "Buy" \[ref=e\d+\]/);
  assert.match(snap, /- textbox "Name" \[ref=e\d+\]/);

  await pw('click', refOf(snap, 'button "Buy"'));
  assert.equal((await page()).clicks, 1);

  await pw('fill', refOf(await pw('snapshot'), 'textbox "Name"'), 'Ada');
  assert.equal((await page()).name, 'Ada');

  await pw('click', refOf(await pw('snapshot'), 'button "Send"'));
  assert.equal((await page()).submits, 1);

  await pw('click', refOf(await pw('snapshot'), 'textbox "Notes"'));
  assert.equal((await page()).focused, 'notes');
  await pw('type', 'hi there');
  assert.equal((await page()).notes, 'hi there');

  await pw('click', refOf(await pw('snapshot'), 'textbox "Name"'));
  await pw('press', 'Enter');
  assert.deepEqual(await page(), {
    clicks: 1,
    submits: 2,
    name: 'Ada',
    notes: 'hi there',
    focused: 'name',
  });
  assert.equal((await pw('eval', 'document.title')).trim(), 'Shop');
  await close();
});

test('a handle is the object, not its expression: calling a function on it runs the expression once', async () => {
  const box = sandbox(hub);
  const { ws, target, page, close } = await visit(box);
  const client = await cdp(ws);
  const attached = await client.send('Target.attachToTarget', {
    targetId: target.id,
    flatten: true,
  });
  const s = attached.result.sessionId;
  const clicked = await client.send(
    'Runtime.evaluate',
    { expression: '(document.querySelector("#buy").click(), document.querySelector("#buy"))' },
    s
  );
  const handle = clicked.result.result;
  assert.equal(handle.type, 'object');
  assert.equal(handle.subtype, 'node');
  assert.equal(handle.className, 'HTMLButtonElement');
  assert.ok(handle.objectId);
  const id = await client.send(
    'Runtime.callFunctionOn',
    {
      objectId: handle.objectId,
      functionDeclaration: 'function () { return this.id; }',
      returnByValue: true,
    },
    s
  );
  assert.deepEqual(id.result.result, { type: 'string', value: 'buy' });
  const form = await client.send(
    'Runtime.evaluate',
    { expression: '(document.querySelector("#f").requestSubmit(), document.querySelector("#f"))' },
    s
  );
  for (let i = 0; i < 3; i++) {
    const got = await client.send(
      'Runtime.callFunctionOn',
      {
        objectId: form.result.result.objectId,
        functionDeclaration: 'function (n) { return this.elements.length + n; }',
        arguments: [{ value: i }],
        returnByValue: true,
      },
      s
    );
    assert.deepEqual(got.result.result, { type: 'number', value: 2 + i });
  }
  const props = await client.send(
    'Runtime.getProperties',
    { objectId: handle.objectId, ownProperties: true },
    s
  );
  assert.ok(Array.isArray(props.result.result));
  const described = await client.send('DOM.describeNode', { objectId: handle.objectId }, s);
  assert.equal(described.result.node.nodeName, 'BUTTON');
  const resolved = await client.send(
    'DOM.resolveNode',
    { backendNodeId: described.result.node.backendNodeId },
    s
  );
  const same = await client.send(
    'Runtime.callFunctionOn',
    {
      objectId: resolved.result.object.objectId,
      functionDeclaration: 'function (other) { return this === other; }',
      arguments: [{ objectId: handle.objectId }],
      returnByValue: true,
    },
    s
  );
  assert.deepEqual(same.result.result, { type: 'boolean', value: true });
  const odd = await client.send(
    'Runtime.evaluate',
    { expression: '[NaN, -0, 1/0]', returnByValue: true },
    s
  );
  assert.deepEqual(odd.result.result, { type: 'object', subtype: 'array', value: [null, 0, null] });
  const nan = await client.send('Runtime.evaluate', { expression: 'NaN', returnByValue: true }, s);
  assert.deepEqual(nan.result.result, { type: 'number', unserializableValue: 'NaN' });
  await client.send('Runtime.releaseObject', { objectId: handle.objectId }, s);
  const released = await client.send(
    'Runtime.callFunctionOn',
    {
      objectId: handle.objectId,
      functionDeclaration: 'function () { return 1; }',
      returnByValue: true,
    },
    s
  );
  assert.equal(released.error?.code, -32000);
  const input = await client.send(
    'Input.dispatchMouseEvent',
    { type: 'mousePressed', x: 20, y: 15, button: 'left', clickCount: 1 },
    s
  );
  assert.deepEqual(input.result, {});
  const after = await page();
  assert.equal(after.clicks, 1);
  assert.equal(after.submits, 1);
  client.close();
  await close();
});
