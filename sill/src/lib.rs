//! Process, advisory-lock, and interface-address floor with typed lock
//! configuration.

#[cfg(not(any(target_os = "linux", target_os = "macos", target_arch = "wasm32")))]
compile_error!("sill supports the Linux and macOS release targets plus wasm contract checks");

pub mod config;
#[cfg(unix)]
mod interface;
mod lock;
mod observe;
#[cfg(unix)]
mod process_tree;
mod protected;
mod spawn;
mod spec;
mod surface;

pub use config::{
    ConfigError, ConfiguredPath, ExitCode, ExitStatuses, LockConfig, LockTarget, ShardCount,
};
#[cfg(unix)]
pub use interface::local_addresses;
pub use lock::{Acquisition, Availability, LockFile, LockName, LockNameError, Possession};
pub use observe::{
    Observation, ObservationFailure, ObservationSpecError, ObservationStall, ObserveError,
    ObservedRecord, OutputStream, RecordBoundary, RecordObserver,
};
#[cfg(unix)]
pub use process_tree::DescendantGroups;
pub use spawn::{
    CapturedOutput, Completion, ProcessUsage, Running, WaitError, WaitOutcome, WaitStep, spawn,
    spawn_observed,
};
#[cfg(unix)]
pub use spec::CancellationRegistration;
pub use spec::{
    Argv, Cancellation, CancellationToken, Deadlines, Environment, Output, ProcessGroup, Redaction,
    RestartDisposition, Spec, SpecError, StdinPolicy, StreamPolicy, WaitPolicy,
};
pub use surface::{
    Contention, HeldShard, LockAttempt, LockAvailability, LockExitReport, LockOwnerReport,
    LockPoolReport, LockReport, LockShardReport, LockSurfaceError, LockTargetKind, LockTopology,
};
