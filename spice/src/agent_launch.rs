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
/// Child variable binding every supervised Rust shell to its durable lane.
pub const SPICE_RS_LANE: &str = wire::SpiceVariable::Lane.as_str();
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
    /// Current Spice executable spelling used by the hook.
    pub executable: String,
    /// Explicit child environment admitted by the resident rim.
    pub environment: Environment,
    /// Durable lane identity. Also the seed for this launch's steering key,
    /// which the resident derives from the same value the same way -- see
    /// [`steer::Key::for_identity`].
    pub lane: CanonicalUuid,
    /// Fresh or exact-session continuation.
    pub resume: vendor::Resume,
    /// The configuration overlay the lane wrote for a vendor that reads one.
    pub config_overlay: Option<PathBuf>,
    /// Authoritative global fast-mode value for vendors that support it.
    pub fast_mode: bool,
    /// Model this lane selected over the vendor's configured model.
    pub model: Option<String>,
    /// Effort this lane selected over the vendor's configured effort.
    pub effort: Option<vendor::Effort>,
}

/// Complete deterministic inputs to vendor rendering and the process door.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    vendor: &'static crate::extension::Vendor,
    binary: String,
    launch: vendor::Launch,
    environment: Environment,
}

impl Plan {
    /// Selected declared vendor.
    #[must_use]
    pub const fn vendor(&self) -> &'static crate::extension::Vendor {
        self.vendor
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
        self.vendor.record(self.binary.as_str(), &self.launch)
    }
}

/// Compose every shared and vendor-specific launch field from admitted inputs.
///
/// # Errors
///
/// Refuses an unsupported lane vendor, empty rim values, or a child environment
/// that attempts to supply the composition-owned lane identity.
pub fn compose(root: &Root, inputs: Inputs) -> Result<Plan, ComposeError> {
    let registry = crate::extension::registry()
        .expect("compiled extension declarations are validated by the test gate");
    compose_with_registry(root, inputs, &registry)
}

