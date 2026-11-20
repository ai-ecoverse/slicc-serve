# slicc-serve

`scserve` on hosts, `serve`/`scserve` in SLICC: publish a directory or a local server through the SLICC tray hub. README.md is the spec. Rust workspace: `crates/serve` (bin), `serve-core` (no host-only deps, builds for `wasm32-wasip1-threads`), `serve-hub` (native: tray, relay daemon, hub client). npm `@ai-ecoverse/slicc-serve` and `…-<os>-<arch>`.

No comments in files we write. Guidance only here (≤1000 chars); no CLAUDE.md. Unit tests: gitignored `test/unit/<crate>.rs` under `cfg(all(test, serve_unit))`, 100% diff coverage. Regressions: committed `test/integration/` against `fake-hub.mjs`, via the CLI; every bug fix adds one.

Never contact the real tray hub (`www.sliccy.ai`) in tests: set `SLICC_TRAY_HUB` to the fake. Never use ports 9222-9229. Don't change the hub's wire contract. Releases: semantic-release on `main`.
