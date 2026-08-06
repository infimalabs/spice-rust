//! Daemonless repository surfaces shared by the `spice` binary.

#![forbid(unsafe_code)]

/// The canvas this appliance serves, re-exported so the lane-local builder and
/// resident share the one [`canvas::DEV_STAGE`] path.
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
#[cfg(not(target_arch = "wasm32"))]
// Gate's source ceiling is narrower than rustfmt's default width.
#[rustfmt::skip]
pub mod task_git;
