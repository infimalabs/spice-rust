//! Outbound agent launch contracts and the single process-door handoff.
//!
//! Vendor modules state exact command lines, while [`Record`] owns the common
//! binary, argument, stream, dialect, and resume contract. Process creation is
//! delegated to `sill`, and captured records are delegated to `listen` after a
//! caller has committed them to tape and supplied their locus.

#![forbid(unsafe_code)]

pub mod acp;
pub mod claude;
pub mod codex;
pub mod launch;
mod record;

pub use acp::{PermissionRequestError, permission_request};
pub use launch::{
    Effort, Launch, McpServer, POST_TOOL_HOOK_EVENT, POST_TOOL_HOOK_STATUS_MESSAGE,
    POST_TOOL_HOOK_TIMEOUT_SECONDS, SteeringHook, SteeringKey,
};
pub use record::{
    LaunchError, Process, ProcessError, Record, RecordStream, Resume, SessionId, SessionIdError,
};
