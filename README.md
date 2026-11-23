# slicc-serve

`scserve` publishes a directory, or a server that already runs on this machine, at a public capability URL on `sliccy.now`, through the SLICC serve hub ([sliccy-ai](https://github.com/ai-ecoverse/sliccy-ai#previews-on-sliccynow-the-serve-hub)). It is slicc 6's `serve` for any host: `scserve ./dist` prints `https://<label>.sliccy.now/` and returns, and anyone with the URL sees the files, pulled from this machine on demand while it is online. `scserve --port 5173` does the same for vite, `python -m http.server` or any other local server, every request relayed whole (method, query, headers, body, status), and `scserve --ttl 7d ./dist` uploads an immutable snapshot that stays up without this machine.

In SLICC the same program is `serve` and `scserve` (`bin/serve.wasm`). There it serves the directory on the kernel's loopback, and seven's page, which keeps a publisher host of its own on the same hub, publishes it: `serve` asks the page over `/dev/embedder` (see [In SLICC](#in-slicc)).

This README is the spec. The plan is [#1](https://github.com/ai-ecoverse/slicc-serve/issues/1); the move from SLICC 6's tray hub to the serve hub is [#7](https://github.com/ai-ecoverse/slicc-serve/issues/7).

## Install

```sh
curl -fsSL https://get.sliccy.ai/serve | sh
```

On Windows, in PowerShell:

```powershell
irm https://get.sliccy.ai/serve.ps1 | iex
```

`get.sliccy.ai/serve` redirects to the latest release's `install.sh`, and `serve.ps1` runs its `install.ps1` ([sliccy-ai](https://github.com/ai-ecoverse/sliccy-ai)). The release assets work directly too, and options go after `sh -s --` (for example `curl -fsSL https://get.sliccy.ai/serve | sh -s -- --dir ~/bin`):

```sh
curl -fsSL https://github.com/ai-ecoverse/slicc-serve/releases/latest/download/install.sh | sh
```

```powershell
irm https://github.com/ai-ecoverse/slicc-serve/releases/latest/download/install.ps1 | iex
```

With npm (Node 18 or later), which picks the binary for the platform from an optional dependency:

```sh
npm install -g @ai-ecoverse/slicc-serve
```

In SLICC, `pnpm add -g @ai-ecoverse/slicc-serve` puts `serve` and `scserve` in `/usr/bin` (the package declares both in `slicc.commands` with `abi: wasi`).

seven's pnpm enforces `minimumReleaseAge=1440`, so it installs a release only once it is 24 hours old. To install a newer one before that:

```sh
pnpm add -g @ai-ecoverse/slicc-serve --config.minimumReleaseAgeExclude='@ai-ecoverse/slicc-serve*'
```

Binaries: macOS (arm64, x86-64, signed and notarized), Linux (arm64, x86-64, static musl) and Windows (arm64, x86-64). [The installers](#the-installers) has the details.

## Usage

```
scserve [--entry FILE] [--ttl DURATION] [--local] [--foreground] [--open] [--json] DIRECTORY
scserve --port PORT [--entry PATH] [--local] [--foreground] [--open] [--json]
scserve --bridge [--max-tabs N] [--quiet] [other options] DIRECTORY | --port PORT
scserve --list [--json]
scserve --stop TOKEN|URL|PORT [--json]
scserve --logs [TOKEN|URL] [--lines N] [--json]
scserve --truncate [TOKEN|URL] [--json]
scserve --version | --upgrade | --help
```

| option | does |
| --- | --- |
| `DIRECTORY` | serves its files with the built-in static server and publishes them |
| `--port PORT` | publishes the server that already runs on `127.0.0.1:PORT` instead (1 to 65535) |
| `--entry FILE` | the page the URL opens, relative to the directory (or a path on the server); default: the directory's index |
| `--ttl DURATION` | publishes an immutable snapshot for `DURATION`: a whole number and `m`, `h`, `d` or `w`, at most `30d` |
| `--local` | stays on this machine: no serve hub, prints `http://127.0.0.1:PORT/` only |
| `--foreground` | stays attached instead of returning; Ctrl-C (or the end of the process) stops the preview |
| `--open` | opens the URL with the platform's opener once it is published (and a sign-in page, see [The serve hub](#the-serve-hub)) |
| `--json` | prints the result, or the error, as one JSON object (an array for `--list`) |
| `--bridge` | makes every visitor's tab a target that a CDP client on this machine can drive, and passes the page's `window.slicc.emit()` calls on (see [The bridge](#the-bridge---bridge)) |
| `--no-bridge` | the default: no bridge |
| `--max-tabs N` | with `--bridge`: at most `N` visitor tabs bridged at once, 1 to 100 (default 20); the hub refuses more |
| `--quiet` | with `--bridge`: no first-visit announcement (the visit is still recorded for `--logs`) |
| `--logs [TOKEN\|URL]` | prints the bridge's lifecycle records (connects, disconnects, emits), of one preview or of all |
| `--lines N` | with `--logs`: only the newest `N` records |
| `--truncate [TOKEN\|URL]` | clears those records and re-arms the first-visit announcement |
| `--list` | lists the previews this machine published |
| `--stop TOKEN\|URL\|PORT` | stops a preview, by its token, its public URL, its local URL or its local port |
| `--version` | prints the version alone (`1.2.0`) |
| `--upgrade` | see [`--upgrade`](#--upgrade) |
| `-h`, `--help` | prints the usage and the options to stdout and exits 0, wherever it appears among the options |

Options come before or after the directory; `--name=value` works for options with a value, and `--` ends the options. Give a directory or `--port`, not both and not neither. `--ttl` needs a directory and is neither `--local` nor `--foreground`. `--list` and `--stop` take only `--json`, `--logs` only a target, `--lines` and `--json`, `--truncate` only a target and `--json`; `--version` and `--upgrade` take nothing else. `--bridge` is neither `--ttl` (a snapshot has no visitor sockets) nor `--local`, and not with `--no-bridge`; `--max-tabs` and `--quiet` need `--bridge`. `--entry` must stay inside the directory: no leading `/` or `\`, no `..`, no `:`; `./` and doubled slashes are dropped, and `\` counts as `/`.

`--project` is refused with `scserve: --project is obsolete: give the directory to serve itself`.

### Exit status

**0** ok, **1** failure (a missing directory, the hub's error, an unreachable hub), **2** usage error (`scserve: <what is wrong>` and the usage on stderr), **3** the serve hub wants a sign-in or refused the request, **4** a quota of the serve hub is used up. With `--json`, a failure also prints `{"ok":false,"exit":N,"lines":[…],"signIn":URL|null}` to stdout.

### Output

On success, stdout is the URL alone (so `url=$(scserve dist)` works), and stderr says what it means:

```
$ scserve ./site
https://3f0c…-a91e….sliccy.now/
scserve: /home/me/site is public at https://3f0c…-a91e….sliccy.now/ (here: http://127.0.0.1:52811/)
scserve: stop it with: scserve --stop 3f0c…-a91e…
```

`--json` prints the record instead: `mode` (`live`, `persistent` or `local`), `previewToken`, `url`, `localUrl`, `root`, `port`, `entry`, `hostId`, `createdAt`, `expiresAt`, `foreground`, and for a snapshot `files` and `bytes`.

Without `--entry`, the URL ends in `/` and opens what the server answers there: `index.html`, else a directory listing (the hub's `entry` is `/`). With `--entry`, the URL names the entry, each segment percent-encoded (`--entry 'docs/a b.html'` is `/docs/a%20b.html`). The URL's host, `<previewToken>.sliccy.now`, is the capability: the preview token is the host's 20 hex digits, `-` and 32 random hex digits.

## What runs where

```
scserve DIR ──spawn──► relay (scserve __relay, detached, one per user)
                         ├─ static server on 127.0.0.1:<random> per directory
                         └─ publisher WebSocket of this machine's host ◄── serve hub ◄── visitor
                              http.request ──the same request──► 127.0.0.1:<port> ──► http.response + body
```

- **The relay.** The hub keeps one publisher socket per host, so one process holds it for every preview: `scserve` starts `scserve __relay` detached (its own session on Unix, `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` on Windows, with the caller's standard handles not inherited), hands it the preview over a loopback control socket, prints the answer and exits. The relay runs the static servers, holds the host's publisher WebSocket, relays each visitor's request to the preview's origin, and exits 5 s after its last preview stops. Its control socket is on `127.0.0.1` with a random port and a 48-hex-digit token in `relay.json` (mode `0600`); a request without the token is dropped. A lock (`relay.lock`) keeps it to one relay per state directory; its stderr goes to `relay.log`.
- **`--foreground`** keeps the control connection open instead of returning: the relay streams one line per visit (`scserve: GET /app.js?v=2 200 1234`: method, path and query, status, body bytes) to stderr, and when the connection ends (Ctrl-C, the process killed, the terminal closed) it stops the preview on the hub and stops its server. A `--stop` from elsewhere ends the foreground `scserve` with `scserve: stopped: <reason>` and exit 0.
- **`--local`** never talks to the hub. `--local DIR` runs the static server in the relay (listed, stopped with `--stop <local URL>`); `--local --foreground DIR` runs it in the `scserve` process itself and logs each request; `--local --port PORT` only prints `http://127.0.0.1:PORT/[ENTRY]` (with `scserve: nothing answers on 127.0.0.1:PORT yet` when nothing does).
- **`--port`** publishes an existing server: the relay passes each visitor's request to `127.0.0.1:PORT` as it came and its answer back as it went (and warns the same way when nothing answers yet). A directory goes through the same relay, to its static server.
- **`--ttl`** needs nothing running: `scserve` itself walks the directory, mints the snapshot, uploads the files and finalizes it, then exits.

### The static server

The built-in server answers `GET` and `HEAD` (others `405` with `Allow: GET, HEAD`, so a visitor's `POST` to a directory preview gets that) on `127.0.0.1` only, as the user who ran `scserve`, so file permissions hold.

- **Paths** are percent-decoded once and split on `/`. A segment that is `.` or `..`, or holds `\`, `:` or NUL, or a path that doesn't decode to UTF-8, is `400`. A segment starting with `.` (`.env`, `.git`) is hidden: `404`, never listed, never snapshotted; `.well-known` is the exception. The resolved file must lie inside the directory after symlinks are resolved, with no hidden segment in its resolved path; a symlink out of it, or to a hidden file or directory (`public.txt -> .env`), is `404`, and `--entry` naming either is `entry file not found`. Names that aren't UTF-8 are skipped.
- **Directories:** a path without its trailing `/` is `301` to the same path with it (query kept); with it, `index.html` if there is one, else an HTML listing of the visible entries.
- **Files** get `Content-Type` from the extension (`text/html; charset=utf-8`, `text/css`, `text/javascript`, `application/json`, `image/*`, `font/*`, `video/*`, `audio/*`, `application/wasm`, …; unknown is `application/octet-stream`), `Content-Length`, `Last-Modified`, `Accept-Ranges: bytes`, `Cache-Control: no-cache` and `X-Content-Type-Options: nosniff`.
- **Ranges:** one `bytes=a-b`, `bytes=a-` or `bytes=-n` range is `206` with `Content-Range`; one outside the file is `416` with `Content-Range: bytes */size`; several ranges, another unit or an `If-Range` get the whole file (`200`), as slicc 6's `parseByteRange` does.
- HTTP/1.1 keep-alive, a 16 KiB limit on the request head (`431`), and a 30 s idle timeout per connection. A request body is read and dropped before the answer when its `Content-Length` is at most 1 MiB, keeping the connection; a larger or chunked one, or one the client holds back for `Expect: 100-continue`, gets its answer, then the connection is closed after reading what is left for up to 2 s (64 MiB at most), so the client sees the answer instead of a reset.

### The relay's answers

The hub relays every visitor request to a live preview over the publisher socket as a stream ([sliccy-ai: Full HTTP over the publisher's socket](https://github.com/ai-ecoverse/sliccy-ai#full-http-over-the-publishers-socket)); the relay is a plain reverse proxy in front of the preview's origin:

- **The request.** `http.request {id, previewToken, method, url, path, headers, body}` opens stream `id`. The relay finds the preview by its token (one it doesn't serve, any more, is `404`), opens a connection to `127.0.0.1:<port>` and sends `method` and `path` (path and query, still percent-encoded, exactly as the visitor sent them) with the visitor's headers in their order, repeats kept, as HTTP/1.1 with `Host: 127.0.0.1:<port>` (which dev servers that check `Host` accept), `X-Forwarded-Host: <label>.sliccy.now`, `X-Forwarded-Proto: https` and `Connection: close`; hop-by-hop headers and any `Host` or `X-Forwarded-*` of the visitor's are dropped, and so is a header whose name isn't a token or whose value holds CR, LF or NUL. A `method` that isn't a token or a `path` that doesn't start with `/` or holds a space or control character is `400`.
- **The request body.** With `body: true`, the binary frames that follow (a 4-byte big-endian stream id, then the bytes) go to the origin as they come, with the visitor's `Content-Length`, or chunked when there was none, and `http.request.end` ends it. The relay answers each frame with `http.ack {id, bytes}` once the origin has taken it, so the hub never has more than 1 MiB of it in flight. When the origin stops reading (it answered early, or closed), the rest is acknowledged and dropped.
- **The answer.** The origin's status and headers go back unchanged as `http.response {id, status, headers, body}` (`[name, value]` pairs with lowercase names, repeats kept, so several `Set-Cookie` lines survive), apart from `Connection`, `Keep-Alive` and `Transfer-Encoding` (the body goes out de-chunked) and a `Location` on the origin itself (`http://127.0.0.1:<port>/x?y`, `http://localhost:<port>/x?y`), which becomes the bare `/x?y` so the redirect stays on the preview. Redirects are passed on, never followed; `1xx` interim answers are skipped; `Range`, conditional requests and cookies are the origin's to answer, so `206`, `304` and `416` come from it. The body follows in binary frames of at most 64 KiB, framed by the origin's `Content-Length`, chunked encoding or the end of the connection, then `http.response.end`; `HEAD`, `204` and `304` have `body: false` and no frames. There is no size limit: the relay keeps at most 1 MiB unacknowledged and waits for the hub's `http.ack` as the visitor reads, so a long download or a server-sent event stream runs as long as the visitor reads it.
- **Early answers.** The relay reads the origin's answer while it is still sending the request body, so an origin that answers before it has read the body (a `413`, a refused login, `Expect: 100-continue`) reaches the visitor at once (the 120 s read timeout on the origin, which `SCSERVE_ORIGIN_TIMEOUT_MS` replaces for tests, starts once the body is sent, so a slow upload isn't cut off); when that answer is complete, the rest of the body is acknowledged and dropped and the connection closed.
- **Failures.** An origin that doesn't answer is `502` `nothing answers on 127.0.0.1:PORT: …`; one that answers something that isn't HTTP/1.x (or headers over 64 KiB or 256 lines, a `101`, a status outside 200 to 599, an invalid `Content-Length`) is `502` `127.0.0.1:PORT gave no valid answer: …`. A body that breaks off (shorter than its `Content-Length`, a broken chunk) is `http.cancel {id, reason}`, which ends the visitor's body in an error.
- **Cancels.** The hub's `http.cancel` (the visitor went away, or one of the hub's timeouts: 30 s for the answer's head, 120 s between frames, 60 s without acknowledgements) closes the connection to the origin and ends the stream; nothing more is sent for it. When the publisher socket drops, every open stream is cancelled the same way.
- **The socket.** The relay opens `wss://seven.sliccy.ai/api/serve/host/<hostId>/ws` with `Authorization: Bearer <secret>`, gets `hello {hostId}` first, and sends the text frame `ping` every 30 s (the hub answers `pong`). `preview.revoked {previewToken}` (a preview stopped through the API) stops the preview locally (its server, its record, a foreground holder). Messages of other types are ignored.
- **When the socket drops**, the relay reconnects after 1, 2, 5, 10, then every 30 s, while it has public previews; visitors get the hub's `502` meanwhile, since a live preview stays minted while its publisher is offline. If the host is refused or gone, its live previews are stopped. A close with `4000` (another socket for the same host replaced this one) stops the live previews without reconnecting, so two relays sharing a `host.json` don't take the socket from each other in turn; `4001` (the host was deleted) stops them and forgets the host. When it (re)connects, the relay lists the host's previews and stops every live one whose `tag` is none of the previews it serves: leftovers of a relay that died.

### Snapshots (`--ttl`)

`scserve --ttl 7d DIR` collects the visible files (hidden ones, symlinks out of the directory and symlinks to hidden files excluded, sorted). A directory reachable by several paths, its own and a symlink's (`latest -> v2`), is collected under each of them, as the live preview serves it; only a symlink back to a directory the walk is inside (`v2/up -> ..`) is skipped, so links can't loop, and the limits below, plus 10,000 directories walked, bound what repeats. It refuses more than 1,000 files, a file over 25 MiB or more than 50 MiB in total before anything is sent, and needs its entry file (`index.html` by default; `scserve: DIR has no index.html; a --ttl snapshot needs its entry file (give one with --entry)`). It mints with `ttlMs` (`POST …/previews {entry, tag, ttlMs}`), uploads the files four at a time (`PUT …/previews/<token>/files/<path>`, each segment percent-encoded, with the host's bearer, the file's `Content-Length` and type; a failed upload can be sent again, since the same path replaces the file), finalizes (`POST …/previews/<token>/finalize`), and prints the URL. If an upload or the finalize fails, it stops the snapshot on the hub (`DELETE …/previews/<token>`) before it exits; the hub's own limits are the same as the walker's (`413` `FILE_TOO_LARGE`, `TOO_MANY_FILES`, `SNAPSHOT_TOO_LARGE`), and a finalize without the entry file is `400` `ENTRY_MISSING`, all exit 1. The TTL counts from the finalize; the snapshot lasts until then and is served by the hub without this machine (`GET` and `HEAD`; a path ending in `/` is its `index.html`, and a directory path without the `/` is `301` to it).

### `--list` and `--stop`

`--list` asks the hub for the previews of this machine's host (`GET …/previews`), joins them with the local records, and prints `TOKEN MODE EXPIRES URL ROOT`: `live`, `snapshot`, `uploading` or `local`, `(foreground)` for an attached one. Records whose relay is gone, snapshots past their expiry and previews the hub no longer has are dropped. `no previews` when there are none.

`--stop` takes a preview token, a public URL (its host's first label is the token), a local URL, a port or a record id. A local URL matches by its port when its host is a loopback name (`127.0.0.1`, any `127.x.y.z`, `localhost`, `*.localhost`), so `http://localhost:61489/` and `61489` both stop the preview served on `127.0.0.1:61489`; a port matches a preview's static server or the server a `--port` preview publishes. When several previews match, it stops none and exits 1 with `scserve: X matches N previews; stop one by its token`. A hub preview is stopped with `DELETE /api/serve/host/<hostId>/previews/<token>`; the relay then gets `preview.revoked` and stops its server. A local preview is stopped through the relay. It prints `stopped <token or URL>`; `scserve: no preview matches X`, `scserve: <token> is not a preview of this machine's host` (its first 20 hex digits aren't this machine's `hostId`) and `scserve: the serve hub has no preview <token> (already stopped or expired)` (`404 PREVIEW_NOT_FOUND`) exit 1.

## The bridge (`--bridge`)

`scserve --bridge ./app` is slicc 6's `serve --bridge` on a host: every tab a visitor opens on the public URL becomes a CDP target on this machine, `preview:<token>:<connId>`, that a CDP client (playwright-cli, a script with `chrome-remote-interface`) can evaluate in, query, click, type into, screenshot and navigate, and the page's `window.slicc.emit(name, detail)` calls arrive here. The hub injects slicc 6's bridge bootstrap into a bridged preview's HTML (`<script src="/__slicc/preview-bridge.js">`, into every `200` `text/html` answer without a `Content-Encoding`; it asks the origin for `identity` so it can), holds one `/__slicc/bridge` WebSocket per visitor tab, and relays between those sockets and the host's publisher socket ([sliccy-ai: The bridge on sliccy.now](https://github.com/ai-ecoverse/sliccy-ai#the-bridge-on-sliccynow)). `scserve`'s relay is that publisher.

```
visitor tab ──wss /__slicc/bridge──► serve hub ──publisher WS──► relay ◄──ws──── CDP client
  (bootstrap answers cdp.req)           bridge.connected / .disconnected          http://127.0.0.1:<port>/<secret>/
                                        bridge.cdp.request / .response / .close
                                        bridge.emit
```

**The mint** of a bridged preview adds `bridge: true`, `maxTabs` (`--max-tabs`, default 20) and `quiet` (`--quiet`) to `POST …/previews`. The page's emits arrive on the publisher socket as `bridge.emit {previewToken, connId, name, detail, at}`. An emit counts only when it comes from a tab that is connected to that preview right now: the hub's beacon emits (`connId: null`, which the bootstrap posts to `/__slicc/emit` while its socket is down) and emits for a tab the relay doesn't know are dropped, as seven's page drops them.

**The CDP endpoint.** The relay runs one CDP server for all its bridged previews, on `127.0.0.1` with a random port (never 9222 to 9229), from the first bridged preview until the relay exits. Every path carries a 32-hex-digit secret, made fresh each time the relay starts and kept in `relay.json` (`0600`); a request without it is `404`, one whose `Host` isn't `127.0.0.1:<port>` or `localhost:<port>` is `403` (no DNS rebinding), and a WebSocket upgrade with an `Origin` header of a web page is `403`. So a page in the user's browser can't reach it, and neither can another user of the machine who doesn't hold the secret.

| path | answers |
| --- | --- |
| `GET /<secret>/json/version` | `{"Browser": "scserve/<version>", "Protocol-Version": "1.3", "webSocketDebuggerUrl": "ws://127.0.0.1:<port>/<secret>/devtools/browser"}` |
| `GET /<secret>/json/list`, `/<secret>/json` | one entry per bridged tab: `{"id": "preview:<token>:<connId>", "type": "page", "title": <the preview's directory, or 127.0.0.1:PORT>, "url": <the preview's public URL; the hub doesn't say which page the tab is on>, "description": "<origin> <userAgent>"}`; no per-target sockets |
| `GET /<secret>/devtools/browser` (WebSocket) | a browser-level CDP connection, below |

`scserve --bridge` prints the endpoint with the URL (`scserve: its visitor tabs are CDP targets at http://127.0.0.1:<port>/<secret>/`), and `--json` and `--list --json` give it as `cdpUrl`. The secret is a capability: anyone who has the URL can drive the bridged tabs while the relay runs.

**The browser connection** emulates the `Target` domain over the bridged tabs, with Chrome's flattened sessions:

- `Target.getTargets` lists the tabs (`type: "page"`, `attached` when a session is on it); `Target.setDiscoverTargets {discover: true}` sends `Target.targetCreated` for each tab there is and each that connects, and `Target.targetDestroyed` for each that goes; `Target.attachToTarget {targetId, flatten: true}` answers a `sessionId` and sends `Target.attachedToTarget`; `Target.setAutoAttach` attaches every tab there is and every new one; `Target.detachFromTarget` ends a session (`Target.detachedFromTarget`); `Target.closeTarget` closes the visitor's bridge (`bridge.close`). `Target.createTarget`, `Target.createBrowserContext` and `Browser.close` are `-32000`, `a preview bridge can't open or close the visitor's browser`; `Browser.getVersion` answers as `/json/version`; any other method without a `sessionId` is `-32601`.
- **In a session**, as slicc 6's `PreviewBridgeCdpTransport` and `SyntheticCdpTransport` do: `Page.enable`, `DOM.enable`, `Page.bringToFront`, `Runtime.disable`, `Runtime.runIfWaitingForDebugger` and `Target.setAutoAttach` answer `{}`; `Runtime.enable` answers `{}` and sends `Runtime.executionContextCreated` for the main world (once per enable); `Page.getFrameTree` and `Page.createIsolatedWorld` answer from the tab's URL and origin. Everything else goes to the tab as `bridge.cdp.request {connId, id, method, params, sessionId}`, and the tab's `bridge.cdp.response` is the answer. The bootstrap answers `Runtime.evaluate`, `DOM.getDocument`, `DOM.querySelector`, `DOM.getBoxModel`, `Input.dispatchMouseEvent`, `Input.dispatchKeyEvent`, `Page.captureScreenshot` (html2canvas), `Page.navigate` and `Target.createTarget` (it opens the URL), and `-32601` for the rest. After a `Page.navigate`, the relay sends `Page.frameNavigated`, a new execution context (when `Runtime` is on) and `Page.loadEventFired`, as slicc 6 does, since the hub relays no CDP events from the tab. A tab that doesn't answer within 30 s is `-32000`, `the visitor's tab didn't answer within 30 s`; a tab that is gone, `-32000`, `the visitor's tab is gone` (the hub's `Preview bridge connection is gone`).
- When a tab disconnects, its sessions end (`Target.detachedFromTarget`, `Target.targetDestroyed`) and its pending calls fail. When the preview stops, the hub closes its tabs' sockets. A CDP client closing its socket detaches its sessions and changes nothing for the visitor.
- **When the relay's publisher socket drops**, the visitor sockets stay on the hub. The relay keeps its targets, and when it is back the hub replays `bridge.connected {…, replay: true}` for the tabs still there: they keep their target ids and sessions. A tab the hub doesn't replay within 3 s of the reconnect closed meanwhile; the relay drops it (`Target.targetDestroyed`, a `disconnected` record with the reason `the serve hub no longer had it after the relay reconnected`).

**What a client can do through the bridge.** The bootstrap the hub injects answers a small set of methods in the page itself, so the relay makes up the rest of what [slicc-cdp](https://github.com/ai-ecoverse/slicc-cdp)'s `playwright-cli` and `curlwright` need from a browser, using only the bootstrap's `Runtime.evaluate`. With it, `tab-list`, `eval` (expressions and functions), `snapshot`, `click`, `fill`, `type`, `press`, `screenshot`, `goto` and `curlwright` requests work on `preview:<token>:<connId>` targets (`playwright-cli --cdp <the endpoint's ws URL>`), each action exactly once:

- **Results:** every `Runtime.evaluate` goes to the tab wrapped as `(0, eval)(<the expression>)` whose awaited result the page describes as JSON: with `returnByValue`, its value (with `NaN`, `Infinity`, `-0` and BigInts as `unserializableValue`, and inside arrays and objects as `JSON.stringify` writes them); without, its type, subtype (`null`, `array`, `node`, `error`), class and description, and for an object or a function a handle.
- **Object handles:** the page keeps each object a result hands out in a registry of its own (`globalThis[Symbol.for('scserve.bridge')]`), and its `objectId` names that entry. `Runtime.callFunctionOn` calls the function on the object itself, with handles and values as arguments, so the expression that produced a handle never runs again; the function is compiled by the page's indirect `eval`, at its global scope, so it sees the page's globals and none of the bridge's names. `Runtime.getProperties` lists a handle's own string-keyed properties from their descriptors, reporting an accessor's `get` and `set` without calling them (and only accessors with `accessorPropertiesOnly`); `DOM.describeNode` (by `objectId` or `backendNodeId`) and `DOM.resolveNode` (by `backendNodeId`) work for DOM nodes, with backend node ids the page numbers itself. `Runtime.releaseObject` and `Runtime.releaseObjectGroup` drop entries (a handle joins the `objectGroup` of the `Runtime.evaluate`, `Runtime.callFunctionOn` or `DOM.resolveNode` that made it); a navigation drops them all, and a handle used after that is `-32000` (`the object is gone from the visitor's page`). Other methods that take an `objectId` of the bridge are `-32000`.
- **One world:** the page has one execution context, renewed by each navigation. `Page.createIsolatedWorld` answers that context, so scripts meant for an isolated world run in the page's own (and see its globals).
- **No trusted input:** a page script can only dispatch events with `isTrusted` false, so `Input.dispatchMouseEvent`, `Input.dispatchKeyEvent` and `Input.insertText` answer `{}` and dispatch nothing (the bootstrap's own, which clicks on a mouse press, would act a second time). `playwright-cli` checks whether its trusted input landed and otherwise dispatches the events in the page itself, which is what happens here, once. A client that relies on `Input.*` alone does nothing through the bridge, and a page that ignores untrusted events (`if (!event.isTrusted) return`) doesn't react.
- **No network domain:** `Inspector.enable`, `Fetch.enable`, `Fetch.disable`, `Network.enable`, `Network.disable` and `Network.setBypassServiceWorker` answer `{}` and no event ever follows; `Network.getAllCookies`, `Network.getCookies` and `Storage.getCookies` answer no cookies. The visitor's cookie jar, requests and responses don't reach the relay.
- **Not through the bridge:** console, network and dialog events, the `DOM` domain's node ids (`DOM.getDocument` and `DOM.querySelector` are the bootstrap's), PDF, screencasts, downloads, and a page whose own CSP forbids `eval` (the bootstrap's evaluation is the page's indirect `eval`).

**The lifecycle records.** The relay keeps the last 500 records of all its bridged previews, in memory (they are gone when the relay exits): `connected` (with the visitor's `origin`, `userAgent` and `connectedAt`), `disconnected` (with the `reason` when the hub gives one) and `emit` (the `name` and the `detail`, as JSON, cut to 1,000 characters). An emit, like a visitor's `origin`, `userAgent` and `reason`, is untrusted text from a stranger's browser; the hub already limits each emit to 16 KiB and 20 per tab every 10 s. Every line `scserve` prints for a person (`--foreground`'s event lines, `--logs`, the first-visit line) writes those fields with control characters (C0, DEL and C1) escaped as JSON escapes them (`\u001b`), so a visitor can't send escape sequences to the terminal; `--json` output is JSON already.

**The first visit.** The first tab that connects to a preview is announced, unless the preview is `--quiet`: its record is `announced` (the others `suppressed`), a `--foreground` `scserve` prints `scserve: first visitor on <url>: <origin> (<userAgent>)`, and the relay sends `preview.state.update {previewToken, announced: true}`, so the hub keeps the latch across relay restarts (it replays `preview.state {previewToken, quiet, announced}` to a publisher socket that connects, after `hello`, with `bridge.connected {…, replay: true}` for tabs already there, which are neither recorded nor announced again).

**Where events go:**

- A `--foreground` `scserve --bridge` prints one line per event to stderr, `scserve: connected <connId> <origin> <userAgent>`, `scserve: disconnected <connId> <reason>`, `scserve: emit <connId> <name> <detail>`; with `--json`, stdout has one JSON object per event after the record: `{"event": "connected"|"disconnected"|"emit", "previewToken", "connId", "at", …}`, the record's fields.
- `scserve --logs [TOKEN|URL] [--lines N]` asks the relay for them: `Preview lifecycle records (oldest to newest):`, then `<at>  <event>  <announced|suppressed|->  preview=<token> conn=<connId> <origin=… userAgent=…|reason=…|name=… detail=…>` per record, or `no lifecycle records (the relay keeps them in memory, so they are gone when it restarts)`; `--json` prints the array. A target that matches no preview the relay bridged is `scserve: no preview matches X` (exit 1); no relay running is no records. A stopped preview's records stay until `--truncate` or until the relay exits.
- `scserve --truncate [TOKEN|URL]` clears them (of that preview, or all), re-arms the first-visit announcement of each matching bridged preview (`preview.state.update {announced: false}`), and prints `cleared <n> records, re-armed <m> previews` (`--json`: `{"cleared": n, "rearmed": m}`).

## The serve hub

`scserve` talks to the serve hub at `https://seven.sliccy.ai` (`SLICC_SERVE_HUB` replaces it, for tests and staging), the API of [sliccy-ai](https://github.com/ai-ecoverse/sliccy-ai#previews-on-sliccynow-the-serve-hub):

| call | what for |
| --- | --- |
| `POST /api/serve/host` | a publisher host of this machine's own, `201 {hostId, secret}`, created once and kept in `host.json` |
| `GET /api/serve/host/<hostId>/ws` (WebSocket) | the relay's publisher socket, on the hub's origin as `ws:`/`wss:` |
| `POST /api/serve/host/<hostId>/previews` `{entry, tag, bridge?, maxTabs?, quiet?, ttlMs?}` | mints a preview: `entry` is the URL path it opens (`/` without `--entry`), `tag` is `/srv/serve/<16 hex>`, opaque to the hub and never the real path, so a relay that restarts can tell its leftovers |
| `PUT …/previews/<token>/files/<path>` and `POST …/previews/<token>/finalize` | a snapshot's files and its publication |
| `DELETE …/previews/<token>`, `GET …/previews` | `--stop`, `--list` and the relay's tidying |

Every call but the first sends `Authorization: Bearer <secret>`, to the hub's origin only. The hub has no sign-in and no quotas today; `scserve` is ready for it to gate publishing, and maps every answer of every call above, and of the WebSocket upgrade (any answer but `101`), the same way:

- **3xx** is never followed blindly. A `Location` on another origin (an auth page), or on the hub's own origin outside the API (anything but `/api/…`), exits 3 with `scserve: the serve hub wants you to sign in: <Location>`; with `--open`, or when you answer `y` on a terminal, the page opens. A same-origin redirect to an API path is followed, at most 5 times (a sixth exits 1, `redirected more than 5 times`), keeping method and body for `307`/`308` and switching to `GET` for `303` (and `301`/`302` after a `POST`); the bearer is only ever sent to the hub's origin.
- **401 and 403** (`UNAUTHORIZED`) exit 3 with the hub's `error` and `code`, and `scserve: sign in at <url>` from `Link: <url>; rel="login"` or the first URL in `WWW-Authenticate` (else the header itself). On a **saved** host, a refusal creates a new host and retries once; a second refusal exits 3.
- **402 and 429** exit 4 with the hub's error and `scserve: retry after <Retry-After>`.
- **404 and 410** on a saved host (`HOST_NOT_FOUND`, `HOST_GONE`) create a new host and retry once; elsewhere they exit 1. `404 PREVIEW_NOT_FOUND` names a preview, not the host, and never replaces the host.
- **Other 4xx** exit 1 with the hub's JSON `error` and `code` (`scserve: publishing: the serve hub answered 400: invalid entry (INVALID_ENTRY)`).
- **5xx**, and a hub that can't be reached, are retried after 1, 2 and 5 s, then exit 1 (`the serve hub answered 503 4 times: …`).

`--list` and `--stop` forget a refused or vanished host instead of creating one; a refused host still exits 3 there. A host is the hub's to delete after 90 days without a call or a socket and without an unexpired snapshot, so a machine that serves now and then gets a new host, and new URLs, then.

**From SLICC 6's tray hub.** Up to slicc-serve 1.3, `scserve` published through SLICC 6's tray hub (`www.sliccy.ai`, `tray.json`). Previews made that way keep working on six's hub, which still answers its own labels on `*.sliccy.now`, until they expire (a live one, until the relay that holds it stops); the new `scserve` doesn't list or stop them, and leaves `tray.json` alone. While a relay of an older `scserve` still runs for six's hub, a new public preview exits 1 with `scserve: a relay for https://www.sliccy.ai (pid N, likely an older scserve) still runs; its previews end when it stops (kill N)`; `--local` previews still go to it.

## State

| file | what |
| --- | --- |
| `<config>/host.json` (`0600`) | this machine's host on each serve hub it used (`hosts: [{hub, hostId, secret, createdAt}]`), and `host.lock` |
| `<state>/previews/<id>.json` (`0600`) | one record per preview: what `--json` prints, plus the relay's pid |
| `<state>/relay.json` (`0600`), `relay.lock`, `relay.log` | the running relay's pid, control port and token, and once a bridged preview is up its CDP port and secret |

`<config>` is `$XDG_CONFIG_HOME/slicc/serve` (default `~/.config/slicc/serve`), `%APPDATA%\slicc\serve` on Windows; `<state>` is `$XDG_STATE_HOME/slicc/serve` (default `~/.local/state/slicc/serve`), `%LOCALAPPDATA%\slicc\serve` on Windows. `SCSERVE_CONFIG_DIR` and `SCSERVE_STATE_DIR` replace them. Directories are `0700`; files are written to a temporary name and renamed.

## Opening

`--open` runs `open URL` on macOS, `xdg-open URL` (else `gio open URL`) on Linux and `explorer.exe URL` on Windows, with a fixed argv and no shell. An opener that is missing or fails prints `scserve: …` and leaves the exit status alone.

## `--upgrade`

A `scserve` from the installers downloads the latest `install.sh` (`install.ps1` on Windows) from `SLICC_INSTALL_BASE_URL` or the latest release, runs it with `--dir` set to the directory `scserve` is in, and prints `scserve: <old> → <new>` or `scserve: already up to date (<version>)`; a download that fails prints `scserve: could not download <url>: HTTP <status>` and exits 1. From npm (the launcher sets `SCSERVE_LAUNCHER=npm`), it prints `npm install -g @ai-ecoverse/slicc-serve@latest`; in SLICC, `pnpm add -g @ai-ecoverse/slicc-serve@latest`. On Windows the running `scserve.exe` is renamed to `scserve.exe.<random>.old` and replaced; the next run removes the old copy.

## The installers

`install.sh` (macOS, Linux) and `install.ps1` (Windows) download `SHA256SUMS` and the binary for the platform (`scserve-<darwin|linux>-<amd64|arm64>`, `scserve-windows-<amd64|arm64>.exe`) from `SLICC_INSTALL_BASE_URL` (the directory holding the release assets; default `https://github.com/ai-ecoverse/slicc-serve/releases/latest/download`, or `…/download/v<version>` with a version), refuse a checksum mismatch before anything is installed, run the binary's `--version` (which must equal `--version` when one is given), move it into place and print `scserve <version> installed to <path>`.

| `install.sh` | `install.ps1` | |
| --- | --- | --- |
| `--version VERSION` (or `=`; a leading `v` is dropped) | `-Version VERSION` | the release (default: the latest) |
| `--dir DIR` (or `=`) | `-Dir DIR` | where `scserve` goes (default `~/.local/bin`; `%LOCALAPPDATA%\slicc-serve\bin`, added to the user's `PATH`) |
| `--help` | `-Help` | the usage, exit 0 |

`install.sh` exits 0 when it installed, 1 on a failure (`install.sh: <what>`), 2 for a usage error; it needs `HOME` only for the default `--dir`. It uses `curl` (else `wget`) and `sha256sum` (else `shasum -a 256`), and never `sudo`. `install.ps1` run as a file exits 1 on failure; through `irm … | iex` it prints the error and returns without closing the shell.

## In SLICC

`serve.wasm` is built from the same crates for `wasm32-wasip1-threads`, with `serve-core` (arguments, the static server, the records) shared with the host build and `wasix-net`'s sockets on the kernel's loopback. The kernel runs it as `serve` or `scserve`, and it names itself after the command it runs as. The package's `engines` asks for `slicc-kernel >=1.153.0` (the kernel with `/dev/embedder`), so an older kernel refuses to install its commands; one without the device anyway prints `serve: /dev/embedder not available (needs slicc-kernel >= 1.153.0)` and exits 1 for everything that needs the page, while `--local`, `--version`, `--help`, `--upgrade` and the usage errors work.

The options, the output, `--json` and the exit status are the host's, with `serve` for `scserve` in every message. The differences:

- **Who publishes:** seven's page, which keeps a publisher host of its own on the serve hub and holds its socket. `serve` never talks to the hub: it asks the page with the [embedder kinds](#the-embedder-contract) below, and the page mints the preview and relays the hub's `http.request` streams to the kernel port.
- **Local URLs** are the page's names for the kernel's loopback: `http://<port>.kernel.localhost/` (from `SLICC_PAGE_LOOPBACK_URL`, else `http://127.0.0.1:<port>/`).
- **`--ttl`** collects the snapshot itself, with the host's walker and limits, and hands the files to the page in the body of one `serve.snapshot` request; the page mints the snapshot, uploads the files and finalizes it ([`serve.snapshot`](#servesnapshot)). Nothing keeps running. The kernel doesn't upload itself: a program's `https://www.sliccy.ai/…` request through the kernel's proxy answers `502` in seven (measured from a seven branch preview with the page's transport, [#4](https://github.com/ai-ecoverse/slicc-serve/issues/4)), while the page reaches the hub, since it already leads a tray there.
- **`--open`** asks the page for a tab with `open.url` (as [slicc-open](https://github.com/ai-ecoverse/slicc-open) does) once the URL is printed; a refusal prints `serve: <the page's message>` and leaves the exit status alone.
- **`--upgrade`** prints `pnpm add -g @ai-ecoverse/slicc-serve@latest`.

### What runs where in SLICC

```
serve DIR ──spawn──► serve (background, one per preview)
                       ├─ static server on the kernel's 127.0.0.1:<random>
                       └─ /dev/embedder: serve.publish, held open ──► seven's page ──► serve hub ◄── visitor
                                           page: http.request ──HTTP/1.1──► kernel 127.0.0.1:<port>
```

- **Detached:** `serve DIR` checks the directory and `--entry`, then starts a background `serve` (the same program, spawned with WASIX `proc_spawn`, standard input on `/dev/null`, standard error appended to `<state>/serve.log`, standard output on a pipe back to it) and waits. The background `serve` binds the static server, sends `serve.publish`, and writes one line back: the record, or what failed. The foreground `serve` prints it the way the host does and exits; the background one keeps the server and the request open. There is one background `serve` per preview and no relay: the page holds the host. Like `nohup`, the background `serve` ignores `SIGHUP` as it starts, and none of its standard streams is the terminal, so closing the terminal that ran `serve` (a hangup of its foreground group) leaves the preview up; `serve --stop`, `kill` (`SIGTERM`) or `kill -9` end it.
- **The request is the preview.** The background `serve` blocks reading the `serve.publish` reply. When that process ends, for whatever reason (`serve --stop`, `kill`, the kernel ending, a crash), the kernel aborts the request and the page revokes the preview on the hub. When the page ends the reply (the user stopped it in seven, the hub stopped it, seven lost its host), the background `serve` stops its server, removes its record and exits.
- **`--port PORT`** publishes a server that already runs on the kernel's loopback: the background `serve` holds only the request. `serve: nothing answers on 127.0.0.1:PORT yet` warns as on a host.
- **`--foreground`** does all of it in the `serve` process itself: it prints the URL, logs each request of the static server (`serve: GET /app.js 200 1234`), and Ctrl-C (or the end of the process) ends the request, which revokes the preview. When the page ends the reply, it prints `serve: stopped: <reason>` and exits 0.
- **`--local DIR`** starts a background `serve` with the static server only, recorded as `local` (listed, stopped with `--stop`); `--local --foreground DIR` serves in the foreground; `--local --port PORT` prints the URL only. None of them asks the page.
- **`--list`** asks the page with `serve.list` and joins its answer with the records, as the host joins the hub's list: `TOKEN MODE EXPIRES URL ROOT`, `(foreground)` for an attached one. A record whose process is gone is dropped (a snapshot's has no process and stays until it expires), and so is one whose token the page doesn't list (its process is stopped). When no page answers (`ENOTSUP`, or no `/dev/embedder`), the records alone are listed.
- **`--stop`** matches its argument against the records as on a host (token, public URL, local URL, port or record id). A public preview is stopped with `serve.stop`, which ends its background `serve` through the page; a token or a public URL that matches no record is sent to the page all the same. When the page refuses, the matching record's process gets `SIGTERM` anyway, and the page's message exits 1. A local preview's process gets `SIGTERM`. Either way the record is removed, and it prints `stopped <token or local URL>`. Several matching records stop none, as on a host.
- **State** is the host's layout in the kernel's file system: `<state>/previews/<id>.json` (`0600`) per preview, with the background process's pid, and `<state>/serve.log`, where `<state>` is `$SCSERVE_STATE_DIR`, else `$XDG_STATE_HOME/slicc/serve`, else `~/.local/state/slicc/serve`. There is no `host.json` and no relay: the page holds the host.

### Errors from the page

A request the page refuses fails with the kernel's errno, and `serve` prints `serve: <the first line of the page's message>` (the errno's text when there is none) and exits 1:

| errno | what it means | what `serve` adds |
| --- | --- | --- |
| `EACCES` | the user dismissed the prompt or didn't answer it, public previews are switched off, another request is waiting; or the kernel refused the request before the page saw it (`serve.publish is not allowed for this user`): the kinds are open to the session's own user, so a `serve` run under `sudo -u` someone else from another user's terminal is refused, while an agent in its own session works | `serve: if seven asked, answer in seven's tab (a click allows it), then run serve again` |
| `ENOTSUP` | no page answers for this session (`no page answers requests for this session`: a follower, a headless kernel), or a seven that doesn't answer the kind yet (`this embedder does not answer serve.publish`) | |
| `ENOENT` | `serve.stop`: the page holds no such preview | |
| `EBUSY`, `EIO`, any other | the page failed (`EIO` for an unreachable hub too) | |

A failure the serve hub answered comes as the reply's first line instead (`{"error", "status", …}`, below) and maps the way the host maps the hub's answers: `401` and `403`, or a `signIn`, exit 3 (`serve: publishing: the serve hub refused it (401: …)`, `serve: sign in at <url>`); `402` and `429` exit 4 (`serve: publishing: the serve hub's limit is reached (429: …)`, `serve: retry after 30 s`); any other status exits 1 (`serve: publishing: the serve hub answered 400: invalid entry (INVALID_ENTRY)`); the hub's `code`, when there is one, follows its error in parentheses. `--json` prints the failure as `{"ok":false,"exit":N,"lines":[…],"signIn":URL|null}`, as on a host.

### The embedder contract

This is what seven's page answers ([#6](https://github.com/ai-ecoverse/slicc-serve/issues/6)); `serve`'s tests run against a fake page that answers the same in a node kernel. Each request is one open of `/dev/embedder` (slicc-kernel 1.153.0 and later): a JSON header line `{"kind": …, …options}`, a body (`serve.snapshot` only), then the reply until end-of-file. Option values are JSON, so no path or URL can break the header.

| kind | options | opened | reply | errors |
| --- | --- | --- | --- | --- |
| `serve.publish` | `port`, `root`, `entry`, `bridge`, `maxTabs`, `quiet` | `O_RDWR` | one JSON line, `{"url","previewToken",…}` or `{"error",…}`; then held open until the preview ends, optionally one `{"stopped"}` line, then end-of-file | `EACCES`, `ENOTSUP`, `EIO` |
| `serve.list` | none | `O_RDWR` | one JSON line per preview, then end-of-file; nothing when there are none | `ENOTSUP`, `EIO` |
| `serve.stop` | `previewToken` | `O_WRONLY` | none; the close returns the errno | `ENOENT`, `ENOTSUP`, `EIO` |
| `serve.snapshot` | `root`, `entry`, `ttlMs`, `files`, `bytes` | `O_RDWR` | after the body: one JSON line, `{"url","previewToken","expiresAt",…}` or `{"error",…}`, then end-of-file | `EACCES`, `ENOTSUP`, `EIO` |
| `serve.logs` | `previewToken` (optional) | `O_RDWR` | one JSON line per lifecycle record, oldest first, then end-of-file | `ENOENT`, `ENOTSUP` |
| `serve.truncate` | `previewToken` (optional) | `O_RDWR` | one JSON line `{"cleared", "rearmed"}` | `ENOENT`, `ENOTSUP` |

#### `serve.publish`

```
{"kind":"serve.publish","port":32768,"root":"/home/me/site"}
{"kind":"serve.publish","port":5173,"entry":"app/index.html"}
```

| option | |
| --- | --- |
| `port` | number, 1 to 65535: the origin on the kernel's loopback, to which the page relays each visitor request |
| `root` | string: the directory's absolute path in the kernel, for the prompt and the chip only; absent with `--port`. The page never reads files: every visit goes to `port` |
| `entry` | string, absent without `--entry`: the `--entry` path as `serve` normalized it (segments joined by `/`; no empty, `.` or `..` segment; no leading `/`; no `\`, `:` or NUL). The page refuses anything else, and a `port` outside 1 to 65535, with an `EIO` error (`invalid entry`, `invalid port`) |
| `allowLive` | none any more: `serve` 1.3 and older sent `true` for `--port` and `false` for a directory, for six's edge cache; the serve hub caches nothing, and the page accepts the option and ignores it |
| `bridge` | `true` with `--bridge`, else absent: see [The bridge in seven](#the-bridge-in-seven) |
| `maxTabs` | number, 1 to 100, with `bridge` only (default 20) |
| `quiet` | `true` with `--quiet`, with `bridge` only |

What the page does, in order:

1. **Who may publish:** a follower, a page that runs no kernel, or a seven with no serve hub configured throws `NotSupportedError` (`ENOTSUP`, `seven publishes previews only through the serve hub, and it has none here`). A publisher socket that isn't connected right now throws an `EIO` error (`seven isn’t connected to the serve hub yet; try again in a moment`). On `*.sliccy.ai` the page uses the hub on its own origin; elsewhere the hub's origin comes from `localStorage['slicc-os.serve-hub']` (which replaces `slicc-os.tray-hub`). The switch "Public previews" off throws `NotAllowedError` (`EACCES`), `public previews are off in seven's settings`. The messages are bios's to choose; `serve` prints the first line as it is.
2. **The prompt:** unless the stamp says `serve` was typed in seven's terminal with a gesture still active (bios's `typed(stamp)`), the page asks: "Publish /home/me/site at a public URL?" (or "the server on port 5173"). Cancel, 60 s without an answer, or a second request of the same session while one is up, throws `NotAllowedError` with what happened (`the request to publish was dismissed`, `nobody answered the request to publish within 60 s`, `another request is waiting for you`); `serve` prints the message as it is.
3. **The mint:** `POST /api/serve/host/<hostId>/previews` `{entry, tag}` with the page's host secret, where `entry` is `/<entry>` with each segment percent-encoded, or `/` without `entry`, and `tag` is `/srv/serve/<id>` with a fresh random hex `<id>` of at least 16 digits from `crypto.getRandomValues` for each request (bios uses 32; the host relay 16), never the real path and never the port (ports are reused), so the page can tell its leftovers on the hub after a reload.
4. **A failure the hub answered** (the mint's HTTP status wasn't 2xx) is the reply, not an errno, so that `serve` can exit 3 or 4: one line `{"error": <the hub's error, or the status text>, "status": <its HTTP status>, "code": <its code, when it has one>, "retryAfter": <its Retry-After, when present>, "signIn": <a sign-in URL, when the hub named one>, "active", "limit": <the hub's counts, when it sent them (`LIVE_PREVIEW_LIMIT`)>}`, then end-of-file. A hub the page can't reach at all is an error (`EIO`) with the reason.
5. **Registered:** the page keeps `previewToken → {port, tag, previewToken, url, root, entry, since, uid: stamp.uid}` and answers `{"url": …, "previewToken": …}` and a newline. The `url` is the hub's (`https://<previewToken>.sliccy.now/` or with the entry); `serve` drops a trailing `index.html` when there is no `entry`, for older pages.
6. **Held open:** the page's reply stream stays open, with nothing more in it, as long as the preview is live. The page shows the live chip ("Serving publicly … Stop").

How it ends:

- **From the command's side:** the kernel aborts the handler's `signal` (reason `closed` when the description is closed or its process ends, `interrupted` when a signal reaches the blocked read) and cancels the reply stream. On either, the page stops the preview on the hub (`DELETE /api/serve/host/<hostId>/previews/<previewToken>`) and drops it from the registry. If the signal is already aborted when the mint answers (the process went away during the prompt or the mint), the page revokes at once and replies nothing.
- **From the page's side** (the chip's Stop, `serve.stop`, `preview.revoked` from the hub, the switch turned off, the host lost): the page stops it on the hub unless the hub stopped it, drops it, writes `{"stopped": <reason>}` and a newline (`stopped in seven`, `stopped with serve --stop`, `revoked by the serve hub`, `public previews were switched off`, `seven lost its serve host` (the host was deleted, or another tab took its socket)), and closes the stream. Every page-side end writes the line, the chip's Stop and `serve.stop` included; `serve` treats end-of-file without it as `stopped in seven`.
- **A reload** ends the kernel, and every process with it: a live preview stays minted on the hub while nobody serves it (visitors get `502`), so on boot, and after each reconnect of its publisher socket, the page stops the host's live previews whose `tag` it holds no registration for, as the host relay does.

The relay itself follows [The relay's answers](#the-relays-answers): for each `http.request` of a preview the page holds, the page relays the request to the kernel port as HTTP/1.1 (the visitor's method, path and query, every header including `Cookie`, `Host: 127.0.0.1:<port>`, and the streamed body, acknowledged with `http.ack` as the origin reads it), and the answer's status, headers (every `Set-Cookie` kept, a `Location` on the origin rewritten to its path) and streamed body back as `http.response`, binary frames and `http.response.end`, within the hub's window; `http.cancel` closes the connection to the origin, and an origin that fails before its answer is `502`. A `http.request` for a preview the page doesn't hold is `404`.

#### `serve.list`

```
{"kind":"serve.list"}
```

The reply is one JSON object per line for each preview the page holds from `serve.publish` or `serve.snapshot` that the caller may see: those whose publishing process had the caller's uid, all of them for root. `{"previewToken", "url", "root", "port", "entry", "since"}` for a live preview, with `root` and `entry` absent as in the request and `since` the mint's time in milliseconds since the epoch, which `serve --list` shows as `createdAt` (ISO 8601) when it has no record of its own. A snapshot's line has `"mode": "persistent"` and `"expiresAt"` (milliseconds since the epoch) and no `port`; a live one may say `"mode": "live"`. An empty reply means none.

#### `serve.stop`

```
{"kind":"serve.stop","previewToken":"0f3c….a91e…"}
```

The page revokes the preview on the hub (a snapshot too, before its TTL), drops it, and ends a live preview's `serve.publish` reply with `{"stopped":"stopped with serve --stop"}`, so the background `serve` that holds it exits. A token the page doesn't hold is stopped on the page's host all the same; one the hub has no preview for (`404 PREVIEW_NOT_FOUND`) throws `NotFoundError` (`ENOENT`), `the serve hub has no such preview`. A held preview of another uid, for a caller that isn't root, is `ENOENT` too.

#### `serve.snapshot`

```
{"kind":"serve.snapshot","root":"/home/me/site","ttlMs":604800000,"files":2,"bytes":1843}
{"path":"index.html","type":"text/html; charset=utf-8","size":1800}
<1800 bytes>
{"path":"app/main.js","type":"text/javascript; charset=utf-8","size":43}
<43 bytes>
```

`serve --ttl` walks the directory first, as on a host ([Snapshots](#snapshots---ttl)): the limits, the entry file and the names are checked before anything is sent. Then it opens the device `O_RDWR`, writes the header, writes every file as the body, and reads the reply: the kernel ends the body at the first read, so the reply comes once the page has read the whole body.

| option | |
| --- | --- |
| `root` | string: the directory's absolute path, for the prompt only |
| `entry` | string, absent without `--entry`: normalized as for `serve.publish` |
| `ttlMs` | number: the TTL in milliseconds, 60000 to 2592000000 (30 days) |
| `files`, `bytes` | numbers: how many files and bytes the body holds, for the prompt and to check the body against |

**The body** is the files in order, each a JSON line `{"path", "type", "size"}` (at most 4 KiB) followed by exactly `size` bytes. `path` is the file's path in the snapshot, segments joined by `/`, as the host uploads it: no leading `/`, no empty, `.` or `..` segment, no segment starting with `.` (but `.well-known`), no `\`, `:` or NUL. `type` is the file's type from its extension, `size` at most 25 MiB. The body ends after the last file.

What the page does, in order:

1. **Who may publish, and the prompt:** as for `serve.publish`, before it reads the body: a follower or a seven without a serve hub is `ENOTSUP`, the switch "Public previews" off is `EACCES`, and unless `serve` was typed in seven's terminal with a gesture still active, it asks: "Publish a snapshot of /home/me/site (2 files, 1.8 KiB) at a public URL for 7 days?". Cancel or no answer is `EACCES`, with its message.
2. **The mint:** `POST /api/serve/host/<hostId>/previews` `{entry, tag, ttlMs}`, with `entry` and `tag` made as for `serve.publish`.
3. **The uploads:** for each file of the body, `PUT /api/serve/host/<hostId>/previews/<previewToken>/files/<path>` (each segment percent-encoded) with the host's secret as the bearer, `Content-Length`, `Content-Type: <type>` and the bytes; the page reaches the API on its own origin (`/7/api/serve/…` on `www.sliccy.ai`, `/api/serve/…` on `seven.sliccy.ai` and branch hosts). The page may upload a few at a time, and may report progress with `request.progress` (`uploaded 12 of 40 files`), which `serve` shows on its standard error.
4. **The finalize:** `POST /api/serve/host/<hostId>/previews/<previewToken>/finalize`, once the body has ended. The TTL counts from here.
5. **The reply:** one line `{"url": <the hub's url>, "previewToken", "expiresAt": <milliseconds since the epoch>, "files", "bytes"}` and a newline, then end-of-file. The page keeps the snapshot for `serve.list` and `serve.stop` (with `stamp.uid`, `root`, `entry`, `since`, `expiresAt`) until it expires or is stopped, across reloads, since a snapshot outlives the page.

When it fails, the page answers at once, without reading the rest of the body: with an error, or with the failure line as a string (never a stream). The kernel then drops whatever `serve` still writes ([slicc-kernel#1536](https://github.com/ai-ecoverse/slicc-kernel/issues/1536)), so `serve` never waits on a body nobody reads; it gets the errno from its next write or from its read, and the failure line from its read.


- **The body is wrong** (a line that isn't the JSON above, a path or type that breaks the rules, a `size` over 25 MiB, a body that ends inside a file, more than 1,000 files or 50 MiB, the same path twice, or files and bytes that don't add up to exactly `files` and `bytes` when the body ends, so what is finalized is what the prompt showed): the page stops the snapshot on the hub (`DELETE …/previews/<previewToken>`) and throws an `EIO` error `bad snapshot body: <what>`.
- **The hub answered a failure** to the mint, an upload or the finalize: the page stops a minted snapshot, and the reply is the failure line of `serve.publish` (`{"error", "status", "code"?, "retryAfter"?, "signIn"?}`), with `"during": "mint"`, `"upload"` or `"finalize"`, and for an upload `"path"`. `serve` says `publishing the snapshot` for the mint and `uploading the snapshot` otherwise, adds ` (<path>)` to the hub's error for an upload, and maps the status to exits 1, 3 and 4 as for `serve.publish`. The hub's own limits (`413` `FILE_TOO_LARGE`, `TOO_MANY_FILES`, `SNAPSHOT_TOO_LARGE`) and `400` `ENTRY_MISSING` are exit 1.
- **The hub can't be reached:** the page stops a minted snapshot if it can, and throws an `EIO` error with the reason.
- **`serve` goes away** before the reply (Ctrl-C, `kill`): the kernel aborts the request, and the page stops the snapshot on the hub unless it had already written its reply, a finalize still in flight included, since `serve` never got the URL. A snapshot whose reply the page had written stays (`serve` may have exited before reading it), and `serve --list` shows it from `serve.list`.

### The bridge in seven

`serve --bridge DIR` in seven is the same preview, with seven's page as the leader that the hub's `bridge.*` messages reach. `serve` sends `bridge`, `maxTabs` and `quiet` with `serve.publish`, and the options and output are the host's, except that the targets show up on seven's own CDP endpoint instead of a local one: `serve: its visitor tabs are CDP targets preview:<token>:* on $SLICC_CDP_URL`. What the page does:

1. **The mint** adds `bridge: true`, `maxTabs` and `quiet` to the mint. The prompt says so: "Publish /home/me/app at a public URL, and let the agent drive the tabs of whoever opens it?" The reply's first line adds `"webhook": "serve-<16 hex>"`, the page's own name for the agent's webhook of this preview (a fresh one for each, matching seven's webhook names; the hub knows no webhooks). A reply without it comes from a seven that doesn't know the bridge and published without one: `serve` then ends the request, so the page revokes the preview, and exits 1 with `serve: this seven publishes without the bridge, so serve stopped the preview again; --bridge needs a newer slicc-bios`.
2. **Visitor tabs as targets.** For each `bridge.connected` of a preview the page holds, the page adds a target `preview:<token>:<connId>` (`type: "page"`, the preview's URL and the connection's origin, as on a host) to the `Target` domain its CDP hook emulates for kernel programs (`SLICC_CDP_URL`), and drops it on `bridge.disconnected` or when the preview stops. It answers in a session exactly as the host's relay does ([The bridge](#the-bridge---bridge)): the same synthetic methods and navigation events, everything else as `bridge.cdp.request` over the page's publisher socket, the same errors and the same 30 s timeout; `Target.closeTarget` sends `bridge.close`. The targets pass the same guard as seven's other targets: a connection seven wouldn't let drive a tab doesn't see them, and whatever seven asks before an agent drives a tab, it asks here. `bridge.connected` with `replay: true` restores a target without recording or announcing it. A `bridge.cdp.response` for a `connId` the page doesn't know is dropped.
3. **Emits** arrive on the publisher socket as `bridge.emit {previewToken, connId, name, detail, at}`, and the page delivers each to the agent as a delivery of its `serve-…` webhook, with the headers `x-slicc-preview-conn` and `x-slicc-preview-token` and the body `{name, detail}`. The page delivers them only while it holds the preview, and drops them after it stopped, and drops an emit that doesn't come from a tab connected to the preview right now (the hub's beacon emits, `connId: null`).
4. **The first visit** of a preview that isn't `quiet` is delivered to the agent on the same webhook, as a delivery with the header `x-slicc-preview-lifecycle: connected`, the headers above and the body `{"origin", "userAgent", "connectedAt"}`; then the page sends `preview.state.update {previewToken, announced: true}`. The page restores `quiet` and `announced` from the hub's `preview.state` when it connects.
5. **Lifecycle records:** the page keeps the last 500 records of the previews it holds from `serve.publish`, in memory, with the host's fields (`at`, `event`, `previewToken`, `connId`, `announced`, `origin`, `userAgent`, `connectedAt`, `reason`, `name`, `detail` cut to 1,000 characters). `serve.logs` answers them as JSON lines, of one preview or of all, filtered by uid as `serve.list` is (a `previewToken` the caller can't see is `ENOENT`, `seven holds no preview <token>`); `serve.truncate` clears them and re-arms the announcement (`preview.state.update {announced: false}`), and answers `{"cleared", "rearmed"}`. `serve --logs` and `serve --truncate` print them as on a host.
6. **`--foreground`** gets each event as a line of the held `serve.publish` reply after the first, `{"event": "connected"|"disconnected"|"emit", …}` with the record's fields, and prints it as a host `--foreground` does; a detached `serve`'s reply gets the same lines and ignores them. A `serve` built before the bridge reads only `stopped` from those lines, and ignores the rest.

## Development

```sh
npm install
npm run lint
npm test
npm run build:wasm
```

- `crates/serve` is the binary (`scserve`, and `scserve.wasm` for the kernel); `crates/serve-core` has no host-only dependency and builds for `wasm32-wasip1-threads`; `crates/serve-hub` (native only) is the hub client, the host, the relay and the CLI.
- `npm run lint`: Biome, the no-comment check, `slicc-lint`, `shellcheck` and `dash -n` on `install.sh`, `cargo fmt`, and `cargo clippy` for the host and `wasm32-wasip1-threads`.
- `npm test` builds `scserve` and runs `test/integration/` with Node's test runner. The hub there is `fake-hub.mjs`, the serve hub's protocol as sliccy-ai's `test/integration/publisher.mjs` shows it from the publisher's side: hosts and their bearer (`401`, `404 HOST_NOT_FOUND`, `410 HOST_GONE`), the publisher WebSocket (`hello`, `ping`/`pong`, `4000` for a socket replaced, `4001` for a host deleted, `preview.revoked`), mint, list and stop, snapshot upload, finalize and serving, a visitor path that resolves `<previewToken>.localhost` hosts and relays each request as `http.request`, binary body frames both ways and `http.ack` within the 1 MiB window (it can hold its acks, to check the relay waits), `http.cancel` when the visitor goes away, the bridge's tab sockets, beacon emits and bootstrap injection, and injected answers per route for every class of error. `public.test.mjs` checks the full HTTP relay through it: a POST of 3 MiB with a query string, cookies and custom headers, a chunked upload, redirects on the origin rewritten and elsewhere passed on, a `404` with its own headers, two `Set-Cookie` lines, a chunked answer, a 30 MiB file held to the window, and a server-sent event stream the visitor leaves. Tests never reach `seven.sliccy.ai`. `seven.test.mjs` installs the package (its staged `package.json`, from `scripts/targets.mjs`) with `bin/serve.wasm` (after `npm run build:wasm`), wasm-bash and wasm-coreutils in a node kernel from `@ai-ecoverse/slicc-kernel` (Node 26), behind a fake page that answers `serve.publish`, `serve.list`, `serve.stop` and `open.url` and records each request: the background `serve`, the page ending a preview, a killed holder revoking, `--port`, `--foreground`, `--open`, `--list` and `--stop` by token, URL and port, each errno and each class of hub failure, and a hostile directory name in the header; then a kernel no page answers, and slicc-kernel 1.152.2 (the devDependency `slicc-kernel-without-embedder`), which refuses the package and, with `engines` stripped, has no `/dev/embedder`. `seven-ttl.test.mjs` does the same for `--ttl`, with a fake page that implements `serve.snapshot` against `fake-hub.mjs` (it reads the body, mints, uploads, finalizes, and stops what it minted on a failure): the files that reach the hub, `--json`, `--entry`, `--open`, a file past the request window, the limits checked before the page is asked, each hub failure (mint, upload, finalize, with a 3 MiB body answered early), the page's refusals, and a `serve` killed before the finalize. `browser.test.mjs` opens the preview in a real Chrome for Testing (a pinned version, installed with `@ai-ecoverse/slicc-cdp`'s `installBrowser` into `node_modules/.cache/browsers` and launched headless with `launchBrowser`), where the fake hub injects slicc 6's real bridge bootstrap (`@ai-ecoverse/cherry`'s `preview-bridge.js`, the bundle slicc's worker serves); `playwright-cli` then snapshots, clicks, fills, types and presses Enter in it through `scserve --bridge`, and the test reads the page's own state to check each happened exactly once, and that `Runtime.callFunctionOn` on a handle doesn't run the handle's expression again. `slicc-cdp.test.mjs` downloads `playwright-cli` and `curlwright` for the platform from a pinned [slicc-cdp](https://github.com/ai-ecoverse/slicc-cdp) release (checked against its `SHA256SUMS`, cached in `node_modules/.cache/`) and runs them against a visitor tab (`visitor.mjs`, which answers as the hub's bootstrap does, evaluating in a `node:vm` context with a stand-in `document`): `tab-list`, `eval`, `snapshot`, `screenshot`, `goto`, a `curlwright` request, and a relay reconnect that keeps one tab's target id and drops a tab that closed meanwhile. `bridge.test.mjs` plays the visitor's tab against the fake hub's `/__slicc/bridge` (`hub.tab(url)`, answering `cdp.req` frames and sending emits) and drives the relay's CDP endpoint with Node's `WebSocket`: the guards (secret, `Host`, `Origin`), `/json/*`, discovery, attach and auto-attach, the synthetic and forwarded methods, navigation events, a tab that goes away mid-call, `closeTarget`, `--stop`, `--max-tabs`, `--quiet`, the first-visit latch through `preview.state.update`, `--logs`, `--truncate`, and an emit full of escape sequences. `install.test.mjs` runs `install.sh` and `--upgrade` against local release directories.
- `npm run test:unit` runs the unit tests in the gitignored `test/unit/` with `cargo llvm-cov`; the pre-commit hook wants 100% coverage of the changed lines in the crates it measures.

CI is `.github/workflows/ci.yml` on [slicc-shared-web's pipeline](https://github.com/ai-ecoverse/slicc-shared-web): lint, the integration tests on Linux, `npm run test:windows` on Windows, the same tests on macOS, `cargo check` for every release target, and the reviews. On a push to `main`, semantic-release tags a release; then the binaries are built for every target (the darwin ones signed and notarized), collected with `serve.wasm` into `SHA256SUMS` (attested), attached to the release with both installers, installed once through `install.sh`, and published to npm as `@ai-ecoverse/slicc-serve` and its six platform packages.

## License

Apache-2.0
