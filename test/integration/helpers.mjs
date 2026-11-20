import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { after } from 'node:test';
import { fileURLToPath } from 'node:url';

export const root = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
export const exe = process.platform === 'win32' ? 'scserve.exe' : 'scserve';
export const binary = process.env.SCSERVE_BIN || join(root, 'target', 'debug', exe);
export const expectedVersion = process.env.SCSERVE_VERSION || '0.0.0';

export function sandbox(hub) {
  const dir = mkdtempSync(join(tmpdir(), 'scserve-it-'));
  const env = {
    PATH: process.env.PATH,
    SystemRoot: process.env.SystemRoot || '',
    HOME: join(dir, 'home'),
    USERPROFILE: join(dir, 'home'),
    XDG_CONFIG_HOME: join(dir, 'config'),
    XDG_STATE_HOME: join(dir, 'state'),
    APPDATA: join(dir, 'config'),
    LOCALAPPDATA: join(dir, 'state'),
    TEMP: process.env.TEMP || tmpdir(),
    TMP: process.env.TMP || tmpdir(),
    TMPDIR: process.env.TMPDIR || tmpdir(),
    ...(hub ? { SLICC_TRAY_HUB: hub.origin } : {}),
  };
  const stateDir = join(dir, 'state', 'slicc', 'serve');
  const configDir = join(dir, 'config', 'slicc', 'serve');
  const box = {
    dir,
    env,
    stateDir,
    configDir,
    site(files = { 'index.html': '<h1>hello</h1>\n' }, name = 'site') {
      const at = join(dir, name);
      for (const [path, content] of Object.entries(files)) {
        const file = join(at, path);
        mkdirSync(dirname(file), { recursive: true });
        writeFileSync(file, content);
      }
      mkdirSync(at, { recursive: true });
      return at;
    },
    run(args, extra = {}, options = {}) {
      return run(args, { ...env, ...extra }, options);
    },
    start(args, extra = {}) {
      return start(args, { ...env, ...extra });
    },
    tray() {
      const file = join(configDir, 'tray.json');
      return existsSync(file) ? JSON.parse(readFileSync(file, 'utf8')) : null;
    },
    async saveTray(at = hub) {
      const res = await fetch(`${at.origin}/tray`, { method: 'POST', body: '{}' });
      const created = await res.json();
      const tray = {
        hub: at.origin,
        trayId: created.trayId,
        controllerToken: created.capabilities.controller.token,
        controllerUrl: created.capabilities.controller.url,
        controllerId: randomUUID(),
        createdAt: new Date().toISOString(),
      };
      mkdirSync(configDir, { recursive: true });
      writeFileSync(join(configDir, 'tray.json'), JSON.stringify({ current: tray, retired: [] }));
      return tray;
    },
    relay() {
      const file = join(stateDir, 'relay.json');
      return existsSync(file) ? JSON.parse(readFileSync(file, 'utf8')) : null;
    },
    kill() {
      const relay = box.relay();
      if (relay) {
        try {
          process.kill(relay.pid);
        } catch {}
      }
    },
  };
  after(async () => {
    box.kill();
    await new Promise((r) => setTimeout(r, 100));
    rmSync(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
  });
  return box;
}

export function run(args, env, { input, timeout = 60_000 } = {}) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(binary, args, {
      env,
      stdio: [input === undefined ? 'ignore' : 'pipe', 'pipe', 'pipe'],
    });
    const out = [];
    const err = [];
    child.stdout.on('data', (c) => out.push(c));
    child.stderr.on('data', (c) => err.push(c));
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error(`scserve ${args.join(' ')} timed out\n${Buffer.concat(err)}`));
    }, timeout);
    child.on('error', reject);
    child.on('close', (code) => {
      clearTimeout(timer);
      resolvePromise({
        code,
        stdout: Buffer.concat(out).toString(),
        stderr: Buffer.concat(err).toString(),
      });
    });
    if (child.stdin) {
      child.stdin.on('error', (e) => {
        if (!['EPIPE', 'EOF', 'ECONNRESET'].includes(e.code)) reject(e);
      });
      child.stdin.end(input);
    }
  });
}

export function start(args, env) {
  const child = spawn(binary, args, { env, stdio: ['ignore', 'pipe', 'pipe'] });
  let stdout = '';
  let stderr = '';
  const waiters = [];
  const check = () => {
    for (const w of [...waiters]) {
      const m = w.stream().match(w.pattern);
      if (m) {
        waiters.splice(waiters.indexOf(w), 1);
        clearTimeout(w.timer);
        w.resolve(m);
      }
    }
  };
  child.stdout.on('data', (c) => {
    stdout += c;
    check();
  });
  child.stderr.on('data', (c) => {
    stderr += c;
    check();
  });
  const exited = new Promise((r) => child.on('close', (code, signal) => r({ code, signal })));
  after(() => {
    if (child.exitCode === null) child.kill();
  });
  const waitFor = (which, pattern, ms = 30_000) =>
    new Promise((resolvePromise, reject) => {
      const w = {
        pattern,
        stream: () => (which === 'stdout' ? stdout : stderr),
        resolve: resolvePromise,
        timer: setTimeout(
          () =>
            reject(
              new Error(`timed out waiting for ${pattern} on ${which}:\n${stdout}\n${stderr}`)
            ),
          ms
        ),
      };
      waiters.push(w);
      check();
    });
  return {
    child,
    exited,
    stdout: (pattern, ms) => waitFor('stdout', pattern, ms),
    stderr: (pattern, ms) => waitFor('stderr', pattern, ms),
    get out() {
      return stdout;
    },
    get err() {
      return stderr;
    },
  };
}

export function get(url, headers = {}) {
  return fetch(url, { headers }).then(async (res) => ({
    status: res.status,
    headers: res.headers,
    body: Buffer.from(await res.arrayBuffer()),
  }));
}

export async function eventually(check, ms = 10_000) {
  const deadline = Date.now() + ms;
  let last;
  while (Date.now() < deadline) {
    try {
      const value = await check();
      if (value) return value;
    } catch (e) {
      last = e;
    }
    await new Promise((r) => setTimeout(r, 50));
  }
  throw last ?? new Error('condition never held');
}

export function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

export function scratch() {
  const dir = mkdtempSync(join(tmpdir(), 'scserve-scratch-'));
  after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}
