import { createContext, runInContext } from 'node:vm';

const PNG = Buffer.from(
  '89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c6360000002000154a24f5d0000000049454e44ae426082',
  'hex'
).toString('base64');

function remote(value) {
  if (value === null) return { type: 'object', subtype: 'null', value: null };
  if (value === undefined) return { type: 'undefined' };
  if (['number', 'boolean', 'string'].includes(typeof value)) return { type: typeof value, value };
  return { type: 'object', description: String(value) };
}

export function visitor({
  title = 'Bridged page',
  html = '<h1>bridged</h1>',
  href = '',
  snapshot = { json: [{ role: 'heading', name: 'bridged', level: 1 }], refs: [] },
  elements = {},
  fetch = globalThis.fetch,
} = {}) {
  class Node {
    constructor(name) {
      this.nodeType = 1;
      this.nodeName = name.toUpperCase();
      this.localName = name;
      this.childNodes = [];
      this.attributes = [];
    }
  }
  const button = new Node('button');
  for (const el of Object.values(elements)) {
    el.scrollIntoView ??= () => {};
    el.getBoundingClientRect ??= () => ({ x: 10, y: 20, width: 100, height: 40 });
  }
  const document = {
    title,
    readyState: 'complete',
    documentElement: { outerHTML: `<html><body>${html}</body></html>` },
    body: { innerText: html.replace(/<[^>]+>/g, '') },
    querySelector: (selector) => elements[selector] ?? null,
    elementFromPoint: () => Object.values(elements)[0] ?? null,
  };
  const context = createContext({
    __sliccAriaElements: [button],
    HTMLIFrameElement: class {},
    Element: class {},
    Node,
    URL,
    fetch,
    btoa,
    atob,
    Uint8Array,
    document,
    location: { href },
    window: {},
    eval: (src) =>
      String(src).includes('sliccAriaSnapshot(') ? context.snapshotOf : runInContext(src, context),
  });
  const seen = [];
  return {
    seen,
    async answer(frame) {
      seen.push(frame.method);
      const params = frame.params ?? {};
      switch (frame.method) {
        case 'Runtime.evaluate':
          try {
            context.snapshotOf = snapshot;
            const value = await runInContext(String(params.expression ?? ''), context, {
              timeout: 1000,
            });
            return { result: { result: remote(value) } };
          } catch (err) {
            return {
              result: {
                result: { type: 'object', subtype: 'error' },
                exceptionDetails: {
                  text: err.message,
                  exception: { type: 'object', description: String(err) },
                },
              },
            };
          }
        case 'DOM.getDocument':
          return { result: { root: { nodeId: 1, nodeName: '#document', childNodeCount: 1 } } };
        case 'DOM.querySelector':
          return { result: { nodeId: 0 } };
        case 'Page.captureScreenshot':
          return { result: { data: PNG } };
        case 'Page.navigate':
          return { result: { frameId: 'cherry-frame', loaderId: 'cherry-loader' } };
        case 'Input.dispatchMouseEvent':
        case 'Input.dispatchKeyEvent':
          return { result: {} };
        default:
          return {
            error: { code: -32601, message: `Cherry: unsupported CDP method '${frame.method}'` },
          };
      }
    },
  };
}
