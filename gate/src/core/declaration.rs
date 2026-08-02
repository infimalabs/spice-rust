use crate::GateId;
use crate::manifest::PREDECESSOR_GATE_RELEASE;

/// Predecessor source and observation retained for parity review.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PredecessorObservation {
    release: &'static str,
    source: &'static str,
    observation: &'static str,
}

impl PredecessorObservation {
    /// Release that owned the replaced behavior.
    #[must_use]
    pub const fn release(self) -> &'static str {
        self.release
    }

    /// Exact predecessor implementation locus.
    #[must_use]
    pub const fn source(self) -> &'static str {
        self.source
    }

    /// Predecessor configuration or runtime observation being preserved.
    #[must_use]
    pub const fn observation(self) -> &'static str {
        self.observation
    }
}

/// Closed acquisition contract for one compiled core arm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoreToolContract {
    /// Pure Rust acquisition from admitted tree/configuration state.
    Compiled { provider: &'static str },
    /// Bounded Git observation with exact permitted argument vectors.
    Git {
        commands: &'static [GitCommandContract],
        timeout: CoreTimeoutContract,
    },
}

/// One exact permitted `git` invocation and its accepted completion codes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GitCommandContract {
    arguments: &'static [&'static str],
    accepted_exit_codes: &'static [i32],
}

impl GitCommandContract {
    /// Arguments after the fixed `git` program name.
    #[must_use]
    pub const fn arguments(self) -> &'static [&'static str] {
        self.arguments
    }

    /// Exit codes that produce a measured observation for this invocation.
    #[must_use]
    pub const fn accepted_exit_codes(self) -> &'static [i32] {
        self.accepted_exit_codes
    }
}

/// Process deadline contract for external observation acquisition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoreTimeoutContract {
    /// Integration must supply a finite non-zero deadline.
    Required,
}

/// One compiled core-arm declaration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoreArmDeclaration {
    id: GateId,
    tool: CoreToolContract,
    schema: &'static str,
    predecessor: PredecessorObservation,
}

impl CoreArmDeclaration {
    /// Stable current gate identity.
    #[must_use]
    pub const fn id(self) -> GateId {
        self.id
    }

    /// Closed acquisition tool contract.
    #[must_use]
    pub const fn tool(self) -> CoreToolContract {
        self.tool
    }

    /// Stable metric or finding schema name.
    #[must_use]
    pub const fn schema(self) -> &'static str {
        self.schema
    }

    /// Exact predecessor behavior represented by this arm.
    #[must_use]
    pub const fn predecessor(self) -> PredecessorObservation {
        self.predecessor
    }
}

const MERGE_COMMANDS: &[GitCommandContract] = &[
    command(&["rev-parse", "--git-path", "MERGE_HEAD"], &[0]),
    command(&["write-tree"], &[0]),
    command(&["rev-parse", "HEAD^{tree}"], &[0]),
    command(
        &[
            "merge-tree",
            "--write-tree",
            "-z",
            "--name-only",
            "HEAD",
            "MERGE_HEAD",
        ],
        &[0, 1],
    ),
    command(
        &[
            "--literal-pathspecs",
            "ls-tree",
            "-z",
            "--name-only",
            "HEAD",
            "--",
            "<conflicted-paths>",
        ],
        &[0],
    ),
];
const STAGING_COMMANDS: &[GitCommandContract] = &[
    command(
        &["diff", "--cached", "--name-only", "--diff-filter=ACMR"],
        &[0],
    ),
    command(&["diff", "--name-only"], &[0]),
];

const fn command(
    arguments: &'static [&'static str],
    accepted_exit_codes: &'static [i32],
) -> GitCommandContract {
    GitCommandContract {
        arguments,
        accepted_exit_codes,
    }
}

const fn predecessor(source: &'static str, observation: &'static str) -> PredecessorObservation {
    PredecessorObservation {
        release: PREDECESSOR_GATE_RELEASE,
        source,
        observation,
    }
}

const DECLARATIONS: [CoreArmDeclaration; 8] = [
    CoreArmDeclaration {
        id: GateId::MergeIntegrity,
        tool: CoreToolContract::Git {
            commands: MERGE_COMMANDS,
            timeout: CoreTimeoutContract::Required,
        },
        schema: "merge-tree-comparison-v1",
        predecessor: predecessor(
            "spice/hooks/precommit.py::_merge_integrity_diagnostic",
            "MERGE_HEAD, staged tree, HEAD tree, merge-tree result and conflicted paths",
        ),
    },
    CoreArmDeclaration {
        id: GateId::PlanPhase,
        tool: CoreToolContract::Compiled {
            provider: "board active-claim workflow head",
        },
        schema: "active-claim-phase-v1",
        predecessor: predecessor(
            "spice/hooks/precommit.py::_run_plan_phase_mutation_guard",
            "Taskwarrior availability and active claimed task phase",
        ),
    },
    CoreArmDeclaration {
        id: GateId::RepoShape,
        tool: CoreToolContract::Compiled {
            provider: "gate immutable selected tree and admitted shape policy",
        },
        schema: "repo-shape-findings-v1",
        predecessor: predecessor(
            "spice/hooks/precommit.py::_run_shape_guards",
            "package roots, generated paths, namespace, path-shape and name-cluster findings",
        ),
    },
    CoreArmDeclaration {
        id: GateId::Staging,
        tool: CoreToolContract::Git {
            commands: STAGING_COMMANDS,
            timeout: CoreTimeoutContract::Required,
        },
        schema: "partial-staging-paths-v1",
        predecessor: predecessor(
            "spice/hooks/precommit.py::_run_staging_guard",
            "intersection of staged and unstaged repository paths",
        ),
    },
    CoreArmDeclaration {
        id: GateId::RepoDocs,
        tool: CoreToolContract::Compiled {
            provider: "gate immutable selected tree and resolved bound state",
        },
        schema: "repo-doc-character-bounds-v1",
        predecessor: predecessor(
            "spice/studies/repodocs.py::repo_truth_doc_findings",
            "selected doctrine documents, decoded character counts, active caps and peer claims",
        ),
    },
    CoreArmDeclaration {
        id: GateId::ConfigKeyValidity,
        tool: CoreToolContract::Compiled {
            provider: "spice admitted layered configuration",
        },
        schema: "config-admission-findings-v1",
        predecessor: predecessor(
            "spice/studies/configgovernance.py::run_config_key_validity_gate",
            "packaged configuration and every active layered configuration key",
        ),
    },
    CoreArmDeclaration {
        id: GateId::ConfigFalseDisable,
        tool: CoreToolContract::Compiled {
            provider: "spice compiled false-disable registry inventory",
        },
        schema: "config-declaration-consumer-inventory-v1",
        predecessor: predecessor(
            "spice/studies/configgovernance.py::run_false_disable_gate",
            "declared false-disable registry paths, resolver behavior and compiled consumers",
        ),
    },
    CoreArmDeclaration {
        id: GateId::ConfigTrackedTrust,
        tool: CoreToolContract::Compiled {
            provider: "spice compiled tracked-config trust inventory",
        },
        schema: "config-declaration-consumer-inventory-v1",
        predecessor: predecessor(
            "spice/studies/configgovernance.py::run_tracked_file_trust_gate",
            "declared executable repository config paths and compiled approval guards",
        ),
    },
];

/// The eight declarations in fixed predecessor-relative order.
#[must_use]
pub const fn core_declarations() -> &'static [CoreArmDeclaration; 8] {
    &DECLARATIONS
}
