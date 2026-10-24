//! Daemonless repository surfaces shared by the `spice` binary.

#![forbid(unsafe_code)]

/// The app server, re-exported so the lane builder and resident share the sole
/// [`serve::APP_STAGE`] generation store.
pub use serve;

pub mod agent_launch;
#[cfg(not(target_arch = "wasm32"))]
pub mod agent_prose;
#[cfg(not(target_arch = "wasm32"))]
pub mod agent_wrapper;
pub mod ambient;
#[cfg(not(target_arch = "wasm32"))]
pub mod cargo_isolation;
#[cfg(unix)]
pub mod cargo_output;
#[cfg(not(target_arch = "wasm32"))]
pub mod change_gate;
pub mod claim_lease;
pub mod cli;
pub mod command_advice;
pub mod commit_message;
pub mod config;
#[cfg(not(target_arch = "wasm32"))]
pub mod export;
pub mod extension;
#[cfg(not(target_arch = "wasm32"))]
pub mod gate;
#[cfg(not(target_arch = "wasm32"))]
pub mod git_shadow;
#[cfg(not(target_arch = "wasm32"))]
pub mod install;
#[cfg(not(target_arch = "wasm32"))]
pub mod journal_schema;
pub mod placeholder;
#[cfg(not(target_arch = "wasm32"))]
pub mod quote;
pub mod repository;
pub mod resident_lock;
pub mod rooms;
#[cfg(not(target_arch = "wasm32"))]
pub mod rtk;
#[cfg(not(target_arch = "wasm32"))]
pub mod session_meter;
#[cfg(not(target_arch = "wasm32"))]
mod steering_read;
#[cfg(not(target_arch = "wasm32"))]
pub mod task_git;
#[cfg(test)]
mod test_repository;
#[cfg(not(target_arch = "wasm32"))]
pub mod version;
