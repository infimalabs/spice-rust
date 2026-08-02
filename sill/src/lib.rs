//! Process and advisory-lock floor with typed lock configuration.

#[cfg(not(any(target_os = "linux", target_os = "macos", target_arch = "wasm32")))]
compile_error!("sill supports the Linux and macOS release targets plus wasm contract checks");

pub mod config;
mod lock;
mod observe;
mod spawn;
mod spec;
mod surface;

pub use config::{ConfigError, ExitCode, ExitStatuses, LockConfig, ShardCount};
pub use lock::{Acquisition, Availability, LockFile, LockName, LockNameError, Possession};
pub use observe::{
    Observation, ObservationFailure, ObservationSpecError, ObserveError, ObservedRecord,
    OutputStream, RecordBoundary, RecordObserver,
};
pub use spawn::{
    CapturedOutput, Completion, Running, WaitError, WaitOutcome, WaitStep, spawn, spawn_observed,
};
pub use spec::{
    Argv, Cancellation, CancellationToken, Deadlines, Environment, Output, ProcessGroup, Redaction,
    RestartDisposition, Spec, SpecError, StdinPolicy, StreamPolicy, WaitPolicy,
};
pub use surface::{
    Contention, HeldShard, LockAttempt, LockAvailability, LockExitReport, LockPoolReport,
    LockReport, LockShardReport, LockSurfaceError, LockTopology,
};
