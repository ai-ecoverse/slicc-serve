import assert from 'node:assert/strict';
import { randomBytes, randomUUID } from 'node:crypto';
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { after, before, beforeEach, describe, it } from 'node:test';
import { eventually, expectedVersion, root } from './helpers.mjs';
import { boot, bootOld } from './kernel.mjs';

const skip = !existsSync(join(root, 'bin', 'serve.wasm')) && 'run npm run build:wasm first';
const STATE = '/root/.local/state/slicc/serve';
const hint =
  "serve: if seven asked, answer in seven's tab (a click allows it), then run serve again\n";
const encoder = new TextEncoder();
const page = { calls: [], fail: undefined, answer: undefined, previews: new Map() };

function refusal(name, message) {
  return Object.assign(new Error(message), { name });
}

function mint(entry = 'index.html') {
  const uuid = randomUUID();
  const secret = randomBytes(8).toString('hex');
  return {
    token: `${uuid}.${secret}`,
    url: `https://${uuid.replaceAll('-', '')}--${secret}.sliccy.now/${entry}`,
  };
}

function publish(request) {
  const { token, url } = mint(request.options.entry);
  let controller;
  const preview = {
    token,
    url,
    options: request.options,
    since: Date.now(),
    ended: undefined,
    end(reason) {
      if (preview.ended) return;
      preview.ended = reason ?? 'page';
      page.previews.delete(token);
      if (reason) controller.enqueue(encoder.encode(`${JSON.stringify({ stopped: reason })}\n`));
      controller.close();
    },
  };
  const gone = (why) => {
    if (preview.ended) return;
    preview.ended = `revoked: ${why}`;
    page.previews.delete(token);
  };
  request.signal.addEventListener('abort', () => gone(String(request.signal.reason?.message)));
  page.previews.set(token, preview);
  return new ReadableStream({
    start(c) {
      controller = c;
      c.enqueue(encoder.encode(`${JSON.stringify({ url, previewToken: token, trayId: 't1' })}\n`));
    },
    cancel: (reason) => gone(String(reason)),
  });
}

const requests = {
  kinds: { 'serve.publish': {}, 'serve.list': {}, 'serve.stop': {}, 'open.url': {} },
  async handle(request) {
    page.calls.push({ kind: request.kind, options: request.options });
    if (page.fail) throw page.fail;
    if (request.kind === 'serve.publish') {
      if (page.answer !== undefined) return page.answer;
      return publish(request);
    }
    if (request.kind === 'serve.list') {
      return [...page.previews.values()]
        .map((p) =>
          JSON.stringify({
            previewToken: p.token,
            url: p.url,
            port: p.options.port,
            ...(p.options.root ? { root: p.options.root } : {}),
            since: p.since,
          })
        )
        .map((l) => `${l}\n`)
        .join('');
    }
    if (request.kind === 'serve.stop') {
      const held = page.previews.get(request.options.previewToken);
      if (!held) throw refusal('NotFoundError', 'the tray hub has no such preview');
      held.end('stopped with serve --stop');
      return '';
    }
    return '';
  },
};

let os;

before(async () => {
  if (skip) return;
  os = await boot({ requests });
  await os.sh(
    "mkdir -p /w/site/app /w/outside && printf '<h1>hi</h1>' > /w/site/index.html && printf 'app' > /w/site/app/main.html && printf s > /w/outside/secret.txt && printf e > /w/site/.env && ln -s /w/outside/secret.txt /w/site/out.txt && ln -s ../outside /w/site/up"
  );
});

after(() => os?.close());

beforeEach(() => {
  page.calls = [];
  page.fail = undefined;
  page.answer = undefined;
});

const run = (argv, opts) => os.run(argv, opts);

