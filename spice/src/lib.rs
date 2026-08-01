//! Daemonless repository surfaces shared by the `spice` binary.

#![forbid(unsafe_code)]

pub mod config;
#[cfg(not(target_arch = "wasm32"))]
pub mod install;
pub mod repository;
