import assert from 'node:assert/strict';
import { test } from 'node:test';
import { expectedVersion, sandbox } from './helpers.mjs';

test('--version prints the version alone', async () => {
  const res = await sandbox().run(['--version']);
  assert.equal(res.code, 0);
  assert.equal(res.stdout, `${expectedVersion}\n`);
});

test('--help prints the usage and the options to stdout', async () => {
  const box = sandbox();
  for (const args of [['--help'], ['-h'], ['--nope', '-h']]) {
    const res = await box.run(args);
    assert.equal(res.code, 0);
    assert.match(res.stdout, /^usage: scserve /);
    assert.match(res.stdout, /--foreground/);
    assert.match(res.stdout, /Exit status: 0 ok, 1 failure, 2 usage error, 3 /);
  }
});

test('no arguments print the usage to stderr and exit 2', async () => {
  const res = await sandbox().run([]);
  assert.equal(res.code, 2);
  assert.equal(res.stdout, '');
  assert.match(res.stderr, /^usage: scserve /);
});

test('usage errors exit 2 with a reason and the usage', async () => {
  const box = sandbox();
  const site = box.site();
  const cases = [
    [['--nope'], 'unknown option: --nope'],
    [[site, '--port', '3000'], 'give a directory or --port, not both'],
    [['--json'], 'give a directory or --port'],
    [['a', 'b'], 'give one directory'],
    [['--port', '0'], '--port must be a port from 1 to 65535: 0'],
    [['--port', 'x'], '--port must be a port from 1 to 65535: x'],
    [['--port'], '--port needs a port'],
    [['--entry', '../x', site], 'invalid entry: ../x'],
    [['--entry', '/etc/passwd', site], 'invalid entry: /etc/passwd'],
    [['--ttl', '0d', site], '--ttl must be a positive whole duration (m, h, d or w)'],
    [['--ttl', '5s', site], '--ttl must be a positive whole duration (m, h, d or w)'],
    [['--ttl', '31d', site], "--ttl can't exceed 30d"],
    [['--ttl', '5w', site], "--ttl can't exceed 30d"],
    [['--ttl', '1h', '--local', site], "--ttl publishes; it can't be --local"],
    [
      ['--ttl', '1h', '--foreground', site],
      '--ttl leaves nothing running, so there is no --foreground',
    ],
    [['--ttl', '1h', '--port', '80'], "--ttl snapshots a directory; it can't snapshot a server"],
    [['--project', site], '--project is obsolete: give the directory to serve itself'],
    [['--bridge', site], '--bridge is not supported yet'],
    [['--no-bridge', site], '--no-bridge is not supported yet'],
    [['--quiet', site], '--quiet is not supported yet'],
    [['--logs'], '--logs is not supported yet'],
    [['--max-tabs', '3', site], '--max-tabs is not supported yet'],
    [['--list', site], '--list takes only --json'],
    [['--stop', 'x', '--open'], '--stop takes only --json'],
    [['--version', '--json'], '--version takes no other arguments'],
    [['--upgrade', site], '--upgrade takes no other arguments'],
    [['--local=yes', site], '--local takes no value'],
    [['--entry=', site], '--entry needs a file'],
  ];
  for (const [args, message] of cases) {
    const res = await box.run(args);
    assert.equal(res.code, 2, `${args.join(' ')}: ${res.stderr}`);
    assert.equal(res.stdout, '');
    assert.ok(
      res.stderr.startsWith(`scserve: ${message}\nusage: scserve `),
      `${args.join(' ')}: ${res.stderr}`
    );
  }
});

test('a missing directory or entry file exits 1', async () => {
  const box = sandbox();
  const site = box.site();
  let res = await box.run([`${site}-missing`]);
  assert.equal(res.code, 1);
  assert.match(res.stderr, /^scserve: no such directory: /);
  res = await box.run([`${site}/index.html`]);
  assert.equal(res.code, 1);
  assert.match(res.stderr, /^scserve: not a directory: /);
  res = await box.run(['--entry', 'nope.html', site]);
  assert.equal(res.code, 1);
  assert.match(res.stderr, /^scserve: entry file not found: .*nope\.html/);
});

test('--upgrade from the npm launcher names the npm command', async () => {
  const res = await sandbox().run(['--upgrade'], { SCSERVE_LAUNCHER: 'npm' });
  assert.equal(res.code, 0);
  assert.equal(res.stdout, 'npm install -g @ai-ecoverse/slicc-serve@latest\n');
});

test('an invalid tray hub URL exits 1', async () => {
  const box = sandbox();
  const res = await box.run([box.site()], { SLICC_TRAY_HUB: 'ftp://nope' });
  assert.equal(res.code, 1);
  assert.match(res.stderr, /^scserve: invalid tray hub URL ftp:\/\/nope/);
});