async function records() {
  const res = await os.sh(`cat ${STATE}/previews/*.json 2>/dev/null`);
  return res.stdout
    .split(/\n(?=\{)/)
    .filter((t) => t.trim())
    .map((t) => JSON.parse(t));
}

async function gone(pid) {
  await eventually(async () => (await os.sh(`test -e /proc/${pid}`)).status !== 0, 20_000);
}

async function refused(port) {
  await eventually(async () => {
    try {
      await os.fetch(port);
      return false;
    } catch {
      return true;
    }
  }, 20_000);
}

async function stopAll() {
  for (const p of [...page.previews.values()]) p.end('cleanup');
  for (const r of await records()) await os.sh(`kill ${r.pid} 2>/dev/null; true`);
  await os.sh(`rm -f ${STATE}/previews/*.json`);
}

describe('serve in a kernel with a page', { skip }, () => {
  beforeEach(stopAll);

  it('installs serve and scserve, which name themselves', async () => {
    assert.equal(
      (await os.sh('command -v serve scserve')).stdout,
      '/usr/bin/serve\n/usr/bin/scserve\n'
    );
    assert.match((await run(['serve', '--help'])).stdout, /^usage: serve \[--entry FILE\]/);
    assert.match((await run(['scserve', '--help'])).stdout, /^usage: scserve \[--entry FILE\]/);
    assert.match((await run(['serve', '--help'])).stdout, /--stop TOKEN\|URL\|PORT/);
    assert.deepEqual(await run(['serve', '--upgrade']), {
      status: 0,
      stdout: 'pnpm add -g @ai-ecoverse/slicc-serve@latest\n',
      stderr: '',
    });
    assert.deepEqual(await run(['serve', '--version']), {
      status: 0,
      stdout: `${expectedVersion}\n`,
      stderr: '',
    });
    assert.deepEqual(await run(['serve', '--local', '--port', '9', '--entry', 'app.html']), {
      status: 0,
      stdout: 'http://9.kernel.localhost/app.html\n',
      stderr: 'serve: nothing answers on 127.0.0.1:9 yet\n',
    });
    const plain = await os.sh('unset SLICC_PAGE_LOOPBACK_URL; serve --local --port 9 --json');
    assert.equal(JSON.parse(plain.stdout).localUrl, 'http://127.0.0.1:9/');
    const usage = await run(['serve', '--nope']);
    assert.equal(usage.status, 2);
    assert.match(usage.stderr, /^serve: unknown option: --nope\nusage: serve /);
  });

  it('publishes a directory from a background serve and returns', async () => {
    const res = await run(['serve', 'site'], { cwd: '/w' });
    assert.equal(res.status, 0, res.stderr);
    const [preview] = page.previews.values();
    assert.equal(res.stdout, `${preview.url.replace(/index\.html$/, '')}\n`);
    const [record] = await records();
    assert.equal(
      res.stderr,
      `serve: /w/site is public at ${record.url} (here: http://${record.port}.kernel.localhost/)\n` +
        `serve: stop it with: serve --stop ${preview.token}\n`
    );
    assert.deepEqual(page.calls, [
      {
        kind: 'serve.publish',
        options: { port: record.port, root: '/w/site', allowLive: false },
      },
    ]);
    assert.equal(record.mode, 'live');
    assert.equal(record.previewToken, preview.token);
    assert.equal(record.trayId, 't1');
    assert.equal((await os.sh(`test -e /proc/${record.pid}`)).status, 0);
    const mode = await os.sh(`stat -c %a ${STATE}/previews/${record.id}.json ${STATE}`);
    assert.equal(mode.stdout, '600\n700\n');
    const got = await os.fetch(record.port, '/app/main.html');
    assert.equal(got.status, 200);
    assert.equal(await got.text(), 'app');
    for (const path of ['/out.txt', '/.env', '/up/secret.txt']) {
      assert.equal((await os.fetch(record.port, path)).status, 404, path);
    }

    preview.end('revoked by the tray hub');
    await gone(record.pid);
    assert.deepEqual(await records(), []);
    await refused(record.port);
  });

  it('revokes when the background serve ends, and lists and stops by token, URL and port', async () => {
    const a = await run(['serve', '--entry', 'app/main.html', '/w/site']);
    assert.equal(a.status, 0, a.stderr);
    const b = await run(['serve', '/w/site', '--json']);
    assert.equal(b.status, 0, b.stderr);
    const out = JSON.parse(b.stdout);
    assert.equal(out.mode, 'live');
    assert.equal(out.root, '/w/site');
    assert.equal(out.foreground, false);
    assert.equal(out.id, undefined);
    assert.equal(out.pid, undefined);
    assert.match(out.localUrl, /^http:\/\/\d+\.kernel\.localhost\/$/);
    assert.equal(page.calls[0].options.entry, 'app/main.html');
    assert.match(a.stdout, /\/app\/main\.html\n$/);

    const list = await run(['serve', '--list', '--json']);
    assert.equal(list.status, 0, list.stderr);
    const rows = JSON.parse(list.stdout);
    assert.deepEqual(
      rows.map((r) => [r.mode, r.root, r.foreground]),
      [
        ['live', '/w/site', false],
        ['live', '/w/site', false],
      ]
    );
    assert.ok(rows.every((r) => r.localUrl && r.createdAt));
    const table = await run(['serve', '--list']);
    assert.match(table.stdout, /^TOKEN\s+MODE\s+EXPIRES\s+URL\s+ROOT\n/);

    const recs = await records();
    const first = recs.find((r) => r.entry === 'app/main.html');
    const second = recs.find((r) => r.entry === undefined);
    const stop = await run(['serve', '--stop', String(first.port)]);
    assert.equal(stop.status, 0, stop.stderr);
    assert.equal(stop.stdout, `stopped ${first.previewToken}\n`);
    await gone(first.pid);
    assert.equal(page.calls.at(-1).kind, 'serve.stop');

    const byUrl = await run(['serve', '--stop', second.url, '--json']);
    assert.equal(byUrl.status, 0, byUrl.stderr);
    assert.deepEqual(JSON.parse(byUrl.stdout), { stopped: second.previewToken });
    await gone(second.pid);
    assert.equal((await run(['serve', '--list'])).stdout, 'no previews\n');

    const c = await run(['serve', '/w/site']);
    assert.equal(c.status, 0, c.stderr);
    const [third] = await records();
    const held = page.previews.get(third.previewToken);
    await os.sh(`kill ${third.pid}`);
    await eventually(() => held.ended?.startsWith('revoked'), 20_000);
    assert.equal(page.previews.size, 0);
  });

  it('drops records the page no longer lists and records whose process is gone', async () => {
    const res = await run(['serve', '/w/site']);
    assert.equal(res.status, 0, res.stderr);
    const [record] = await records();
    page.previews.clear();
    await os.sh(
      `printf '{"id":"dead","mode":"local","port":1,"pid":999999,"createdAt":"x"}' > ${STATE}/previews/dead.json`
    );
    assert.equal((await run(['serve', '--list'])).stdout, 'no previews\n');
    await gone(record.pid);
    assert.deepEqual(await records(), []);
  });

  it('publishes a server on the kernel loopback with --port', async () => {
    const local = await run(['serve', '--local', '/w/site']);
    assert.equal(local.status, 0, local.stderr);
    const port = Number(new URL(local.stdout.trim()).hostname.split('.')[0]);
    assert.match(local.stderr, /serving \/w\/site at .*; stop it with: serve --stop /);
    assert.deepEqual(page.calls, []);
    const res = await run(['serve', '--port', String(port), '--entry', 'app/main.html']);
    assert.equal(res.status, 0, res.stderr);
    assert.deepEqual(page.calls, [
      { kind: 'serve.publish', options: { port, entry: 'app/main.html', allowLive: true } },
    ]);
    assert.match(res.stderr, new RegExp(`serve: 127\\.0\\.0\\.1:${port} is public at `));
    const ambiguous = await run(['serve', '--stop', String(port)]);
    assert.equal(ambiguous.status, 1);
    assert.equal(ambiguous.stderr, `serve: ${port} matches 2 previews; stop one by its token\n`);
    const published = (await records()).find((r) => r.previewToken);
    assert.equal((await run(['serve', '--stop', published.previewToken])).status, 0);
    await gone(published.pid);
    const none = await run(['serve', '--port', '1']);
    assert.match(none.stderr, /^serve: nothing answers on 127\.0\.0\.1:1 yet\n/);
    const stop = await run(['serve', '--stop', local.stdout.trim()]);
    assert.equal(stop.status, 0, stop.stderr);
    assert.equal(stop.stdout, `stopped ${local.stdout.trim()}\n`);
    await refused(port);
  });

  it('stays attached with --foreground, logs visits, and stops when the page ends it', async () => {
    const running = run(['serve', '--foreground', '/w/site']);
    await eventually(() => page.previews.size === 1, 20_000);
    const [preview] = page.previews.values();
    await eventually(async () => (await records()).length === 1);
    const [record] = await records();
    assert.equal(record.foreground, true);
    assert.equal((await os.fetch(record.port, '/index.html')).status, 200);
    const table = await run(['serve', '--list']);
    assert.match(table.stdout, /live \(foreground\)/);
    preview.end('stopped in seven');
    const res = await running;
    assert.equal(res.status, 0);
    assert.equal(res.stdout, `${record.url}\n`);
    assert.match(res.stderr, /^serve: \/w\/site is public at .*; Ctrl-C stops it\n/);
    assert.match(res.stderr, /\nserve: GET \/index\.html 200 11\n/);
    assert.match(res.stderr, /serve: stopped: stopped in seven\n$/);
    assert.deepEqual(await records(), []);

    const again = run(['serve', '--foreground', '--port', String(record.port)]);
    await eventually(() => page.previews.size === 1, 20_000);
    const [held] = page.previews.values();
    await eventually(async () => (await records()).length === 1);
    const [attached] = await records();
    await os.sh(`kill -INT ${attached.pid}`);
    const interrupted = await again;
    assert.notEqual(interrupted.status, 0);
    assert.doesNotMatch(interrupted.stderr, /stopped:/);
    await eventually(() => held.ended?.startsWith('revoked'), 20_000);
  });

  it('keeps a preview published after the terminal that started it hangs up', async () => {
    let output = '';
    const term = await os.kernel.openTerminal(['bash', '-i'], {
      cwd: '/w',
      onData: (bytes) => {
        output += Buffer.from(bytes).toString();
      },
    });
    term.write('(serve site; echo "done $?"; sleep 60)\r');
    await eventually(() => output.includes('done 0'), 20_000);
    const [record] = await records();
    const [preview] = page.previews.values();
    assert.equal(record.previewToken, preview.token);
    term.close();
    await term.exited;
    await new Promise((r) => setTimeout(r, 3000));
    assert.equal(preview.ended, undefined);
    assert.equal((await os.sh(`test -e /proc/${record.pid}`)).status, 0);
    assert.equal(await (await os.fetch(record.port, '/')).text(), '<h1>hi</h1>');
    assert.deepEqual(
      (await records()).map((r) => r.id),
      [record.id]
    );
  });

  it('opens the URL with open.url for --open', async () => {
    const res = await run(['serve', '--open', '/w/site']);
    assert.equal(res.status, 0, res.stderr);
    assert.deepEqual(page.calls.at(-1), { kind: 'open.url', options: { url: res.stdout.trim() } });
  });

  it("prints the page's refusals with the kernel's errno, and leaves nothing running", async () => {
    for (const [name, message, extra] of [
      ['NotAllowedError', 'the request to publish was dismissed', hint],
      ['NotSupportedError', "seven isn't connected to the tray hub", ''],
      ['TypeError', 'the tray is not connected\nsecond line', ''],
    ]) {
      page.fail = refusal(name, message);
      assert.deepEqual(await run(['serve', '/w/site']), {
        status: 1,
        stdout: '',
        stderr: `serve: ${message.split('\n')[0]}\n${extra}`,
      });
    }
    page.fail = refusal('NotAllowedError', 'public previews are off');
    const json = await run(['serve', '/w/site', '--json']);
    assert.equal(json.status, 1);
    assert.deepEqual(JSON.parse(json.stdout), {
      ok: false,
      exit: 1,
      lines: ['serve: public previews are off', hint.trim()],
      signIn: null,
    });
    await eventually(
      async () => !(await os.sh('cat /proc/[0-9]*/cmdline')).stdout.includes('__hold'),
      20_000
    );
    assert.deepEqual(await records(), []);
  });

  it("maps the tray hub's failures to exits 1, 3 and 4", async () => {
    const cases = [
      [
        {
          error: 'too many',
          status: 429,
          code: 'LIVE_PREVIEW_LIMIT',
          retryAfter: '30',
          active: 200,
          limit: 200,
        },
        4,
        "serve: publishing: the tray hub's limit is reached (429: too many (200 of 200 live previews in use); list them with `serve --list` and stop the ones you no longer need with `serve --stop <token>`)\nserve: retry after 30 s\n",
      ],
      [
        {
          error: 'sign in first',
          status: 403,
          code: 'TRAY_REVOKED',
          signIn: 'https://auth.example/login',
        },
        3,
        'serve: publishing: the tray hub refused it (403: sign in first (TRAY_REVOKED))\nserve: sign in at https://auth.example/login\n',
      ],
      [
        { error: 'gated', status: 302, signIn: 'https://auth.example/' },
        3,
        'serve: the tray hub wants you to sign in: https://auth.example/\n',
      ],
      [
        { error: 'invalid preview entry path', status: 400 },
        1,
        'serve: publishing: the tray hub answered 400: invalid preview entry path\n',
      ],
    ];
    for (const [answer, status, stderr] of cases) {
      page.answer = `${JSON.stringify(answer)}\n`;
      assert.deepEqual(await run(['serve', '/w/site']), { status, stdout: '', stderr });
    }
    page.answer = '';
    assert.deepEqual(await run(['serve', '/w/site']), {
      status: 1,
      stdout: '',
      stderr: 'serve: seven ended the request without a URL\n',
    });
    page.answer = '{"url":"https://x.example/"}\n';
    assert.equal(
      (await run(['serve', '/w/site'])).stderr,
      "serve: seven's answer to serve.publish has no url or previewToken\n"
    );
    assert.deepEqual(await records(), []);
  });

  it('passes a hostile root to the page as JSON, and refuses --ttl, missing directories and entries', async () => {
    const name = 'h"q\\$(touch pwned)\té😀';
    await os.kernel.writeFile(`/w/${name}/index.html`, 'x');
    const res = await run(['serve', name], { cwd: '/w' });
    assert.equal(res.status, 0, res.stderr);
    assert.equal(page.calls[0].options.root, `/w/${name}`);
    assert.equal((await os.sh('test -e /w/pwned || test -e pwned')).status, 1);
    assert.equal((await run(['serve', '/w/nope'])).stderr, 'serve: no such directory: /w/nope\n');
    assert.equal(
      (await run(['serve', '--entry', 'x.html', '/w/site'])).stderr,
      'serve: entry file not found: /w/site/x.html\n'
    );
    assert.equal(page.calls.length, 1);
  });

  it('stops a token the page holds without a record, and says when nothing matches', async () => {
    const res = await run(['serve', '/w/site']);
    assert.equal(res.status, 0, res.stderr);
    const [record] = await records();
    await os.sh(`rm ${STATE}/previews/${record.id}.json`);
    const stop = await run(['serve', '--stop', record.previewToken]);
    assert.equal(stop.status, 0, stop.stderr);
    await gone(record.pid);
    assert.deepEqual(await run(['serve', '--stop', record.previewToken]), {
      status: 1,
      stdout: '',
      stderr: 'serve: the tray hub has no such preview\n',
    });
    assert.deepEqual(await run(['serve', '--stop', 'nothing']), {
      status: 1,
      stdout: '',
      stderr: 'serve: no preview matches nothing\n',
    });
  });
});

describe('what 1.0.0 got wrong in seven', { skip }, () => {
  beforeEach(stopAll);

  it('lists nothing when nothing is published (#18)', async () => {
    assert.deepEqual(await run(['serve', '--list']), {
      status: 0,
      stdout: 'no previews\n',
      stderr: '',
    });
    assert.deepEqual(await run(['serve', '--list', '--json']), {
      status: 0,
      stdout: '[]\n',
      stderr: '',
    });
  });

  it('prints JSON for --local DIR and --local --port with --json (#19)', async () => {
    const dir = await run(['serve', '--local', '--json', '/w/site']);
    assert.equal(dir.status, 0, dir.stderr);
    assert.equal(dir.stderr, '');
    const out = JSON.parse(dir.stdout);
    assert.equal(out.mode, 'local');
    assert.equal(out.root, '/w/site');
    assert.match(out.localUrl, /^http:\/\/\d+\.kernel\.localhost\/$/);
    assert.equal(out.port, Number(new URL(out.localUrl).hostname.split('.')[0]));
    const port = await run(['serve', '--port', String(out.port), '--local', '--json']);
    assert.equal(port.status, 0, port.stderr);
    const shown = JSON.parse(port.stdout);
    assert.match(shown.createdAt, /^\d{4}-\d\d-\d\dT/);
    assert.deepEqual(
      { ...shown, createdAt: 'x' },
      { mode: 'local', localUrl: out.localUrl, port: out.port, createdAt: 'x', foreground: false }
    );
  });

  it('returns from --local DIR and keeps serving in the background (#20)', async () => {
    const res = await run(['serve', '--local', '/w/site']);
    assert.equal(res.status, 0, res.stderr);
    assert.doesNotMatch(res.stderr, /in the foreground/);
    const port = Number(new URL(res.stdout.trim()).hostname.split('.')[0]);
    assert.equal(await (await os.fetch(port, '/')).text(), '<h1>hi</h1>');
    const [record] = await records();
    assert.equal(record.mode, 'local');
    assert.equal((await run(['serve', '--stop', res.stdout.trim()])).status, 0);
    await gone(record.pid);
    await refused(port);
  });
});

describe('serve without a page', { skip }, () => {
  it('gets ENOTSUP from a session no page answers, and still serves --local', async () => {
    const bare = await boot();
    try {
      await bare.sh('mkdir -p /w/site && printf x > /w/site/index.html');
      assert.deepEqual(await bare.run(['serve', '/w/site']), {
        status: 1,
        stdout: '',
        stderr: 'serve: no page answers requests for this session\n',
      });
      const local = await bare.run(['serve', '--local', '/w/site']);
      assert.equal(local.status, 0, local.stderr);
      const list = await bare.run(['serve', '--list', '--json']);
      assert.equal(list.status, 0, list.stderr);
      assert.deepEqual(
        JSON.parse(list.stdout).map((r) => [r.mode, r.url]),
        [['local', local.stdout.trim()]]
      );
      const port = Number(new URL(local.stdout.trim()).hostname.split('.')[0]);
      assert.equal(await (await bare.fetch(port)).text(), 'x');
      assert.equal((await bare.run(['serve', '--stop', String(port)])).status, 0);
    } finally {
      bare.close();
    }
  });

  it('is refused by a kernel older than 1.153.0, and without /dev/embedder says what it needs', async () => {
    const old = await bootOld(true);
    try {
      const refusedRun = await old.run(['serve', '--version']);
      assert.notEqual(refusedRun.status, 0);
      assert.match(refusedRun.stderr, /needs slicc-kernel >=1\.153\.0/);
    } finally {
      old.close();
    }
    const bare = await bootOld(false);
    try {
      await bare.sh('mkdir -p /w/site');
      for (const argv of [
        ['serve', '/w/site'],
        ['scserve', '--stop', '00000000-0000-4000-a000-000000000000.abcdef'],
      ]) {
        const res = await bare.run(argv);
        assert.equal(res.status, 1);
        assert.match(
          res.stderr,
          /^(scserve|serve): \/dev\/embedder not available \(needs slicc-kernel >= 1\.153\.0\)\n$/
        );
      }
      assert.equal((await bare.run(['serve', '--version'])).status, 0);
      assert.equal((await bare.run(['serve', '--list'])).stdout, 'no previews\n');
    } finally {
      bare.close();
    }
  });
});
