//! The complete daemonless Doctor report.

use serde::{Deserialize, Serialize};

use crate::ConfigSource;

/// Every ambient variable owned by Spice.
///
/// Doctor reports only whether these names are present. Their values never
/// enter the report.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum SpiceVariable {
    /// Installed shell-hook directory.
    #[serde(rename = "SPICE_HOOKS")] // env-policy: allow
    Hooks,
    /// Rendered lane direction.
    #[serde(rename = "SPICE_INBOX")] // env-policy: allow
    Inbox,
    /// Lane bound to an intercepted shell.
    #[serde(rename = "SPICE_LANE")] // env-policy: allow
    Lane,
    /// Current mutation attempt number.
    #[serde(rename = "SPICE_MUTATION_ATTEMPT")] // env-policy: allow
    MutationAttempt,
    /// Seed repeated into the mutation process.
    #[serde(rename = "SPICE_MUTATION_SEED")] // env-policy: allow
    MutationSeed,
    /// Lane identity proving shell interception activated.
    #[serde(rename = "SPICE_PROOF")] // env-policy: allow
    Proof,
}

impl SpiceVariable {
    /// The complete vocabulary in stable report order.
    pub const ALL: &[Self] = &[
        Self::Hooks,
        Self::Inbox,
        Self::Lane,
        Self::MutationAttempt,
        Self::MutationSeed,
        Self::Proof,
    ];

    /// Exact process-environment spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hooks => "SPICE_HOOKS",                      // env-policy: allow
            Self::Inbox => "SPICE_INBOX",                      // env-policy: allow
            Self::Lane => "SPICE_LANE",                        // env-policy: allow
            Self::MutationAttempt => "SPICE_MUTATION_ATTEMPT", // env-policy: allow
            Self::MutationSeed => "SPICE_MUTATION_SEED",       // env-policy: allow
            Self::Proof => "SPICE_PROOF",                      // env-policy: allow
        }
    }
}

/// Result of one local Doctor observation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DoctorObservation {
    /// The declared local input is available.
    Ready {},
    /// A declared local input is absent or unusable.
    Unavailable {
        /// Exact non-secret reason for the observation.
        reason: String,
    },
    /// The executable can be known only when the caller supplies an operand.
    InputRequired {
        /// Exact input needed before inspection is possible.
        reason: String,
    },
    /// The command declares no external tool to inspect.
    NotApplicable {
        /// Why no tool observation applies.
        reason: String,
    },
    /// A truthful observation would cross Doctor's offline, no-execution law.
    Unsupported {
        /// Boundary preventing the observation.
        reason: String,
    },
}

/// Whether a declared external tool may be launched by its command.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DoctorToolMode {
    /// The owning command may launch the tool.
    Launch,
    /// The owning command observes the integration without launching it.
    Observe,
}

impl DoctorToolMode {
    /// The complete closed vocabulary.
    pub const ALL: &[Self] = &[Self::Launch, Self::Observe];
}

/// Whether a tool must exist for its declaring integration to operate.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DoctorToolRequirement {
    /// The command requires this tool.
    Required,
    /// Absence disables only an optional integration.
    Optional,
}

impl DoctorToolRequirement {
    /// The complete closed vocabulary.
    pub const ALL: &[Self] = &[Self::Required, Self::Optional];
}

/// Authority supplying the exact executable declaration.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DoctorToolSource {
    /// The caller supplies the executable after the command delimiter.
    CommandOperand,
    /// The compiled current Gate policy contract supplies the executable.
    GatePolicyContract,
    /// The compiled current Gate mutation profile supplies the executable.
    GateMutationContract,
    /// The sealed Proof contract supplies the OCI runtime.
    ProofRuntime,
    /// The sealed Proof battery supplies a program inside the container.
    ProofBattery,
    /// A worktree-local integration seam supplies the executable.
    WorktreeConfiguration,
}

