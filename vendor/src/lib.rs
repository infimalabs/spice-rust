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
pub mod config;
pub mod launch;
pub mod omp;
mod record;
mod vendor;

pub use acp::{PermissionRequestError, permission_request};
pub use config::{
    Binary, ClaudeConfig, CodexConfig, Config, DEFAULT_CLAUDE_AUTO_COMPACT_WINDOW_TOKENS,
    DEFAULT_CLAUDE_MODEL, DEFAULT_CODEX_MODEL, DEFAULT_CODEX_MODEL_CONTEXT_WINDOW,
    DEFAULT_OMP_MODEL, OmpConfig, codex_auto_compact_token_limit,
};
pub use launch::{
    Attribution, ContractError, Effort, EffortError, Launch, McpServer, POST_TOOL_HOOK_EVENT,
    POST_TOOL_HOOK_STATUS_MESSAGE, POST_TOOL_HOOK_TIMEOUT_SECONDS, PRE_TOOL_HOOK_EVENT,
    PRE_TOOL_HOOK_STATUS_MESSAGE, PRE_TOOL_HOOK_TIMEOUT_SECONDS, SteeringHook, SteeringKey,
};
pub use record::{
    LaunchError, Process, ProcessError, Record, RecordStream, Resume, SessionDeclaration,
    SessionId, SessionIdError,
};
pub use vendor::{
    CLAUDE, CODEX, LaunchValues, OMP, StdoutContract, StdoutFormat, SteeringChannel, ToolEnvelope,
    TranscriptIndex, TranscriptLayout, Vendor, WorkingDirectoryFolders,
};
