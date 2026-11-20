import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { pathToFileURL } from 'node:url';
import { root, scratch } from './helpers.mjs';

test('AGENTS.md stays within 1000 characters and there is no CLAUDE.md', () => {
  assert.ok([...readFileSync(join(root, 'AGENTS.md'), 'utf8')].length <= 1000);
  assert.ok(!existsSync(join(root, 'CLAUDE.md')));
});

test('npm-stage builds the main package with slicc.commands and the platform packages', () => {
  const from = scratch();
  const out = scratch();
  const targets = spawnSync(
    process.execPath,
    [
      '--input-type=module',
      '-e',
      `import { TARGETS, assetName } from ${JSON.stringify(pathToFileURL(join(root, 'scripts', 'targets.mjs')).href)}; console.log(JSON.stringify(TARGETS.map(assetName)))`,
    ],
    { encoding: 'utf8' }
  );
  for (const name of [...JSON.parse(targets.stdout), 'serve.wasm'])
    writeFileSync(join(from, name), name);
  const res = spawnSync(
    process.execPath,
    [join(root, 'scripts', 'npm-stage.mjs'), '--version', '1.2.3', '--from', from, '--out', out],
    { encoding: 'utf8' }
  );
  assert.equal(res.status, 0, res.stderr);
  const main = JSON.parse(readFileSync(join(out, 'slicc-serve', 'package.json'), 'utf8'));
  assert.equal(main.name, '@ai-ecoverse/slicc-serve');
  assert.equal(main.version, '1.2.3');
  assert.deepEqual(main.bin, { scserve: 'bin/scserve.js' });
  assert.deepEqual(main.engines, { node: '>=18', 'slicc-kernel': '>=1.153.0' });
  assert.deepEqual(main.slicc, {
    abi: 'wasi',
    commands: { serve: { wasm: 'bin/serve.wasm' }, scserve: { wasm: 'bin/serve.wasm' } },
  });
  assert.equal(Object.keys(main.optionalDependencies).length, 6);
  const win = JSON.parse(readFileSync(join(out, 'slicc-serve-win32-x64', 'package.json'), 'utf8'));
  assert.deepEqual(win.files, ['bin/scserve.exe']);
  assert.deepEqual(win.os, ['win32']);
  assert.ok(existsSync(join(out, 'slicc-serve', 'bin', 'serve.wasm')));
});

test('the release config and the workflow stay on the shared pipeline', () => {
  const ci = readFileSync(join(root, '.github', 'workflows', 'ci.yml'), 'utf8');
  const pkg = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));
  const shared = pkg.devDependencies['@ai-ecoverse/slicc-shared-web'];
  assert.ok(ci.includes(`ai-ecoverse/slicc-shared-web/.github/workflows/pipeline.yml@v${shared}`));
  const release = JSON.parse(readFileSync(join(root, '.releaserc.json'), 'utf8'));
  assert.deepEqual(release.branches, ['main']);
});
