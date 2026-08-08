//! Daemonless repository surfaces shared by the `spice` binary.

#![forbid(unsafe_code)]

/// The canvas this appliance serves, re-exported so the lane builder and
/// resident share the sole [`canvas::APP_STAGE`] generation store.
pub use canvas;

pub mod agent_launch;
pub mod cli;
pub mod config;
#[cfg(not(target_arch = "wasm32"))]
pub mod gate;
#[cfg(not(target_arch = "wasm32"))]
pub mod git_shadow;
#[cfg(not(target_arch = "wasm32"))]
pub mod install;
pub mod placeholder;
pub mod repository;
pub mod resident_lock;
#[cfg(not(target_arch = "wasm32"))]
pub mod task_git;
