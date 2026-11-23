import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { root } from './helpers.mjs';

export const SLICC_CDP = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'))
  .devDependencies['@ai-ecoverse/slicc-cdp'];
export const CHROME = '158.0.8127.36';
const RELEASE = `https://github.com/ai-ecoverse/slicc-cdp/releases/download/v${SLICC_CDP}`;
const platform = { darwin: 'darwin', linux: 'linux', win32: 'windows' }[process.platform];
const arch = { x64: 'amd64', arm64: 'arm64' }[process.arch];
const exe = process.platform === 'win32' ? '.exe' : '';
const cache = join(root, 'node_modules', '.cache');

export async function tool(name) {
  const asset = `${name}-${platform}-${arch}${exe}`;
  const dir = join(cache, `slicc-cdp-${SLICC_CDP}`);
  const file = join(dir, asset);
  if (existsSync(file)) return file;
  mkdirSync(dir, { recursive: true });
  const sums = await (await fetch(`${RELEASE}/SHA256SUMS`)).text();
  const want = sums
    .split('\n')
    .map((l) => l.trim().split(/\s+/))
    .find(([, n]) => n === asset)?.[0];
  assert.ok(want, `${asset} is not in slicc-cdp ${SLICC_CDP}'s SHA256SUMS`);
  const res = await fetch(`${RELEASE}/${asset}`);
  assert.equal(res.status, 200, `downloading ${asset}`);
  const bytes = Buffer.from(await res.arrayBuffer());
  assert.equal(createHash('sha256').update(bytes).digest('hex'), want, `${asset} checksum`);
  writeFileSync(`${file}.part`, bytes);
  chmodSync(`${file}.part`, 0o755);
  renameSync(`${file}.part`, file);
  return file;
}

export function run(bin, args, cwd) {
  return new Promise((resolve, reject) => {
    const child = spawn(bin, args, {
      cwd,
      env: {
        PATH: process.env.PATH,
        HOME: cwd,
        USERPROFILE: cwd,
        XDG_CONFIG_HOME: join(cwd, 'config'),
        APPDATA: join(cwd, 'config'),
        LOCALAPPDATA: join(cwd, 'config'),
        SystemRoot: process.env.SystemRoot ?? '',
      },
    });
    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (d) => {
      stdout += d;
    });
    child.stderr.on('data', (d) => {
      stderr += d;
    });
    const timer = setTimeout(() => child.kill(), 60_000);
    child.on('error', reject);
    child.on('close', (code) => {
      clearTimeout(timer);
      resolve({ code, stdout, stderr });
    });
  });
}
