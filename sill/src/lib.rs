//! Dependency-free process and advisory-lock floor.

#[cfg(not(any(target_os = "linux", target_os = "macos", target_arch = "wasm32")))]
compile_error!("sill supports the Linux and macOS release targets plus wasm contract checks");

mod lock;
mod spawn;
mod spec;

pub use lock::{Acquisition, LockFile, LockName, LockNameError, Possession};
pub use spawn::{CapturedOutput, Completion, Running, WaitError, WaitOutcome, spawn};
pub use spec::{
    Argv, Cancellation, CancellationToken, Deadlines, Environment, Output, ProcessGroup, Redaction,
    RestartDisposition, Spec, SpecError, StdinPolicy, StreamPolicy, WaitPolicy,
};
