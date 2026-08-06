//! Daemonless repository surfaces shared by the `spice` binary.

#![forbid(unsafe_code)]

/// The canvas this appliance serves, re-exported for the build tooling.
///
/// `Cargo.lock` is one of the inputs the committed app stage records itself
/// against, so a crate naming `canvas` as a new dependency invalidates that
/// stage and demands a rebuild in the sealed image. Reaching it through a
/// dependency that is already declared costs no manifest edit, which is what
/// lets `cargo xtask dev-app` name [`canvas::DEV_STAGE`] rather than restate
/// the path the resident will read.
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