impl DoctorToolSource {
    /// The complete closed vocabulary.
    pub const ALL: &[Self] = &[
        Self::CommandOperand,
        Self::GatePolicyContract,
        Self::GateMutationContract,
        Self::ProofRuntime,
        Self::ProofBattery,
        Self::WorktreeConfiguration,
    ];
}

/// Authority owning runtime and output bounds.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DoctorToolBounds {
    /// Spice adds no independent limit around the caller's child.
    ChildOwned,
    /// Gate owns runtime, drain, reap, grace, and capture limits.
    GateContract,
    /// Proof owns runtime, drain, reap, grace, and capture limits.
    ProofLimits,
    /// Observation never starts the tool.
    NotExecuted,
}

impl DoctorToolBounds {
    /// The complete closed vocabulary.
    pub const ALL: &[Self] = &[
        Self::ChildOwned,
        Self::GateContract,
        Self::ProofLimits,
        Self::NotExecuted,
    ];
}

/// Who can stop a declared tool invocation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DoctorToolCancellation {
    /// The child and caller own interruption.
    ChildOwned,
    /// The owning limits terminate and reap at their deadline.
    Deadline,
    /// Observation never starts the tool.
    NotExecuted,
}

impl DoctorToolCancellation {
    /// The complete closed vocabulary.
    pub const ALL: &[Self] = &[Self::ChildOwned, Self::Deadline, Self::NotExecuted];
}

/// Network posture of a declared tool invocation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DoctorToolNetwork {
    /// The caller's child declares its own network behavior.
    ChildOwned,
    /// The owning command does not promise an offline boundary.
    Permitted,
    /// The owning command constructs a network-denied boundary.
    Denied,
    /// Observation never starts the tool.
    NotExecuted,
}

impl DoctorToolNetwork {
    /// The complete closed vocabulary.
    pub const ALL: &[Self] = &[
        Self::ChildOwned,
        Self::Permitted,
        Self::Denied,
        Self::NotExecuted,
    ];
}

/// Compiled target named by the running binary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoctorTarget {
    /// Compiled operating-system family.
    pub os: String,
    /// Compiled processor architecture.
    pub architecture: String,
}

/// Local paths inspected without opening repository authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoctorLocations {
    /// Process working directory.
    pub working: String,
    /// Canonical worktree root.
    pub repository: String,
    /// Canonical Git common directory.
    pub repository_common: String,
    /// Exact tracked configuration location.
    pub tracked_configuration: String,
    /// Exact machine-local configuration location.
    pub local_configuration: String,
}

/// Presence of one known ambient variable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoctorEnvironment {
    /// Closed variable name.
    pub variable: SpiceVariable,
    /// Presence only; the ambient value never crosses.
    pub observation: DoctorObservation,
}

/// Winning source for one effective configuration leaf.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoctorConfigProvenance {
    /// Canonical concrete configuration path.
    pub path: String,
    /// Highest-precedence source that supplied the effective value.
    pub source: ConfigSource,
}

/// One declared external tool and its local observation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoctorTool {
    /// Registry-owned tool name.
    pub name: String,
    /// Component owning the declaration.
    pub owner: String,
    /// Launch or observation posture.
    pub mode: DoctorToolMode,
    /// Required or optional standing.
    pub requirement: DoctorToolRequirement,
    /// Authority supplying the executable.
    pub source: DoctorToolSource,
    /// Runtime and output limit owner.
    pub bounds: DoctorToolBounds,
    /// Cancellation owner.
    pub cancellation: DoctorToolCancellation,
    /// Network posture.
    pub network: DoctorToolNetwork,
    /// Current local observation.
    pub observation: DoctorObservation,
}

/// Tool posture for one generated command leaf.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoctorCommand {
    /// Space-separated generated command path.
    pub command: String,
    /// Aggregate posture, never ready when a declared row is non-ready.
    pub observation: DoctorObservation,
    /// Registry declarations in stable order.
    pub tools: Vec<DoctorTool>,
}

