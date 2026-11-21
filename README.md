# slicc-serve

`scserve` publishes a directory, or a server that already runs on this machine, at a public capability URL on `sliccy.now`, through the SLICC tray hub. It is slicc 6's `serve` for any host: `scserve ./dist` prints `https://<label>.sliccy.now/` and returns, and anyone with the URL sees the files, pulled from this machine on demand while it is online. `scserve --port 5173` does the same for vite, `python -m http.server` or any other local server, and `scserve --ttl 7d ./dist` uploads an immutable snapshot that stays up without this machine.

In SLICC the same program is `serve` and `scserve` (`bin/serve.wasm`). There it serves the directory on the kernel's loopback, and seven's page, which already leads a tray on the same hub, publishes it: `serve` asks the page over `/dev/embedder` (see [In SLICC](#in-slicc)).

This README is the spec. The plan is [#1](https://github.com/ai-ecoverse/slicc-serve/issues/1).

## Install

```sh
curl -fsSL https://github.com/ai-ecoverse/slicc-serve/releases/latest/download/install.sh | sh
```

On Windows, in PowerShell:

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
| `--local` | stays on this machine: no tray hub, prints `http://127.0.0.1:PORT/` only |
| `--foreground` | stays attached instead of returning; Ctrl-C (or the end of the process) stops the preview |
| `--open` | opens the URL with the platform's opener once it is published (and a sign-in page, see [The tray hub](#the-tray-hub)) |
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

**0** ok, **1** failure (a missing directory, the hub's error, an unreachable hub), **2** usage error (`scserve: <what is wrong>` and the usage on stderr), **3** the tray hub wants a sign-in or refused the request, **4** a quota of the tray hub is used up. With `--json`, a failure also prints `{"ok":false,"exit":N,"lines":[…],"signIn":URL|null}` to stdout.

### Output

On success, stdout is the URL alone (so `url=$(scserve dist)` works), and stderr says what it means:

```
$ scserve ./site
https://0f3c…--a91e….sliccy.now/
scserve: /home/me/site is public at https://0f3c…--a91e….sliccy.now/ (here: http://127.0.0.1:52811/)
scserve: stop it with: scserve --stop 0f3c….a91e…
```

`--json` prints the record instead: `mode` (`live`, `persistent` or `local`), `previewToken`, `url`, `localUrl`, `root`, `port`, `entry`, `trayId`, `createdAt`, `expiresAt`, `foreground`, and for a snapshot `files` and `bytes`.

Without `--entry`, the URL ends in `/` and opens what the server answers there: `index.html`, else a directory listing (the hub's `entryPath` is still `…/index.html`; the relay maps it to `/`). With `--entry`, the URL names the entry.

## What runs where

```
scserve DIR ──spawn──► relay (scserve __relay, detached, one per user)
                         ├─ static server on 127.0.0.1:<random> per directory
                         └─ controller WebSocket of this machine's tray ◄── tray hub ◄── visitor
                              preview.request ──GET──► 127.0.0.1:<port> ──► preview.response chunks
```

- **The relay.** The hub lets one leader socket per tray, so one process holds it for every preview: `scserve` starts `scserve __relay` detached (its own session on Unix, `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` on Windows, with the caller's standard handles not inherited), hands it the preview over a loopback control socket, prints the answer and exits. The relay runs the static servers, holds the tray's controller WebSocket, answers the hub's `preview.request` with an HTTP GET to the preview's origin, and exits 5 s after its last preview stops. Its control socket is on `127.0.0.1` with a random port and a 48-hex-digit token in `relay.json` (mode `0600`); a request without the token is dropped. A lock (`relay.lock`) keeps it to one relay per state directory; its stderr goes to `relay.log`.
- **`--foreground`** keeps the control connection open instead of returning: the relay streams one line per visit (`scserve: GET /app.js 200 1234`) to stderr, and when the connection ends (Ctrl-C, the process killed, the terminal closed) it revokes the preview on the hub and stops its server. A `--stop` from elsewhere ends the foreground `scserve` with `scserve: stopped: <reason>` and exit 0.
- **`--local`** never talks to the hub. `--local DIR` runs the static server in the relay (listed, stopped with `--stop <local URL>`); `--local --foreground DIR` runs it in the `scserve` process itself and logs each request; `--local --port PORT` only prints `http://127.0.0.1:PORT/[ENTRY]` (with `scserve: nothing answers on 127.0.0.1:PORT yet` when nothing does).
- **`--port`** publishes an existing server: the relay forwards each visit's path to `127.0.0.1:PORT` (and warns the same way when nothing answers yet). It is published with `allowLive: true`, so the hub doesn't cache it; a directory is published with `allowLive: false` (the hub's 5 s edge cache).
- **`--ttl`** needs nothing running: `scserve` itself walks the directory, mints the snapshot, uploads the files and finalizes it, then exits.

### The static server

The built-in server answers `GET` and `HEAD` (others `405` with `Allow: GET, HEAD`) on `127.0.0.1` only, as the user who ran `scserve`, so file permissions hold.

- **Paths** are percent-decoded once and split on `/`. A segment that is `.` or `..`, or holds `\`, `:` or NUL, or a path that doesn't decode to UTF-8, is `400`. A segment starting with `.` (`.env`, `.git`) is hidden: `404`, never listed, never snapshotted; `.well-known` is the exception. The resolved file must lie inside the directory after symlinks are resolved, with no hidden segment in its resolved path; a symlink out of it, or to a hidden file or directory (`public.txt -> .env`), is `404`, and `--entry` naming either is `entry file not found`. Names that aren't UTF-8 are skipped.
- **Directories:** a path without its trailing `/` is `301` to the same path with it (query kept); with it, `index.html` if there is one, else an HTML listing of the visible entries.
- **Files** get `Content-Type` from the extension (`text/html; charset=utf-8`, `text/css`, `text/javascript`, `application/json`, `image/*`, `font/*`, `video/*`, `audio/*`, `application/wasm`, …; unknown is `application/octet-stream`), `Content-Length`, `Last-Modified`, `Accept-Ranges: bytes`, `Cache-Control: no-cache` and `X-Content-Type-Options: nosniff`.
- **Ranges:** one `bytes=a-b`, `bytes=a-` or `bytes=-n` range is `206` with `Content-Range`; one outside the file is `416` with `Content-Range: bytes */size`; several ranges, another unit or an `If-Range` get the whole file (`200`), as slicc 6's `parseByteRange` does.
- HTTP/1.1 keep-alive, a 16 KiB limit on the request head (`431`), and a 30 s idle timeout per connection.

### The relay's answers

The hub sends `preview.request {reqId, servedRoot, vfsPath, asText, range?}`; the relay answers with `preview.response` messages, exactly as slicc 6's leader does:

- `vfsPath` must lie inside `servedRoot` (no `.`/`..` segments, no `%2e`, `servedRoot` never `/`), else `403`; a `servedRoot` the relay doesn't serve (any more) is `404`.
- The path under `servedRoot` goes to the origin as it came (still percent-encoded); the query string is not part of the hub's request. The entry path of a preview without `--entry` goes to `/`. Same-origin redirects of the origin (`301`, `302`, `303`, `307`, `308` to `127.0.0.1:PORT` or `localhost:PORT`) are followed, at most 5; others are passed on as their status.
- `200`: the body (at most 25 MiB; more, or a larger `Content-Length`, is `413` `preview file exceeds 25 MiB limit`) in chunks of 64 KiB, as UTF-8 when the hub asked for text and the body is valid UTF-8, else base64, each chunk with `mime` (the origin's `Content-Type`, else from the extension), `chunkIndex`, `totalChunks`, `status: 200` and `size`.
- `206`: the window, cut to 8 MiB (media players ask for the next one), always base64, with `status: 206`, `size` (the whole entity) and `range: {start, end}`. `416` passes `size` from `Content-Range: bytes */size`.
- Any other status is `ok: false` with that status and the first 4 KiB of the body as `reason`. An origin that doesn't answer is `502` `nothing answers on 127.0.0.1:PORT: …`.
- `preview.revoked` stops the preview locally (its server, its record, a foreground holder). Other messages are ignored. The relay sends `{"type":"ping"}` every 30 s, because the hub counts a leader without application messages for 2 minutes as gone.
- **When the socket drops**, the relay claims the tray again and reconnects after 1, 2, 5, 10, then every 30 s, while it has public previews. If the tray is gone or refused, its live previews are stopped. When it (re)connects, it lists the tray's previews and revokes live ones it doesn't serve, left over from a relay that died.

### Snapshots (`--ttl`)

`scserve --ttl 7d DIR` collects the visible files (hidden ones, symlinks out of the directory and symlinks to hidden files excluded, sorted). A directory reachable by several paths, its own and a symlink's (`latest -> v2`), is collected under each of them, as the live preview serves it; only a symlink back to a directory the walk is inside (`v2/up -> ..`) is skipped, so links can't loop, and the limits below, plus 10,000 directories walked, bound what repeats. It refuses more than 1,000 files, a file over 25 MiB or more than 50 MiB in total before anything is sent, and needs its entry file (`index.html` by default; `scserve: DIR has no index.html; a --ttl snapshot needs its entry file (give one with --entry)`). It mints with `ttlMs`, uploads the files four at a time (`PUT …/preview/<token>/file?path=<path>` with the upload token and each file's type), finalizes, and prints the URL. If an upload or the finalize fails, it stops the snapshot on the hub before it exits. A snapshot counts toward the tray's 10-snapshot quota (`429 PREVIEW_LIMIT`, exit 4), lasts until its TTL, and is served by the hub without this machine. Subdirectory index pages aren't reachable by their directory URL in a snapshot (the hub serves stored paths only); link to `sub/index.html`.

### `--list` and `--stop`

`--list` asks the hub for the previews of this machine's tray (and of [retired trays](#state) that still hold snapshots), joins them with the local records, and prints `TOKEN MODE EXPIRES URL ROOT`: `live`, `snapshot`, `uploading` or `local`, `(foreground)` for an attached one. Records whose relay is gone, snapshots past their expiry and previews the hub no longer has are dropped. `no previews` when there are none.

`--stop` takes a preview token, a public URL (its host encodes the token, as slicc 6's `previewTokenFromUrl` reads it), a local URL, a port or a record id. A local URL matches by its port when its host is a loopback name (`127.0.0.1`, any `127.x.y.z`, `localhost`, `*.localhost`), so `http://localhost:61489/` and `61489` both stop the preview served on `127.0.0.1:61489`; a port matches a preview's static server or the server a `--port` preview publishes. When several previews match, it stops none and exits 1 with `scserve: X matches N previews; stop one by its token`. A hub preview is stopped with the tray that minted it (`POST /api/tray/<trayId>/preview/stop`); the relay then gets `preview.revoked` and stops its server. A local preview is stopped through the relay. It prints `stopped <token or URL>`; `scserve: no preview matches X`, `scserve: <token> is not a preview of this machine's trays` and `scserve: the tray hub has no preview <token> (already stopped or expired)` exit 1.

## The bridge (`--bridge`)

`scserve --bridge ./app` is slicc 6's `serve --bridge` on a host: every tab a visitor opens on the public URL becomes a CDP target on this machine, `preview:<token>:<connId>`, that a CDP client (playwright-cli, a script with `chrome-remote-interface`) can evaluate in, query, click, type into, screenshot and navigate, and the page's `window.slicc.emit(name, detail)` calls arrive here. Nothing on the hub changes: the hub already injects the bridge's bootstrap into a bridged preview's HTML (`<script src="/__slicc/preview-bridge.js">`), holds one `/__slicc/bridge` WebSocket per visitor tab, and relays between those sockets and the tray's leader. `scserve` is that leader.

```
visitor tab ──wss /__slicc/bridge──► tray hub ──controller WS──► relay ◄──ws──── CDP client
  (bootstrap answers cdp.req)          bridge.connected / .disconnected          http://127.0.0.1:<port>/<secret>/
                                       bridge.cdp.request / .response / .close
                                       webhook.event (emits)
```

**The mint** of a bridged preview adds `bridge: true`, `maxTabs` (`--max-tabs`, default 20), `quiet` (`--quiet`) and `webhookId: "scserve-<record id>"` to `POST /api/tray/<id>/preview`. The hub routes the preview's emits to the leader as `webhook.event` with that `webhookId`, which is all the id is for: the hub doesn't look it up.

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

**The lifecycle records.** The relay keeps the last 500 records of all its bridged previews, in memory (they are gone when the relay exits): `connected` (with the visitor's `origin`, `userAgent` and `connectedAt`), `disconnected` (with the `reason` when the hub gives one) and `emit` (the `name` and the `detail`, as JSON, cut to 1,000 characters). An emit, like a visitor's `origin`, `userAgent` and `reason`, is untrusted text from a stranger's browser; the hub already limits each emit to 16 KiB and 20 per tab every 10 s. Every line `scserve` prints for a person (`--foreground`'s event lines, `--logs`, the first-visit line) writes those fields with control characters (C0, DEL and C1) escaped as JSON escapes them (`\u001b`), so a visitor can't send escape sequences to the terminal; `--json` output is JSON already.

**The first visit.** The first tab that connects to a preview is announced, unless the preview is `--quiet`: its record is `announced` (the others `suppressed`), a `--foreground` `scserve` prints `scserve: first visitor on <url>: <origin> (<userAgent>)`, and the relay sends `preview.state.update {previewToken, announced: true}`, so the hub keeps the latch across relay restarts (it replays `preview.state {previewToken, quiet, announced}` to a leader that connects, with `bridge.connected {…, replay: true}` for tabs already there, which are neither recorded nor announced again).

**Where events go:**

- A `--foreground` `scserve --bridge` prints one line per event to stderr, `scserve: connected <connId> <origin> <userAgent>`, `scserve: disconnected <connId> <reason>`, `scserve: emit <connId> <name> <detail>`; with `--json`, stdout has one JSON object per event after the record: `{"event": "connected"|"disconnected"|"emit", "previewToken", "connId", "at", …}`, the record's fields.
- `scserve --logs [TOKEN|URL] [--lines N]` asks the relay for them: `Preview lifecycle records (oldest to newest):`, then `<at>  <event>  <announced|suppressed|->  preview=<token> conn=<connId> <origin=… userAgent=…|reason=…|name=… detail=…>` per record, or `no lifecycle records (the relay keeps them in memory, so they are gone when it restarts)`; `--json` prints the array. A target that matches no bridged preview is `scserve: no preview matches X` (exit 1); no relay running is no records.
- `scserve --truncate [TOKEN|URL]` clears them (of that preview, or all), re-arms the first-visit announcement of each matching bridged preview (`preview.state.update {announced: false}`), and prints `cleared <n> records, re-armed <m> previews` (`--json`: `{"cleared": n, "rearmed": m}`).

## The tray hub

`scserve` talks to the tray hub at `https://www.sliccy.ai` (`SLICC_TRAY_HUB` replaces it, for tests and staging) with the same API as slicc 6 and seven. Nothing on the hub changes for it:

| call | what for |
| --- | --- |
| `POST /tray` `{}` | a tray of this machine's own, created once and kept in `tray.json` |
| `POST /controller/<token>` `{controllerId, leaderKey?, runtime: "slicc-serve/1"}` | claims the tray's leadership; the answer's `leaderKey` is saved |
| `GET /controller/<token>?controllerId=…&leaderKey=…` (WebSocket) | the controller socket, on the hub's origin as `ws:`/`wss:` |
| `POST /api/tray/<id>/preview` `{servedRoot, entryPath, allowLive, ttlMs?}` | mints a preview: `servedRoot` is `/srv/serve/<16 hex>`, opaque to the hub, and never the real path |
| `PUT /api/tray/<id>/preview/<token>/file?path=` and `POST …/finalize` | a snapshot's files, with the upload token |
| `POST /api/tray/<id>/preview/stop` `{previewToken}`, `GET /api/tray/<id>/previews` | `--stop`, `--list` and the relay's tidying |

The controller token goes in `Authorization: Bearer` to the hub's origin only. Anonymous trays are what the hub offers today; `scserve` is ready for the hub to gate them, and maps every answer of every call above, and of the WebSocket upgrade (any answer but `101`), the same way:

- **3xx** is never followed blindly. A `Location` on another origin (an auth page), or on the hub's own origin outside the API (`/tray`, `/api/…`, `/controller/…`), exits 3 with `scserve: the tray hub wants you to sign in: <Location>`; with `--open`, or when you answer `y` on a terminal, the page opens. A same-origin redirect to an API path is followed, at most 5 times (a sixth exits 1, `redirected more than 5 times`), keeping method and body for `307`/`308` and switching to `GET` for `303` (and `301`/`302` after a `POST`); the bearer is only ever sent to the hub's origin.
- **401 and 403** exit 3 with the hub's `error` and `code`, and `scserve: sign in at <url>` from `Link: <url>; rel="login"` or the first URL in `WWW-Authenticate` (else the header itself). On a **saved** tray, a refusal drops the tray, creates a new one and retries once; a second refusal exits 3.
- **402 and 429** exit 4 with the hub's error and `scserve: retry after <Retry-After>`. The hub's quota codes name the remedy: `LIVE_PREVIEW_LIMIT` (`… (200 of 200 live previews in use); list them with scserve --list and stop the ones you no longer need with scserve --stop <token>`) and `PREVIEW_LIMIT` for snapshots.
- **404 and 410** on a saved tray (`TRAY_NOT_INITIALIZED`, `TRAY_EXPIRED`, a leadership the hub won't give back) create a new tray and retry once; elsewhere they exit 1. A tray expires an hour after its last leader left, so a machine that serves now and then gets a new tray, and new URLs, then.
- **Other 4xx** exit 1 with the hub's JSON `error` (`scserve: publishing: the tray hub answered 400: invalid preview entry path`).
- **5xx**, and a hub that can't be reached, are retried after 1, 2 and 5 s, then exit 1 (`the tray hub answered 503 4 times: …`).

`--list` and `--stop` drop a refused or vanished tray instead of creating one; a refused current tray still exits 3 there.

## State

| file | what |
| --- | --- |
| `<config>/tray.json` (`0600`) | the current tray (`hub`, `trayId`, `controllerToken`, `controllerUrl`, `controllerId`, `leaderKey`) and retired trays that still hold snapshots |
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

- **Who publishes:** seven's page, which leads a tray on the tray hub already. `serve` never talks to the hub: it asks the page with the [embedder kinds](#the-embedder-contract) below, and the page mints the preview and relays the hub's `preview.request` to `kernel.loopbackFetch`.
- **Local URLs** are the page's names for the kernel's loopback: `http://<port>.kernel.localhost/` (from `SLICC_PAGE_LOOPBACK_URL`, else `http://127.0.0.1:<port>/`).
- **`--ttl`** collects the snapshot itself, with the host's walker and limits, and hands the files to the page in the body of one `serve.snapshot` request; the page mints the snapshot, uploads the files and finalizes it ([`serve.snapshot`](#servesnapshot)). Nothing keeps running. The kernel doesn't upload itself: a program's `https://www.sliccy.ai/…` request through the kernel's proxy answers `502` in seven (measured from a seven branch preview with the page's transport, [#4](https://github.com/ai-ecoverse/slicc-serve/issues/4)), while the page reaches the hub, since it already leads a tray there.
- **`--open`** asks the page for a tab with `open.url` (as [slicc-open](https://github.com/ai-ecoverse/slicc-open) does) once the URL is printed; a refusal prints `serve: <the page's message>` and leaves the exit status alone.
- **`--upgrade`** prints `pnpm add -g @ai-ecoverse/slicc-serve@latest`.

### What runs where in SLICC

```
serve DIR ──spawn──► serve (background, one per preview)
                       ├─ static server on the kernel's 127.0.0.1:<random>
                       └─ /dev/embedder: serve.publish, held open ──► seven's page ──► tray hub ◄── visitor
                                           page: preview.request ──kernel.loopbackFetch──► 127.0.0.1:<port>
```

- **Detached:** `serve DIR` checks the directory and `--entry`, then starts a background `serve` (the same program, spawned with WASIX `proc_spawn`, standard input on `/dev/null`, standard error appended to `<state>/serve.log`, standard output on a pipe back to it) and waits. The background `serve` binds the static server, sends `serve.publish`, and writes one line back: the record, or what failed. The foreground `serve` prints it the way the host does and exits; the background one keeps the server and the request open. There is one background `serve` per preview and no relay: the page holds the tray. Like `nohup`, the background `serve` ignores `SIGHUP` as it starts, and none of its standard streams is the terminal, so closing the terminal that ran `serve` (a hangup of its foreground group) leaves the preview up; `serve --stop`, `kill` (`SIGTERM`) or `kill -9` end it.
- **The request is the preview.** The background `serve` blocks reading the `serve.publish` reply. When that process ends, for whatever reason (`serve --stop`, `kill`, the kernel ending, a crash), the kernel aborts the request and the page revokes the preview on the hub. When the page ends the reply (the user stopped it in seven, the hub revoked it, seven lost its tray), the background `serve` stops its server, removes its record and exits.
- **`--port PORT`** publishes a server that already runs on the kernel's loopback: the background `serve` holds only the request. `serve: nothing answers on 127.0.0.1:PORT yet` warns as on a host.
- **`--foreground`** does all of it in the `serve` process itself: it prints the URL, logs each request of the static server (`serve: GET /app.js 200 1234`), and Ctrl-C (or the end of the process) ends the request, which revokes the preview. When the page ends the reply, it prints `serve: stopped: <reason>` and exits 0.
- **`--local DIR`** starts a background `serve` with the static server only, recorded as `local` (listed, stopped with `--stop`); `--local --foreground DIR` serves in the foreground; `--local --port PORT` prints the URL only. None of them asks the page.
- **`--list`** asks the page with `serve.list` and joins its answer with the records, as the host joins the hub's list: `TOKEN MODE EXPIRES URL ROOT`, `(foreground)` for an attached one. A record whose process is gone is dropped (a snapshot's has no process and stays until it expires), and so is one whose token the page doesn't list (its process is stopped). When no page answers (`ENOTSUP`, or no `/dev/embedder`), the records alone are listed.
- **`--stop`** matches its argument against the records as on a host (token, public URL, local URL, port or record id). A public preview is stopped with `serve.stop`, which ends its background `serve` through the page; a token or a public URL that matches no record is sent to the page all the same. When the page refuses, the matching record's process gets `SIGTERM` anyway, and the page's message exits 1. A local preview's process gets `SIGTERM`. Either way the record is removed, and it prints `stopped <token or local URL>`. Several matching records stop none, as on a host.
- **State** is the host's layout in the kernel's file system: `<state>/previews/<id>.json` (`0600`) per preview, with the background process's pid, and `<state>/serve.log`, where `<state>` is `$SCSERVE_STATE_DIR`, else `$XDG_STATE_HOME/slicc/serve`, else `~/.local/state/slicc/serve`. There is no `tray.json` and no relay.

### Errors from the page

A request the page refuses fails with the kernel's errno, and `serve` prints `serve: <the first line of the page's message>` (the errno's text when there is none) and exits 1:

| errno | what it means | what `serve` adds |
| --- | --- | --- |
| `EACCES` | the user dismissed the prompt or didn't answer it, public previews are switched off, another request is waiting; or the kernel refused the request before the page saw it (`serve.publish is not allowed for this user`): the kinds are open to the session's own user, so a `serve` run under `sudo -u` someone else from another user's terminal is refused, while an agent in its own session works | `serve: if seven asked, answer in seven's tab (a click allows it), then run serve again` |
| `ENOTSUP` | no page answers for this session (`no page answers requests for this session`: a follower, a headless kernel), or a seven that doesn't answer the kind yet (`this embedder does not answer serve.publish`) | |
| `ENOENT` | `serve.stop`: the page holds no such preview | |
| `EBUSY`, `EIO`, any other | the page failed (`EIO` for an unreachable hub too) | |

A failure the tray hub answered comes as the reply's first line instead (`{"error", "status", …}`, below) and maps the way the host maps the hub's answers: `401` and `403`, or a `signIn`, exit 3 (`serve: publishing: the tray hub refused it (403: …)`, `serve: sign in at <url>`); `402` and `429` exit 4 (`serve: publishing: the tray hub's limit is reached (429: …)`, `serve: retry after 30 s`); any other status exits 1 (`serve: publishing: the tray hub answered 400: …`). `--json` prints the failure as `{"ok":false,"exit":N,"lines":[…],"signIn":URL|null}`, as on a host.

### The embedder contract

This is what seven's page answers ([#6](https://github.com/ai-ecoverse/slicc-serve/issues/6)); `serve`'s tests run against a fake page that answers the same in a node kernel. Each request is one open of `/dev/embedder` (slicc-kernel 1.153.0 and later): a JSON header line `{"kind": …, …options}`, a body (`serve.snapshot` only), then the reply until end-of-file. Option values are JSON, so no path or URL can break the header.

| kind | options | opened | reply | errors |
| --- | --- | --- | --- | --- |
| `serve.publish` | `port`, `root`, `entry`, `allowLive` | `O_RDWR` | one JSON line, `{"url","previewToken",…}` or `{"error",…}`; then held open until the preview ends, optionally one `{"stopped"}` line, then end-of-file | `EACCES`, `ENOTSUP`, `EIO` |
| `serve.list` | none | `O_RDWR` | one JSON line per preview, then end-of-file; nothing when there are none | `ENOTSUP`, `EIO` |
| `serve.stop` | `previewToken` | `O_WRONLY` | none; the close returns the errno | `ENOENT`, `ENOTSUP`, `EIO` |
| `serve.snapshot` | `root`, `entry`, `ttlMs`, `files`, `bytes` | `O_RDWR` | after the body: one JSON line, `{"url","previewToken","expiresAt",…}` or `{"error",…}`, then end-of-file | `EACCES`, `ENOTSUP`, `EIO` |
| `serve.logs` | `previewToken` (optional) | `O_RDWR` | one JSON line per lifecycle record, oldest first, then end-of-file | `ENOENT`, `ENOTSUP` |
| `serve.truncate` | `previewToken` (optional) | `O_RDWR` | one JSON line `{"cleared", "rearmed"}` | `ENOENT`, `ENOTSUP` |

#### `serve.publish`

```
{"kind":"serve.publish","port":32768,"root":"/home/me/site","allowLive":false}
{"kind":"serve.publish","port":5173,"entry":"app/index.html","allowLive":true}
```

| option | |
| --- | --- |
| `port` | number, 1 to 65535: the origin on the kernel's loopback, which the page reaches with `kernel.loopbackFetch(…, {port})` |
| `root` | string: the directory's absolute path in the kernel, for the prompt and the chip only; absent with `--port`. The page never reads files: every visit goes to `port` |
| `entry` | string, absent without `--entry`: the `--entry` path as `serve` normalized it (segments joined by `/`; no empty, `.` or `..` segment; no leading `/`; no `\`, `:` or NUL). The page refuses anything else, and a `port` outside 1 to 65535, with an `EIO` error (`invalid entry`, `invalid port`) |
| `allowLive` | boolean: `true` for `--port` (the hub doesn't edge-cache it), `false` for a directory (the hub's 5 s edge cache); `serve` always sends it, and the page takes `true` when it is missing |
| `bridge` | `true` with `--bridge`, else absent: see [The bridge in seven](#the-bridge-in-seven) |
| `maxTabs` | number, 1 to 100, with `bridge` only (default 20) |
| `quiet` | `true` with `--quiet`, with `bridge` only |

What the page does, in order:

1. **Who may publish:** a follower, a page that runs no kernel, or a seven with no tray hub configured throws `NotSupportedError` (`ENOTSUP`). A tray that isn't connected right now throws an `EIO` error with what is wrong. The switch "Public previews" off throws `NotAllowedError` (`EACCES`), `public previews are off in seven's settings`. The messages are bios's to choose; `serve` prints the first line as it is.
2. **The prompt:** unless the stamp says `serve` was typed in seven's terminal with a gesture still active (bios's `typed(stamp)`), the page asks: "Publish /home/me/site at a public URL?" (or "the server on port 5173"). Cancel, 60 s without an answer, or a second request of the same session while one is up, throws `NotAllowedError` with what happened (`the request to publish was dismissed`, `nobody answered the request to publish within 60 s`, `another request is waiting for you`); `serve` prints the message as it is.
3. **The mint:** `POST /api/tray/<trayId>/preview` `{servedRoot, entryPath, allowLive}`, where `servedRoot` is `/srv/serve/<id>` with a fresh random hex `<id>` of at least 16 digits from `crypto.getRandomValues` for each request (bios uses 32; the host relay 16) (never the real path, never the port: ports are reused), and `entryPath` is `<servedRoot>/<entry>`, or `<servedRoot>/index.html` without `entry` (the hub rejects `<servedRoot>/`).
4. **A failure the hub answered** (the mint's HTTP status wasn't 2xx) is the reply, not an errno, so that `serve` can exit 3 or 4: one line `{"error": <the hub's error, or the status text>, "status": <its HTTP status>, "code": <its code, when it has one>, "retryAfter": <its Retry-After, when present>, "signIn": <a sign-in URL, when the hub named one>, "active", "limit": <the hub's counts, when it sent them (`LIVE_PREVIEW_LIMIT`)>}`, then end-of-file. A hub the page can't reach at all is an error (`EIO`) with the reason.
5. **Registered:** the page keeps `servedRoot → {port, entryPath, entryIsRoot: entry absent, previewToken, url, root, entry, since, uid: stamp.uid}` and answers `{"url": …, "previewToken": …}` and a newline, optionally with `"trayId"`. The `url` is the hub's, with a trailing `index.html` dropped when there is no `entry` (the page may leave that to `serve`, which drops it too).
6. **Held open:** the page's reply stream stays open, with nothing more in it, as long as the preview is live. The page shows the live chip ("Serving publicly … Stop").

How it ends:

- **From the command's side:** the kernel aborts the handler's `signal` (reason `closed` when the description is closed or its process ends, `interrupted` when a signal reaches the blocked read) and cancels the reply stream. On either, the page revokes the preview on the hub (`POST /api/tray/<trayId>/preview/stop {previewToken}`) and drops it from the registry. If the signal is already aborted when the mint answers (the process went away during the prompt or the mint), the page revokes at once and replies nothing.
- **From the page's side** (the chip's Stop, `serve.stop`, `preview.revoked` from the hub, the switch turned off, the tray lost): the page revokes it on the hub unless the hub revoked it, drops it, writes `{"stopped": <reason>}` and a newline (`stopped in seven`, `stopped with serve --stop`, `revoked by the tray hub`, `public previews were switched off`, `seven lost its tray`), and closes the stream. Every page-side end writes the line, the chip's Stop and `serve.stop` included; `serve` treats end-of-file without it as `stopped in seven`.
- **A reload** ends the kernel, and every process with it: on boot, and after each leader reconnect, the page revokes the tray's live previews it holds no registration for, as the host relay does.

The relay itself (`preview.request` → `kernel.loopbackFetch(GET <path>, {port, headers: {range}})` → `preview.response` chunks) follows [The relay's answers](#the-relays-answers), with `vfsPath` checked against the registered `servedRoot`, the path under it sent as it came, and the `entryPath` of a preview without `entry` sent as `/`.

#### `serve.list`

```
{"kind":"serve.list"}
```

The reply is one JSON object per line for each preview the page holds from `serve.publish` or `serve.snapshot` that the caller may see: those whose publishing process had the caller's uid, all of them for root. `{"previewToken", "url", "root", "port", "entry", "since"}` for a live preview, with `root` and `entry` absent as in the request and `since` the mint's time in milliseconds since the epoch, which `serve --list` shows as `createdAt` (ISO 8601) when it has no record of its own. A snapshot's line has `"mode": "persistent"` and `"expiresAt"` (milliseconds since the epoch) and no `port`; a live one may say `"mode": "live"`. An empty reply means none.

#### `serve.stop`

```
{"kind":"serve.stop","previewToken":"0f3c….a91e…"}
```

The page revokes the preview on the hub (a snapshot too, before its TTL), drops it, and ends a live preview's `serve.publish` reply with `{"stopped":"stopped with serve --stop"}`, so the background `serve` that holds it exits. A token the page doesn't hold is revoked on the tray all the same; one the hub has no preview for throws `NotFoundError` (`ENOENT`), `the tray hub has no such preview`. A held preview of another uid, for a caller that isn't root, is `ENOENT` too.

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

1. **Who may publish, and the prompt:** as for `serve.publish`, before it reads the body: a follower or a seven without a tray hub is `ENOTSUP`, the switch "Public previews" off is `EACCES`, and unless `serve` was typed in seven's terminal with a gesture still active, it asks: "Publish a snapshot of /home/me/site (2 files, 1.8 KiB) at a public URL for 7 days?". Cancel or no answer is `EACCES`, with its message.
2. **The mint:** `POST /api/tray/<trayId>/preview` `{servedRoot, entryPath, allowLive: false, ttlMs}`, with `servedRoot` and `entryPath` made as for `serve.publish`. The answer's `uploadToken` stays in the page.
3. **The uploads:** on `*.sliccy.ai` the page reaches the hub through sliccy-ai's same-origin proxy, which forwards `preview/<token>/file` and `preview/<token>/finalize` for this ([sliccy-ai#162](https://github.com/ai-ecoverse/sliccy-ai/pull/162)); elsewhere it calls the hub directly. For each file of the body, `PUT /api/tray/<trayId>/preview/<previewToken>/file?path=<path>` with the upload token as the bearer, `Content-Type: <type>` and the bytes. The page may upload a few at a time, and may report progress with `request.progress` (`uploaded 12 of 40 files`), which `serve` shows on its standard error.
4. **The finalize:** `POST /api/tray/<trayId>/preview/<previewToken>/finalize` with the upload token, once the body has ended.
5. **The reply:** one line `{"url": <the hub's url>, "previewToken", "trayId"?, "expiresAt": <milliseconds since the epoch>, "files", "bytes"}` and a newline, then end-of-file. The page keeps the snapshot for `serve.list` and `serve.stop` (with `stamp.uid`, `root`, `entry`, `since`, `expiresAt`) until it expires or is stopped, across reloads, since a snapshot outlives the page.

When it fails, the page answers at once, without reading the rest of the body: with an error, or with the failure line as a string (never a stream). The kernel then drops whatever `serve` still writes ([slicc-kernel#1536](https://github.com/ai-ecoverse/slicc-kernel/issues/1536)), so `serve` never waits on a body nobody reads; it gets the errno from its next write or from its read, and the failure line from its read.


- **The body is wrong** (a line that isn't the JSON above, a path or type that breaks the rules, a `size` over 25 MiB, a body that ends inside a file, more than 1,000 files or 50 MiB, the same path twice, or files and bytes that don't add up to exactly `files` and `bytes` when the body ends, so what is finalized is what the prompt showed): the page stops the snapshot on the hub (`POST …/preview/stop`) and throws an `EIO` error `bad snapshot body: <what>`.
- **The hub answered a failure** to the mint, an upload or the finalize: the page stops a minted snapshot, and the reply is the failure line of `serve.publish` (`{"error", "status", "code"?, "retryAfter"?, "signIn"?, "active"?, "limit"?}`), with `"during": "mint"`, `"upload"` or `"finalize"`, and for an upload `"path"`. `serve` says `publishing the snapshot` for the mint and `uploading the snapshot` otherwise, adds ` (<path>)` to the hub's error for an upload, and maps the status to exits 1, 3 and 4 as for `serve.publish`. `PREVIEW_LIMIT` (`429`, the tray's 10 snapshots) is exit 4.
- **The hub can't be reached:** the page stops a minted snapshot if it can, and throws an `EIO` error with the reason.
- **`serve` goes away** before the reply (Ctrl-C, `kill`): the kernel aborts the request, and the page stops the snapshot on the hub unless it had already written its reply, a finalize still in flight included, since `serve` never got the URL. A snapshot whose reply the page had written stays (`serve` may have exited before reading it), and `serve --list` shows it from `serve.list`.

### The bridge in seven

`serve --bridge DIR` in seven is the same preview, with seven's page as the leader that the hub's `bridge.*` messages reach. `serve` sends `bridge`, `maxTabs` and `quiet` with `serve.publish`, and the options and output are the host's, except that the targets show up on seven's own CDP endpoint instead of a local one: `serve: its visitor tabs are CDP targets preview:<token>:* on $SLICC_CDP_URL`. What the page does:

1. **The mint** adds `bridge: true`, `maxTabs`, `quiet` and `webhookId: "serve-<16 hex>"` (a fresh name for each preview, matching seven's webhook names) to the mint. The prompt says so: "Publish /home/me/app at a public URL, and let the agent drive the tabs of whoever opens it?" The reply's first line adds `"webhook": "serve-<16 hex>"`, which `serve` prints.
2. **Visitor tabs as targets.** For each `bridge.connected` of a preview the page holds, the page adds a target `preview:<token>:<connId>` (`type: "page"`, the preview's URL and the connection's origin, as on a host) to the `Target` domain its CDP hook emulates for kernel programs (`SLICC_CDP_URL`), and drops it on `bridge.disconnected` or when the preview stops. It answers in a session exactly as the host's relay does ([The bridge](#the-bridge---bridge)): the same synthetic methods and navigation events, everything else as `bridge.cdp.request` over the tray's controller socket, the same errors and the same 30 s timeout; `Target.closeTarget` sends `bridge.close`. The targets pass the same guard as seven's other targets: a connection seven wouldn't let drive a tab doesn't see them, and whatever seven asks before an agent drives a tab, it asks here. `bridge.connected` with `replay: true` restores a target without recording or announcing it. A `bridge.cdp.response` for a `connId` the page doesn't know is dropped.
3. **Emits** arrive as `webhook.event {webhookId: "serve-…", headers: {"x-slicc-preview-conn", "x-slicc-preview-token"}, body: {name, detail}}`, which seven already delivers to the agent as a webhook of that name. The page delivers them only while it holds the preview, and drops them after it stopped.
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

- `crates/serve` is the binary (`scserve`, and `scserve.wasm` for the kernel); `crates/serve-core` has no host-only dependency and builds for `wasm32-wasip1-threads`; `crates/serve-hub` (native only) is the hub client, the tray, the relay and the CLI.
- `npm run lint`: Biome, the no-comment check, `slicc-lint`, `shellcheck` and `dash -n` on `install.sh`, `cargo fmt`, and `cargo clippy` for the host and `wasm32-wasip1-threads`.
- `npm test` builds `scserve` and runs `test/integration/` with Node's test runner. The hub there is `fake-hub.mjs`: trays, leader election, the controller WebSocket (`preview.request` to the relay and the chunks back, `preview.revoked`), mint, upload, finalize, stop and list, a visitor path that resolves `<label>.localhost` hosts like the preview worker, and injected answers per route for every class of error. Tests never reach `www.sliccy.ai`. `seven.test.mjs` installs the package (its staged `package.json`, from `scripts/targets.mjs`) with `bin/serve.wasm` (after `npm run build:wasm`), wasm-bash and wasm-coreutils in a node kernel from `@ai-ecoverse/slicc-kernel` (Node 26), behind a fake page that answers `serve.publish`, `serve.list`, `serve.stop` and `open.url` and records each request: the background `serve`, the page ending a preview, a killed holder revoking, `--port`, `--foreground`, `--open`, `--list` and `--stop` by token, URL and port, each errno and each class of hub failure, and a hostile directory name in the header; then a kernel no page answers, and slicc-kernel 1.152.2 (the devDependency `slicc-kernel-without-embedder`), which refuses the package and, with `engines` stripped, has no `/dev/embedder`. `seven-ttl.test.mjs` does the same for `--ttl`, with a fake page that implements `serve.snapshot` against `fake-hub.mjs` (it reads the body, mints, uploads, finalizes, and stops what it minted on a failure): the files that reach the hub, `--json`, `--entry`, `--open`, a file past the request window, the limits checked before the page is asked, each hub failure (mint, upload, finalize, with a 3 MiB body answered early), the page's refusals, and a `serve` killed before the finalize. `install.test.mjs` runs `install.sh` and `--upgrade` against local release directories.
- `npm run test:unit` runs the unit tests in the gitignored `test/unit/` with `cargo llvm-cov`; the pre-commit hook wants 100% coverage of the changed lines in the crates it measures.

CI is `.github/workflows/ci.yml` on [slicc-shared-web's pipeline](https://github.com/ai-ecoverse/slicc-shared-web): lint, the integration tests on Linux, `npm run test:windows` on Windows, the same tests on macOS, `cargo check` for every release target, and the reviews. On a push to `main`, semantic-release tags a release; then the binaries are built for every target (the darwin ones signed and notarized), collected with `serve.wasm` into `SHA256SUMS` (attested), attached to the release with both installers, installed once through `install.sh`, and published to npm as `@ai-ecoverse/slicc-serve` and its six platform packages.

## License

Apache-2.0
