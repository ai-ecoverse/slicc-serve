import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { join } from 'node:path';
import { after, test } from 'node:test';
import { pathToFileURL } from 'node:url';
import { binary, expectedVersion, root, scratch } from './helpers.mjs';

const unix = process.platform !== 'win32';
const os = process.platform === 'darwin' ? 'darwin' : 'linux';
const arch = process.arch === 'arm64' ? 'arm64' : 'amd64';
const asset = `scserve-${os}-${arch}`;

function sh(args, env = {}) {
  return new Promise((resolve) => {
    const child = spawn('sh', [join(root, 'install.sh'), ...args], {
      env: { PATH: process.env.PATH, HOME: env.HOME ?? '/nonexistent', ...env },
    });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (c) => (stdout += c));
    child.stderr.on('data', (c) => (stderr += c));
    child.on('close', (code) => resolve({ code, stdout, stderr }));
  });
}

function release({ corrupt = false } = {}) {
  const dir = scratch();
  copyFileSync(binary, join(dir, asset));
  const sum = createHash('sha256').update(readFileSync(binary)).digest('hex');
  writeFileSync(join(dir, 'SHA256SUMS'), `${corrupt ? '0'.repeat(64) : sum}  ${asset}\n`);
  copyFileSync(join(root, 'install.sh'), join(dir, 'install.sh'));
  return dir;
}

test('install.sh installs the binary from SHA256SUMS and reports its version', {
  skip: !unix && 'POSIX only',
}, async () => {
  const assets = release();
  const dir = join(scratch(), 'bin');
  const res = await sh(['--dir', dir], { SLICC_INSTALL_BASE_URL: pathToFileURL(assets).href });
  assert.equal(res.code, 0, res.stderr);
  assert.match(
    res.stdout,
    new RegExp(`^scserve ${expectedVersion} installed to ${join(dir, 'scserve')}\\n`)
  );
  assert.match(res.stdout, /Add .* to your PATH:\n {2}export PATH=".*:\$PATH"\n$/);
  assert.ok(existsSync(join(dir, 'scserve')));
});

test('install.sh pins --version and refuses a release that reports another', {
  skip: !unix && 'POSIX only',
}, async () => {
  const assets = release();
  const dir = join(scratch(), 'bin');
  const base = pathToFileURL(assets).href;
  const ok = await sh([`--version=v${expectedVersion}`, `--dir=${dir}`], {
    SLICC_INSTALL_BASE_URL: base,
  });
  assert.equal(ok.code, 0, ok.stderr);
  const bad = await sh(['--version', '9.9.9', '--dir', dir], { SLICC_INSTALL_BASE_URL: base });
  assert.equal(bad.code, 1);
  assert.equal(bad.stderr, `install.sh: ${asset} reports ${expectedVersion}, not 9.9.9\n`);
});

test('install.sh refuses a checksum mismatch and installs nothing', {
  skip: !unix && 'POSIX only',
}, async () => {
  const assets = release({ corrupt: true });
  const dir = join(scratch(), 'bin');
  const res = await sh(['--dir', dir], { SLICC_INSTALL_BASE_URL: pathToFileURL(assets).href });
  assert.equal(res.code, 1);
  assert.match(
    res.stderr,
    new RegExp(
      `^install\\.sh: checksum mismatch for ${asset}: expected 0{64}, got [0-9a-f]{64}\\n$`
    )
  );
  assert.ok(!existsSync(join(dir, 'scserve')));
});

test('install.sh: options, usage errors and HOME', { skip: !unix && 'POSIX only' }, async () => {
  const help = await sh(['--help']);
  assert.equal(help.code, 0);
  assert.match(help.stdout, /^usage: install\.sh \[--version VERSION\] \[--dir DIR\] \[--help\]/);
  const unknown = await sh(['--nope']);
  assert.equal(unknown.code, 2);
  assert.match(unknown.stderr, /^install\.sh: unknown option: --nope\nusage: install\.sh/);
  const missing = await sh(['--dir']);
  assert.equal(missing.code, 2);
  assert.match(missing.stderr, /^install\.sh: --dir needs a value/);
  const home = await sh([], { HOME: '' });
  assert.equal(home.code, 1);
  assert.equal(home.stderr, 'install.sh: HOME is not set; give --dir\n');
  const nothing = await sh(['--dir', scratch()], {
    SLICC_INSTALL_BASE_URL: pathToFileURL(join(scratch(), 'none')).href,
  });
  assert.equal(nothing.code, 1);
  assert.match(nothing.stderr, /^install\.sh: could not download .*\/SHA256SUMS\n$/);
});

test('--upgrade runs the latest install.sh into its own directory', {
  skip: !unix && 'POSIX only',
}, async () => {
  const assets = release();
  const server = createServer((req, res) => {
    const file = join(assets, req.url.slice(1));
    if (!existsSync(file)) {
      res.writeHead(404);
      return res.end();
    }
    res.end(readFileSync(file));
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  after(() => server.close());
  const base = `http://127.0.0.1:${server.address().port}`;
  const dir = join(scratch(), 'bin');
  mkdirSync(dir);
  copyFileSync(binary, join(dir, 'scserve'));
  const run = (env) =>
    new Promise((resolve) => {
      const child = spawn(join(dir, 'scserve'), ['--upgrade'], {
        env: { PATH: process.env.PATH, HOME: scratch(), ...env },
      });
      let stdout = '';
      let stderr = '';
      child.stdout.on('data', (c) => (stdout += c));
      child.stderr.on('data', (c) => (stderr += c));
      child.on('close', (code) => resolve({ code, stdout, stderr }));
    });
  const res = await run({ SLICC_INSTALL_BASE_URL: base });
  assert.equal(res.code, 0, res.stderr);
  assert.equal(res.stdout, `scserve: already up to date (${expectedVersion})\n`);
  const missing = await run({ SLICC_INSTALL_BASE_URL: `${base}/nope` });
  assert.equal(missing.code, 1);
  assert.equal(missing.stderr, `scserve: could not download ${base}/nope/install.sh: HTTP 404\n`);
});
