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
    [['--bridge', '--no-bridge', site], 'give --bridge or --no-bridge, not both'],
    [['--quiet', site], '--quiet goes with --bridge'],
    [['--max-tabs', '3', site], '--max-tabs goes with --bridge'],
    [['--bridge', '--max-tabs', '0', site], '--max-tabs must be a number from 1 to 100: 0'],
    [['--bridge', '--max-tabs', '101', site], '--max-tabs must be a number from 1 to 100: 101'],
    [['--bridge', '--local', site], "--bridge publishes; it can't be --local"],
    [
      ['--bridge', '--ttl', '1h', site],
      '--ttl snapshots have no visitor sockets, so there is no --bridge',
    ],
    [['--lines', '3'], '--lines goes with --logs'],
    [['--logs', '--lines', '0'], '--lines must be a positive number: 0'],
    [['--logs', 't', '--open'], '--logs takes only a token or URL, --lines and --json'],
    [['--truncate', 't', '--lines', '2'], '--lines goes with --logs'],
    [['--truncate', '--open'], '--truncate takes only a token or URL and --json'],
    [['--logs='], '--logs needs a token or URL after ='],
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
  box.site({ '.env': 'SECRET=1' }, 'site');
  res = await box.run(['--entry', '.env', site]);
  assert.equal(res.code, 1);
  assert.match(res.stderr, /^scserve: entry file not found: .*\.env/);
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

test('--entry refuses a symlink out of the directory', {
  skip: process.platform === 'win32' && 'symlinks need privileges on Windows',
}, async () => {
  const box = sandbox();
  const outside = box.site({ 'secret.html': 's' }, 'outside');
  const site = box.site();
  const { symlinkSync } = await import('node:fs');
  const { join } = await import('node:path');
  symlinkSync(join(outside, 'secret.html'), join(site, 'out.html'));
  const res = await box.run(['--local', '--entry', 'out.html', site]);
  assert.equal(res.code, 1);
  assert.match(res.stderr, /^scserve: entry file not found: .*out\.html/);
});

test('the harness survives a scserve that exits before reading its stdin', async () => {
  const box = sandbox();
  for (let i = 0; i < 3; i++) {
    const res = await box.run(['--version'], {}, { input: Buffer.alloc(4 * 1024 * 1024, 120) });
    assert.equal(res.code, 0);
    assert.equal(res.stdout, `${expectedVersion}\n`);
  }
  const quick = await Promise.all(Array.from({ length: 20 }, () => box.run(['--list'])));
  assert.ok(quick.every((r) => r.code === 0 && r.stdout === 'no previews\n'));
});
