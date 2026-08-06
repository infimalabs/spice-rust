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
    APP, APP_GENERATION_ROUTE, App, AppAsset, AppGeneration, AppSource, AppSourceError, Discovery,
    LOCAL_STAGE, LOCAL_STAGE_CURRENT, LOCAL_STAGE_GENERATIONS, PublishError, ServedAsset,
};
pub use config::{Config, ConfigError};
pub use speech::{SpeechBackend, SpeechConfig, SpeechConfigError};