/// Complete successful Doctor answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoctorReport<C> {
    /// Compiled target identity.
    pub target: DoctorTarget,
    /// Working, repository, and configuration paths.
    pub locations: DoctorLocations,
    /// Presence of every known ambient variable.
    pub environment: Vec<DoctorEnvironment>,
    /// Fully admitted effective typed configuration.
    pub configuration: C,
    /// One source row for every concrete effective leaf.
    pub provenance: Vec<DoctorConfigProvenance>,
    /// One deterministic row for every generated command contract.
    pub commands: Vec<DoctorCommand>,
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::{DoctorObservation, DoctorReport, SpiceVariable};

    #[derive(Deserialize)]
    struct WorkspacePolicy {
        policy: EnvironmentPolicy,
    }

    #[derive(Deserialize)]
    struct EnvironmentPolicy {
        env_names: Vec<String>,
    }

    #[test]
    fn observation_objects_reject_unknown_fields_and_variants() {
        for document in [
            br#"{"status":"ready","reason":"not allowed"}"#.as_slice(),
            br#"{"status":"future"}"#.as_slice(),
        ] {
            assert!(
                crate::decode::<DoctorObservation>(document).is_err(),
                "Doctor observations have one closed spelling",
            );
        }
    }

    #[test]
    fn environment_names_round_trip_without_values() {
        for variable in SpiceVariable::ALL {
            let bytes = crate::encode(variable).expect("variable encodes");
            assert_eq!(
                crate::decode::<SpiceVariable>(&bytes).expect("variable decodes"),
                *variable,
            );
            assert_eq!(bytes, format!("\"{}\"", variable.as_str()).as_bytes());
        }
    }

    #[test]
    fn report_environment_rows_are_the_tracked_workspace_vocabulary() {
        let tracked: WorkspacePolicy = toml::from_str(include_str!("../../spice.toml"))
            .expect("tracked workspace policy decodes");
        let declared: Vec<_> = SpiceVariable::ALL
            .iter()
            .map(|variable| variable.as_str())
            .collect();

        assert_eq!(tracked.policy.env_names, declared);
    }

    #[test]
    fn report_structures_reject_unknown_fields_at_every_level() {
        const REPORT: &str = concat!(
            r#"{"target":{"os":"linux","architecture":"x86_64"}"#,
            r#","locations":{"working":"/repo","repository":"/repo""#,
            r#","repository_common":"/repo/.git""#,
            r#","tracked_configuration":"/repo/spice.toml""#,
            r#","local_configuration":"/repo/.git/.spice/config.toml"}"#,
            r#","environment":[{"variable":"SPICE_HOOKS""#, // env-policy: allow
            r#","observation":{"status":"ready"}}]"#,
            r#","configuration":null"#,
            r#","provenance":[{"path":"lane.lease","source":"builtin"}]"#,
            r#","commands":[{"command":"doctor""#,
            r#","observation":{"status":"ready"},"tools":[{"name":"rtk""#,
            r#","owner":"rtk","mode":"observe","requirement":"optional""#,
            r#","source":"worktree-configuration","bounds":"not-executed""#,
            r#","cancellation":"not-executed","network":"not-executed""#,
            r#","observation":{"status":"ready"}}]}]}"#,
        );

        let admitted: DoctorReport<()> =
            crate::decode(REPORT.as_bytes()).expect("complete report decodes");
        assert_eq!(admitted.commands[0].command, "doctor");

        for pointer in [
            "",
            "/target",
            "/locations",
            "/environment/0",
            "/provenance/0",
            "/commands/0",
            "/commands/0/tools/0",
        ] {
            let mut document: serde_json::Value =
                serde_json::from_str(REPORT).expect("fixture is JSON");
            document
                .pointer_mut(pointer)
                .expect("fixture pointer exists")
                .as_object_mut()
                .expect("fixture pointer names an object")
                .insert("future".to_owned(), serde_json::json!(true));
            let bytes = serde_json::to_vec(&document).expect("mutated fixture encodes");
            assert!(
                crate::decode::<DoctorReport<()>>(&bytes).is_err(),
                "unknown field at {pointer:?} crossed",
            );
        }
    }
}
