//! Process and advisory-lock floor with typed lock configuration.

#[cfg(not(any(target_os = "linux", target_os = "macos", target_arch = "wasm32")))]
compile_error!("sill supports the Linux and macOS release targets plus wasm contract checks");

pub mod config;
mod lock;
mod spawn;
mod spec;
mod surface;

pub use config::{ConfigError, ExitCode, ExitStatuses, LockConfig, ShardCount};
pub use lock::{Acquisition, LockFile, LockName, LockNameError, Possession};
pub use spawn::{CapturedOutput, Completion, Running, WaitError, WaitOutcome, WaitStep, spawn};
pub use spec::{
    Argv, Cancellation, CancellationToken, Deadlines, Environment, Output, ProcessGroup, Redaction,
    RestartDisposition, Spec, SpecError, StdinPolicy, StreamPolicy, WaitPolicy,
};
pub use surface::{
    Contention, HeldShard, LockAttempt, LockExitReport, LockPoolReport, LockReport,
    LockSurfaceError, LockTopology,
};
