//! Daemonless repository surfaces shared by the `spice` binary.

#![forbid(unsafe_code)]

pub mod agent_launch;
pub mod cli;
pub mod config;
#[cfg(not(target_arch = "wasm32"))]
pub mod gate;
#[cfg(not(target_arch = "wasm32"))]
pub mod install;
pub mod placeholder;
pub mod repository;
#[cfg(not(target_arch = "wasm32"))]
// Gate's source ceiling is narrower than rustfmt's default width.
#[rustfmt::skip]
pub mod task_git;
