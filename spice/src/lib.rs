//! Daemonless repository surfaces shared by the `spice` binary.

#![forbid(unsafe_code)]

/// The canvas this appliance serves, re-exported so the lane builder and
/// resident share the sole [`canvas::APP_STAGE`] generation store.
pub use canvas;

pub mod agent_launch;
#[cfg(not(target_arch = "wasm32"))]
pub mod agent_wrapper;
#[cfg(not(target_arch = "wasm32"))]
pub mod cargo_isolation;
#[cfg(not(target_arch = "wasm32"))]
pub mod change_gate;
pub mod claim_lease;
pub mod cli;
pub mod command_advice;
pub mod commit_message;
pub mod config;
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
