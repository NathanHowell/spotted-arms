// Silence warnings for crates used only in binaries or indirectly
#![allow(unused_crate_dependencies)]

pub mod compute;
pub mod github;
pub mod instance;
pub mod metadata;
pub mod server;
pub mod telemetry;
pub mod utils;
pub mod webhook;
