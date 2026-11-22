//! Public directory server components, independent of the maintainer pipeline.
#[cfg(test)]
#[path = "../asset_manifest.rs"]
mod asset_manifest;
pub mod config;
pub mod discovery;
pub mod metadata;
pub mod render;
pub mod server;
pub mod similar;
pub mod snapshot;
