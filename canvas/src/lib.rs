//! Loopback operator surface, derived reads, and causal thread walks.

#![forbid(unsafe_code)]

mod asset;
#[cfg(not(target_arch = "wasm32"))]
pub mod axum;
pub mod config;
#[cfg(not(target_arch = "wasm32"))]
pub mod stream;
#[cfg(not(target_arch = "wasm32"))]
pub mod thread;
pub mod view;

pub use asset::{Discovery, EMBEDDED_APP, EmbeddedApp, EmbeddedAsset, PublishError};
pub use config::{Config, ConfigError};
