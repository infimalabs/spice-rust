//! Loopback operator surface, derived reads, and causal thread walks.

#![forbid(unsafe_code)]

mod asset;
#[cfg(not(target_arch = "wasm32"))]
pub mod axum;
pub mod config;
pub mod speech;
#[cfg(not(target_arch = "wasm32"))]
pub mod stream;
#[cfg(not(target_arch = "wasm32"))]
pub mod thread;
pub mod view;

pub use asset::{
    AppSource, AppSourceError, Discovery, EMBEDDED_APP, EmbeddedApp, EmbeddedAsset, LOCAL_STAGE,
    PublishError, ResolvedAsset,
};
pub use config::{Config, ConfigError};
pub use speech::{SpeechBackend, SpeechConfig, SpeechConfigError};