fn compose_with_registry(
    root: &Root,
    inputs: Inputs,
    registry: &crate::extension::Registry<'_>,
) -> Result<Plan, ComposeError> {
    if inputs.worktree.as_os_str().is_empty() {
        return Err(ComposeError::Empty("launch.worktree"));
    }
    if inputs.executable.is_empty() || inputs.executable.contains('\0') {
        return Err(ComposeError::Empty("launch.executable"));
    }
    if environment_contains(&inputs.environment, SPICE_RS_LANE) {
        return Err(ComposeError::ReservedEnvironment(SPICE_RS_LANE));
    }

    let mut environment = inputs.environment.entries().to_vec();
    environment.push((
        OsString::from(SPICE_RS_LANE),
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
        token: steer::Key::for_identity(inputs.lane).as_str().to_owned(),
    });

    let declared = registry
        .vendor(inputs.vendor.as_str())
        .ok_or_else(|| ComposeError::UnsupportedVendor(inputs.vendor.as_str().to_owned()))?;
    let configured = declared.launch_values(root);
    // Claude alone takes its window from the environment; Codex renders its
    // limit inline and omp reads the overlay the lane wrote.
    if let Some(window) = configured.auto_compact_window_tokens
        && declared.dialect() == listen::Dialect::Claude
        && !environment_contains_entries(&environment, CLAUDE_AUTO_COMPACT_WINDOW)
    {
        environment.push((
            OsString::from(CLAUDE_AUTO_COMPACT_WINDOW),
            OsString::from(window.to_string()),
        ));
    }
    let hook = vendor::SteeringHook {
        command: format!(
            "{} hook {}",
            shell_word(&inputs.executable),
            declared.name()
        ),
    };
    let environment = Environment::new(environment).map_err(ComposeError::Environment)?;

    Ok(Plan {
        vendor: declared,
        binary: configured.binary.to_owned(),
        launch: vendor::Launch {
            cwd: inputs.worktree,
            prompt: inputs.prompt,
            model: inputs.model.unwrap_or_else(|| configured.model.to_owned()),
            effort: inputs.effort.unwrap_or(configured.effort),
            personality: configured.personality.map(str::to_owned),
            fast_mode: configured.fast_mode.map(|_| inputs.fast_mode),
            model_context_window: configured.model_context_window,
            auto_compact_window_tokens: configured.auto_compact_window_tokens,
            config_overlay: inputs.config_overlay,
            attribution,
            hook,
            steering_key,
            mcp: None,
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
                "lane.vendor has no declared extension value; observed {vendor:?}"
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

#[cfg(test)]
mod tests {
    use super::*;

    const LANE: &str = "018f82d4-6f59-7d06-92e3-bce9131163c7";
    const FIXTURE_DIALECT: vendor::Vendor = vendor::Vendor::named("fixture", &vendor::CODEX);
    const FIXTURE_VENDOR: crate::extension::Vendor =
        crate::extension::Vendor::new(&FIXTURE_DIALECT, fixture_configuration, 2);
    const FIXTURE_DECLARATIONS: &[crate::extension::Declaration] =
        &[crate::extension::Declaration {
            namespace: crate::extension::Namespace::Vendor,
            name: "fixture",
            provider: "fixture",
            value: crate::extension::Value::Vendor(&FIXTURE_VENDOR),
        }];

    fn fixture_configuration(root: &Root) -> vendor::LaunchValues<'_> {
        let configured = root.vendor().codex();
        vendor::LaunchValues {
            binary: configured.binary().as_str(),
            model: configured.model(),
            effort: configured.effort(),
            personality: configured.personality(),
            fast_mode: Some(configured.fast_mode()),
            model_context_window: Some(configured.model_context_window()),
            auto_compact_window_tokens: Some(configured.auto_compact_window_tokens()),
        }
    }

    #[test]
    fn a_third_declaration_reaches_launch_without_another_mode_arm() {
        let registry = crate::extension::Registry::discover(FIXTURE_DECLARATIONS)
            .expect("third declaration is coherent");
        let plan = compose_with_registry(
            &Root::default(),
            Inputs {
                worktree: PathBuf::from("/work/tree"),
                vendor: "fixture".parse().expect("fixture vendor name"),
                prompt: "[$spice](.agents/skills/spice/SKILL.md)".to_owned(),
                executable: "spice".to_owned(),
                environment: Environment::empty(),
                lane: LANE.parse().expect("fixture lane"),
                resume: vendor::Resume::Fresh,
                config_overlay: None,
                fast_mode: false,
                model: None,
                effort: None,
            },
            &registry,
        )
        .expect("third declaration composes");

        assert_eq!(plan.vendor().name(), "fixture");
        assert_eq!(plan.launch().hook.command, "'spice' hook fixture");
        assert_eq!(
            plan.record().expect("third declaration renders").dialect(),
            listen::Dialect::Codex
        );
    }

    #[test]
    fn a_lane_selection_governs_over_the_configured_model_and_effort() {
        let registry = crate::extension::Registry::discover(FIXTURE_DECLARATIONS)
            .expect("third declaration is coherent");
        let inputs = |model: Option<&str>, effort| Inputs {
            worktree: PathBuf::from("/work/tree"),
            vendor: "fixture".parse().expect("fixture vendor name"),
            prompt: "[$spice](.agents/skills/spice/SKILL.md)".to_owned(),
            executable: "spice".to_owned(),
            environment: Environment::empty(),
            lane: LANE.parse().expect("fixture lane"),
            resume: vendor::Resume::Fresh,
            config_overlay: None,
            fast_mode: false,
            model: model.map(str::to_owned),
            effort,
        };
        let root = Root::default();
        let configured = compose_with_registry(&root, inputs(None, None), &registry)
            .expect("configured launch composes");
        assert_eq!(configured.launch().model, vendor::DEFAULT_CODEX_MODEL);
        assert_eq!(configured.launch().effort, root.vendor().codex().effort());

        let selected = compose_with_registry(
            &root,
            inputs(Some("gpt-5.5"), Some(vendor::Effort::Low)),
            &registry,
        )
        .expect("selected launch composes");
        assert_eq!(selected.launch().model, "gpt-5.5");
        assert_eq!(selected.launch().effort, vendor::Effort::Low);
    }
}
