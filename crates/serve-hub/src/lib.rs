#![cfg(not(target_os = "wasi"))]

pub mod bridge;
pub mod cdp;
pub mod cli;
pub mod client;
pub mod daemon;
pub mod detach;
pub mod fsx;
pub mod hub;
pub mod list;
pub mod open;
pub mod records;
pub mod relay;
pub mod tray;
pub mod ttl;
pub mod upgrade;
pub mod ws;

#[cfg(all(test, serve_unit))]
mod unit {
    include!("../../../test/unit/serve_hub.rs");
}
