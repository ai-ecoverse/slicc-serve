import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { attachKernel, createNodeKernel } from '@ai-ecoverse/slicc-kernel/node';
import { createNodeKernel as createOldKernel } from 'slicc-kernel-without-embedder/node';

const root = resolve(import.meta.dirname, '../..');
const { mainManifest } = await import(pathToFileURL(join(root, 'scripts', 'targets.mjs')).href);

function walk(dir, out = []) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) walk(path, out);
    else out.push(path);
  }
  return out;
}

async function install(kernel, name, dir) {
  for (const path of walk(dir)) {
    const rel = relative(dir, path).replaceAll('\\', '/');
    await kernel.writeFile(`/node_modules/${name}/${rel}`, readFileSync(path));
  }
}

export async function boot(options = {}, { create = createNodeKernel, engines = true } = {}) {
  const kernel = await create(options);
  for (const name of ['@ai-ecoverse/wasm-bash', '@ai-ecoverse/wasm-coreutils']) {
    await install(kernel, name, join(root, 'node_modules', name));
  }
  const manifest = mainManifest('0.0.0', []);
  if (!engines) delete manifest.engines['slicc-kernel'];
  await kernel.writeFile(
    `/node_modules/${manifest.name}/package.json`,
    JSON.stringify(manifest, null, 2)
  );
  await kernel.writeFile(
    `/node_modules/${manifest.name}/bin/serve.wasm`,
    readFileSync(join(root, 'bin', 'serve.wasm'))
  );
  let client;
  const os = {
    kernel,
    sh: (script, opts = {}) => kernel.run(['sh', '-c', script], { cwd: '/tmp', ...opts }),
    run: (argv, opts = {}) => kernel.run(argv, { cwd: '/tmp', ...opts }),
    async fetch(port, path = '/', init = {}) {
      client ??= await attachKernel(await kernel.connect());
      return client.loopbackFetch(`http://127.0.0.1:${port}${path}`, { port, ...init });
    },
    close: () => kernel.terminate(),
  };
  return os;
}

export const bootOld = (engines) => boot({}, { create: createOldKernel, engines });
