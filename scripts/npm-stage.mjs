#!/usr/bin/env node
import { chmodSync, copyFileSync, existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { assetName, NAME, TARGETS } from './targets.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const description =
  'serve and scserve: publish a directory or a local server at a public capability URL through the SLICC tray hub, on any host and inside SLICC';
const common = {
  license: 'Apache-2.0',
  repository: { type: 'git', url: 'git+https://github.com/ai-ecoverse/slicc-serve.git' },
  homepage: 'https://github.com/ai-ecoverse/slicc-serve#readme',
};

const { values } = parseArgs({
  options: {
    version: { type: 'string' },
    from: { type: 'string', default: join(root, 'artifacts', 'release') },
    out: { type: 'string', default: join(root, 'dist', 'npm') },
    only: { type: 'string', multiple: true },
  },
});

if (!values.version) {
  process.stderr.write(
    'usage: npm-stage.mjs --version <semver> [--from dir] [--out dir] [--only os-arch]...\n'
  );
  process.exit(2);
}
const version = values.version.replace(/^v/, '');
const selected = values.only?.length ? TARGETS.filter((t) => values.only.includes(t.id)) : TARGETS;

function write(dir, file, content) {
  mkdirSync(dir, { recursive: true });
  writeFileSync(
    join(dir, file),
    typeof content === 'string' ? content : `${JSON.stringify(content, null, 2)}\n`
  );
}

function need(file) {
  if (!existsSync(file)) throw new Error(`missing ${file}`);
  return file;
}

function stagePlatform(target) {
  const name = `${NAME}-${target.id}`;
  const dir = join(values.out, `slicc-serve-${target.id}`);
  const file = target.os === 'win32' ? 'scserve.exe' : 'scserve';
  mkdirSync(join(dir, 'bin'), { recursive: true });
  copyFileSync(need(join(values.from, assetName(target))), join(dir, 'bin', file));
  chmodSync(join(dir, 'bin', file), 0o755);
  copyFileSync(join(root, 'LICENSE'), join(dir, 'LICENSE'));
  write(dir, 'package.json', {
    name,
    version,
    description: `The scserve binary for ${target.label}`,
    ...common,
    os: [target.os],
    cpu: [target.cpu],
    files: [`bin/${file}`],
    preferUnplugged: true,
  });
  write(
    dir,
    'README.md',
    `# ${name}\n\nThe \`scserve\` binary for ${target.label}. Install [${NAME}](https://www.npmjs.com/package/${NAME}) instead of this package.\n`
  );
  return name;
}

function stageMain(platforms) {
  const dir = join(values.out, 'slicc-serve');
  mkdirSync(join(dir, 'bin'), { recursive: true });
  copyFileSync(join(root, 'npm', 'scserve.js'), join(dir, 'bin', 'scserve.js'));
  chmodSync(join(dir, 'bin', 'scserve.js'), 0o755);
  copyFileSync(need(join(values.from, 'serve.wasm')), join(dir, 'bin', 'serve.wasm'));
  copyFileSync(join(root, 'LICENSE'), join(dir, 'LICENSE'));
  copyFileSync(join(root, 'README.md'), join(dir, 'README.md'));
  write(dir, 'package.json', {
    name: NAME,
    version,
    description,
    ...common,
    bin: { scserve: 'bin/scserve.js' },
    files: ['bin/scserve.js', 'bin/serve.wasm'],
    engines: { node: '>=18' },
    slicc: {
      abi: 'wasi',
      commands: {
        serve: { wasm: 'bin/serve.wasm' },
        scserve: { wasm: 'bin/serve.wasm' },
      },
    },
    optionalDependencies: Object.fromEntries(platforms.map((name) => [name, version])),
  });
}

rmSync(values.out, { recursive: true, force: true });
stageMain(selected.map(stagePlatform));
process.stdout.write(
  `staged ${NAME}@${version} and ${selected.length} platform packages in ${values.out}\n`
);
