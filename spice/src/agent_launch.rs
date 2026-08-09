//! Pure composition of one admitted resident-agent launch plan.

use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

use sill::{Environment, SpecError};
use wire::{CanonicalUuid, Name};

use crate::config::Root;

/// Child variable understood by Claude Code's auto-compaction boundary.
pub const CLAUDE_AUTO_COMPACT_WINDOW: &str = "CLAUDE_CODE_AUTO_COMPACT_WINDOW";
/// Child variable binding every supervised shell to its durable lane.
pub const SPICE_LANE: &str = "SPICE_LANE"; // env-policy: allow
/// Claude's native commit attribution trailer.
pub const CLAUDE_ATTRIBUTION_TRAILER: &str = "Co-Authored-By";

/// Outer-boundary values required to compose a launch without ambient reads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Inputs {
    /// Resolved worktree root.
    pub worktree: PathBuf,
    /// Selected lane vendor.
    pub vendor: Name,
    /// Neutral skill prompt with no operator prose.
    pub prompt: String,
    /// Current Spice executable spelling used by the hook and MCP server.
    pub executable: String,
    /// Explicit child environment admitted by the resident rim.
    pub environment: Environment,
    /// Durable lane identity and steering key.
    pub lane: CanonicalUuid,
    /// Fresh or exact-session continuation.
    pub resume: vendor::Resume,
}

/// Landed renderer selected by one admitted plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dialect {
    /// Claude Code stream-json.
    Claude,
    /// Codex exec JSONL.
    Codex,
}

/// Complete deterministic inputs to vendor rendering and the process door.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    dialect: Dialect,
    binary: String,
    launch: vendor::Launch,
    environment: Environment,
}

impl Plan {
    /// Selected vendor dialect.
    #[must_use]
    pub const fn dialect(&self) -> Dialect {
        self.dialect
    }

    /// Configured vendor executable.
    #[must_use]
    pub fn binary(&self) -> &str {
        &self.binary
    }

    /// Vendor-neutral launch fields.
    #[must_use]
    pub const fn launch(&self) -> &vendor::Launch {
        &self.launch
    }

    /// Exact child environment, including composition-owned variables.
    #[must_use]
    pub const fn environment(&self) -> &Environment {
        &self.environment
    }

    /// Render this already admitted plan into the selected vendor dialect.
    ///
    /// # Errors
    ///
    /// Returns a path-specific vendor contract refusal or the common
    /// argument-vector refusal. Rendering performs no repository, policy,
    /// environment, or clock reads.
    pub fn record(&self) -> Result<vendor::Record, vendor::ContractError> {
        match self.dialect {
            Dialect::Claude => vendor::claude::record(self.binary.as_str(), &self.launch),
            Dialect::Codex => vendor::codex::record(self.binary.as_str(), &self.launch),
        }
    }
}

/// Compose every shared and vendor-specific launch field from admitted inputs.
///
/// # Errors
///
/// Refuses an unsupported lane vendor, empty rim values, or a child environment
/// that attempts to supply the composition-owned lane identity.
pub fn compose(root: &Root, inputs: Inputs) -> Result<Plan, ComposeError> {
    if inputs.worktree.as_os_str().is_empty() {
        return Err(ComposeError::Empty("launch.worktree"));
    }
    if inputs.executable.is_empty() || inputs.executable.contains('\0') {
        return Err(ComposeError::Empty("launch.executable"));
    }
    if environment_contains(&inputs.environment, SPICE_LANE) {
        return Err(ComposeError::ReservedEnvironment(SPICE_LANE));
    }

    let mut environment = inputs.environment.entries().to_vec();
    environment.push((
        OsString::from(SPICE_LANE),
        OsString::from(inputs.lane.to_string()),
    ));
    let attribution = if root
        .policy()
        .commit_message()
        .trailer_rejection(CLAUDE_ATTRIBUTION_TRAILER)
        .is_some()
    {
        vendor::Attribution::Withheld
    } else {
        vendor::Attribution::Harness
    };
    let steering_key = Some(vendor::SteeringKey {
        token: inputs.lane.to_string(),
    });

    let (dialect, binary, model, effort, personality, fast_mode) = match inputs.vendor.as_str() {
        "claude" => {
            let configured = root.vendor().claude();
            if !environment_contains_entries(&environment, CLAUDE_AUTO_COMPACT_WINDOW) {
                environment.push((
                    OsString::from(CLAUDE_AUTO_COMPACT_WINDOW),
                    OsString::from(configured.auto_compact_window_tokens().to_string()),
                ));
            }
            (
                Dialect::Claude,
                configured.binary().as_str(),
                configured.model(),
                configured.effort(),
                None,
                None,
            )
        }
        "codex" => {
            let configured = root.vendor().codex();
            (
                Dialect::Codex,
                configured.binary().as_str(),
                configured.model(),
                configured.effort(),
                configured.personality().map(str::to_owned),
                Some(configured.fast_mode()),
            )
        }
        other => return Err(ComposeError::UnsupportedVendor(other.to_owned())),
    };
    let hook_harness = match dialect {
        Dialect::Claude => "claude",
        Dialect::Codex => "codex",
    };
    let hook = vendor::SteeringHook {
        command: format!("{} hook {hook_harness}", shell_word(&inputs.executable)),
    };
    let mcp = vendor::McpServer {
        name: "spice".to_owned(),
        command: inputs.executable,
        args: vec!["mcp".to_owned()],
    };
    let environment = Environment::new(environment).map_err(ComposeError::Environment)?;

    Ok(Plan {
        dialect,
        binary: binary.to_owned(),
        launch: vendor::Launch {
            cwd: inputs.worktree,
            prompt: inputs.prompt,
            model: model.to_owned(),
            effort,
            personality,
            fast_mode,
            attribution,
            hook,
            steering_key,
            mcp,
            resume: inputs.resume,
        },
        environment,
    })
}

fn environment_contains(environment: &Environment, key: &str) -> bool {
    environment_contains_entries(environment.entries(), key)
}

fn environment_contains_entries(entries: &[(OsString, OsString)], key: &str) -> bool {
    entries.iter().any(|(candidate, _)| candidate == key)
}

fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// A launch plan could not be composed from its admitted inputs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComposeError {
    /// A required outer-rim input was empty.
    Empty(&'static str),
    /// The selected lane vendor has no landed renderer.
    UnsupportedVendor(String),
    /// The child attempted to supply a composition-owned variable.
    ReservedEnvironment(&'static str),
    /// The completed child environment violated process-door law.
    Environment(SpecError),
}

impl fmt::Display for ComposeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty(path) => write!(formatter, "{path} must not be empty"),
            Self::UnsupportedVendor(vendor) => write!(
                formatter,
                "lane.vendor must be claude or codex; observed {vendor:?}"
            ),
            Self::ReservedEnvironment(name) => write!(
                formatter,
                "launch.environment.{name} is composition-owned and must not be supplied"
            ),
            Self::Environment(error) => {
                write!(formatter, "launch.environment is invalid: {error}")
            }
        }
    }
}

impl Error for ComposeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Environment(error) => Some(error),
            Self::Empty(_) | Self::UnsupportedVendor(_) | Self::ReservedEnvironment(_) => None,
        }
    }
}
