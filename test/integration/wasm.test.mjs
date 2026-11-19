import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { WASI } from 'node:wasi';
import { expectedVersion, wasm } from './helpers.mjs';

const skip = !existsSync(wasm) && 'run npm run build:wasm first';

function runWasm(args, name = 'serve', env = {}) {
  const dir = mkdtempSync(join(tmpdir(), 'scserve-wasm-'));
  const fs = process.getBuiltinModule('node:fs');
  const out = fs.openSync(join(dir, 'out'), 'w');
  const err = fs.openSync(join(dir, 'err'), 'w');
  const wasi = new WASI({
    version: 'preview1',
    args: [name, ...args],
    env,
    stdout: out,
    stderr: err,
    returnOnExit: true,
  });
  const module = new WebAssembly.Module(readFileSync(wasm));
  const imports = wasi.getImportObject();
  for (const { module: from, name: fn } of WebAssembly.Module.imports(module)) {
    if (from === 'wasi_snapshot_preview1') continue;
    imports[from] ??= {};
    imports[from][fn] = () => 52;
  }
  const code = wasi.start(new WebAssembly.Instance(module, imports));
  fs.closeSync(out);
  fs.closeSync(err);
  const result = {
    code,
    stdout: readFileSync(join(dir, 'out'), 'utf8'),
    stderr: readFileSync(join(dir, 'err'), 'utf8'),
  };
  rmSync(dir, { recursive: true, force: true });
  return result;
}

test('the kernel build prints its version', { skip }, () => {
  const res = runWasm(['--version']);
  assert.equal(res.code, 0);
  assert.equal(res.stdout, `${expectedVersion}\n`);
});

test('the kernel build names itself after the command it runs as', { skip }, () => {
  assert.match(runWasm(['--help']).stdout, /^usage: serve \[--entry FILE\]/);
  assert.match(runWasm(['--help'], 'scserve').stdout, /^usage: scserve \[--entry FILE\]/);
});

test('the kernel build keeps the exit codes', { skip }, () => {
  const usage = runWasm(['--nope']);
  assert.equal(usage.code, 2);
  assert.match(usage.stderr, /^serve: unknown option: --nope\nusage: serve /);
  const publish = runWasm(['site']);
  assert.equal(publish.code, 1);
  assert.match(publish.stderr, /^serve: publishing from SLICC needs seven's serve\.publish/);
});

test('--upgrade in the kernel names the pnpm command', { skip }, () => {
  const res = runWasm(['--upgrade']);
  assert.equal(res.code, 0);
  assert.equal(res.stdout, 'pnpm add -g @ai-ecoverse/slicc-serve@latest\n');
});

test('--local --port in the kernel prints the page loopback URL', { skip }, () => {
  const res = runWasm(['--local', '--port', '5173']);
  assert.equal(res.code, 0);
  assert.equal(res.stdout, 'http://127.0.0.1:5173/\n');
});

test('--local --port in the kernel uses SLICC_PAGE_LOOPBACK_URL', { skip }, () => {
  const res = runWasm(['--local', '--port', '5173', '--entry', 'app.html'], 'serve', {
    SLICC_PAGE_LOOPBACK_URL: 'http://{port}.kernel.localhost',
  });
  assert.equal(res.stdout, 'http://5173.kernel.localhost/app.html\n');
});
