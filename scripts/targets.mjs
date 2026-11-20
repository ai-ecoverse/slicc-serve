export const TARGETS = [
  {
    id: 'darwin-arm64',
    asset: 'darwin-arm64',
    rust: 'aarch64-apple-darwin',
    os: 'darwin',
    cpu: 'arm64',
    label: 'macOS on Apple silicon',
  },
  {
    id: 'darwin-x64',
    asset: 'darwin-amd64',
    rust: 'x86_64-apple-darwin',
    os: 'darwin',
    cpu: 'x64',
    label: 'macOS on Intel',
  },
  {
    id: 'linux-arm64',
    asset: 'linux-arm64',
    rust: 'aarch64-unknown-linux-musl',
    os: 'linux',
    cpu: 'arm64',
    label: 'Linux on arm64',
  },
  {
    id: 'linux-x64',
    asset: 'linux-amd64',
    rust: 'x86_64-unknown-linux-musl',
    os: 'linux',
    cpu: 'x64',
    label: 'Linux on x86-64',
  },
  {
    id: 'win32-arm64',
    asset: 'windows-arm64',
    rust: 'aarch64-pc-windows-msvc',
    os: 'win32',
    cpu: 'arm64',
    label: 'Windows on arm64',
  },
  {
    id: 'win32-x64',
    asset: 'windows-amd64',
    rust: 'x86_64-pc-windows-msvc',
    os: 'win32',
    cpu: 'x64',
    label: 'Windows on x86-64',
  },
];

export const SCOPE = '@ai-ecoverse';
export const NAME = `${SCOPE}/slicc-serve`;

export const KERNEL = '>=1.153.0';

export function mainManifest(version, platforms) {
  return {
    name: NAME,
    version,
    description:
      'serve and scserve: publish a directory or a local server at a public capability URL through the SLICC tray hub, on any host and inside SLICC',
    license: 'Apache-2.0',
    repository: { type: 'git', url: 'git+https://github.com/ai-ecoverse/slicc-serve.git' },
    homepage: 'https://github.com/ai-ecoverse/slicc-serve#readme',
    bin: { scserve: 'bin/scserve.js' },
    files: ['bin/scserve.js', 'bin/serve.wasm'],
    engines: { node: '>=18', 'slicc-kernel': KERNEL },
    slicc: {
      abi: 'wasi',
      commands: {
        serve: { wasm: 'bin/serve.wasm' },
        scserve: { wasm: 'bin/serve.wasm' },
      },
    },
    optionalDependencies: Object.fromEntries(platforms.map((name) => [name, version])),
  };
}

export function assetName(target) {
  return `scserve-${target.asset}${target.os === 'win32' ? '.exe' : ''}`;
}
