pub mod args;
pub mod exit;
pub mod http;
pub mod mime;
pub mod paths;
pub mod range;
pub mod record;
pub mod safe;
pub mod server;
pub mod snapshot;
pub mod time;
pub mod token;

pub const NAME: &str = "scserve";
pub const PACKAGE: &str = "@ai-ecoverse/slicc-serve";
pub const DEFAULT_HUB: &str = "https://www.sliccy.ai";
pub const SERVED_PREFIX: &str = "/srv/serve/";
pub const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
pub const MAX_RANGE_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_SNAPSHOT_FILES: usize = 1_000;
pub const MAX_SNAPSHOT_BYTES: u64 = 50 * 1024 * 1024;
pub const MAX_TTL_MS: u64 = 30 * 86_400_000;

pub fn version() -> &'static str {
    option_env!("SCSERVE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

#[cfg(all(test, serve_unit))]
mod unit {
    include!("../../../test/unit/serve_core.rs");
}
