use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

use sill::{Completion, SpecError, WaitOutcome};
use wire::Digest;

use super::{
    AttemptEvidence, AttemptKind, MutantDisposition, MutantEvidence, MutationInventory,
    MutationMovement, MutationToolIdentity, OutcomeEvidence, ProcessCompletion, ProcessEvidence,
    StreamEvidence, SurvivorRate, ToolOutcomes,
};
use crate::jurisdiction::WalkError;

pub(super) fn process_evidence(outcome: &WaitOutcome) -> ProcessEvidence {
    let code = outcome.completion().status().code();
    let completion = match outcome.completion() {
        Completion::Exited(_) => ProcessCompletion::Exited(code),
        Completion::TimedOut(_) => ProcessCompletion::TimedOut(code),
        Completion::Cancelled(_) => ProcessCompletion::Cancelled(code),
    };
    ProcessEvidence {
        completion,
        stdout: outcome.stdout().map(StreamEvidence::from),
        stderr: outcome.stderr().map(StreamEvidence::from),
    }
}

pub(crate) fn decode_attempt(process: &ProcessEvidence, outcome: &OutcomeEvidence) -> AttemptKind {
    match &process.completion {
        ProcessCompletion::TimedOut(_) | ProcessCompletion::Cancelled(_) => AttemptKind::TimedOut,
        ProcessCompletion::LaunchError(_) | ProcessCompletion::SpecError(_) => {
            AttemptKind::LaunchError
        }
        ProcessCompletion::WaitError(_) => AttemptKind::WaitError,
        ProcessCompletion::Exited(_) if !process.outputs_complete() => AttemptKind::Undecodable,
        ProcessCompletion::Exited(code) => decode_outcome(*code, outcome),
    }
}

fn decode_outcome(code: Option<i32>, evidence: &OutcomeEvidence) -> AttemptKind {
    let OutcomeEvidence::Present(bytes) = evidence else {
        return AttemptKind::Undecodable;
    };
    let Ok(outcome) = serde_json::from_slice::<ToolOutcomes>(bytes) else {
        return AttemptKind::Undecodable;
    };
    if outcome.cargo_mutants_version != "27.1.0"
        || outcome.total_mutants != 1
        || outcome.outcomes.len() != 1
    {
        return AttemptKind::Undecodable;
    }
    let counts = (
        outcome.caught,
        outcome.missed,
        outcome.timeout,
        outcome.unviable,
    );
    match (code, outcome.outcomes[0].summary.as_str(), counts) {
        (Some(0), "CaughtMutant", (1, 0, 0, 0)) => AttemptKind::Killed,
        (Some(2), "MissedMutant", (0, 1, 0, 0)) => AttemptKind::Survived,
        (Some(3), "Timeout", (0, 0, 1, 0)) => AttemptKind::TimedOut,
        _ => AttemptKind::Undecodable,
    }
}

pub(super) fn classify(attempts: &[AttemptEvidence]) -> MutantDisposition {
    if attempts
        .iter()
        .all(|attempt| attempt.kind == AttemptKind::Killed)
    {
        MutantDisposition::ConfirmedKilled
    } else if attempts
        .iter()
        .all(|attempt| attempt.kind == AttemptKind::Survived)
    {
        MutantDisposition::ConfirmedSurvived
    } else {
        MutantDisposition::Unconfirmed
    }
}

/// The survivor rate a finished campaign measured, if every mutant was
/// confirmed, and its movement against the committed floor.
pub(super) fn decide(
    mutants: &[MutantEvidence],
    floor: Option<SurvivorRate>,
) -> (Option<SurvivorRate>, MutationMovement) {
    let confirmed = mutants
        .iter()
        .filter(|mutant| mutant.disposition != MutantDisposition::Unconfirmed)
        .count();
    let survivors = mutants
        .iter()
        .filter(|mutant| mutant.disposition == MutantDisposition::ConfirmedSurvived)
        .count();
    let rate = (confirmed == mutants.len()).then(|| {
        SurvivorRate::new(survivors as u64, confirmed as u64)
            .expect("a non-empty confirmed inventory always forms a rate")
    });
    (rate, movement(rate, floor, confirmed, mutants.len()))
}

pub(super) fn movement(
    measured: Option<SurvivorRate>,
    floor: Option<SurvivorRate>,
    confirmed: usize,
    total: usize,
) -> MutationMovement {
    let Some(measured) = measured else {
        return MutationMovement::Unconfirmed { confirmed, total };
    };
    let Some(floor) = floor else {
        return MutationMovement::Unbaselined { measured };
    };
    match measured.compare(floor) {
        std::cmp::Ordering::Less => MutationMovement::Shrank { floor, measured },
        std::cmp::Ordering::Equal => MutationMovement::Held { floor },
        std::cmp::Ordering::Greater => MutationMovement::Regressed { floor, measured },
    }
}

/// Invalid mutation configuration or rational rate.
#[derive(Debug)]
pub enum RateError {
    /// Mutation tool executable was empty.
    EmptyProgram,
    /// A process could otherwise be selected through ambient PATH lookup.
    NonAbsoluteProgram { program: PathBuf },
    /// Mutation tool version identity was empty or could forge a line.
    InvalidToolIdentity { identity: String },
    /// Caller tried to supply an environment key the gate owns.
    ReservedEnvironment { key: String },
    /// No target was selected.
    EmptyTargets,
    /// One target was empty or could forge an inventory line.
    InvalidTarget { target: String },
    /// One target appeared twice.
    DuplicateTarget { target: String },
    /// Retry budget was zero.
    ZeroAttempts,
    /// Outer process runtime bound was zero.
    ZeroRuntime,
    /// Output capture bound was zero.
    ZeroCapture,
    /// A rational rate had no denominator.
    ZeroDenominator,
    /// A rate named more survivors than confirmed mutants.
    SurvivorsExceedConfirmed { survivors: u64, confirmed: u64 },
    /// Sill refused the declared wait policy.
    Spec(SpecError),
}

impl fmt::Display for RateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyProgram => formatter.write_str("mutation tool program is empty"),
            Self::NonAbsoluteProgram { program } => write!(
                formatter,
                "mutation tool program {} is not absolute",
                program.display()
            ),
            Self::InvalidToolIdentity { identity } => {
                write!(formatter, "invalid mutation tool identity {identity:?}")
            }
            Self::ReservedEnvironment { key } => {
                write!(formatter, "mutation environment key {key:?} is gate-owned")
            }
            Self::EmptyTargets => formatter.write_str("mutation target selection is empty"),
            Self::InvalidTarget { target } => {
                write!(formatter, "invalid mutation target {target:?}")
            }
            Self::DuplicateTarget { target } => {
                write!(formatter, "mutation target {target:?} is repeated")
            }
            Self::ZeroAttempts => formatter.write_str("mutation attempts must be positive"),
            Self::ZeroRuntime => formatter.write_str("mutation runtime bound must be positive"),
            Self::ZeroCapture => formatter.write_str("mutation capture bound must be positive"),
            Self::ZeroDenominator => formatter.write_str("survivor rate denominator is zero"),
            Self::SurvivorsExceedConfirmed {
                survivors,
                confirmed,
            } => write!(
                formatter,
                "survivor rate names {survivors} survivors among only {confirmed} confirmed mutants"
            ),
            Self::Spec(source) => source.fmt(formatter),
        }
    }
}

impl Error for RateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Spec(source) => Some(source),
            _ => None,
        }
    }
}

/// Failure before mutation evidence was eligible for scoring.
#[derive(Debug)]
pub enum KillError {
    /// The selected lower-is-better mutation arm had no tracked floor.
    MissingFloor {
        /// Exact absent tracked state path.
        path: PathBuf,
        /// Stable floor name the arm requires.
        name: &'static str,
    },
    /// Selected-tree walking or snapshotting failed.
    Walk(WalkError),
    /// A declared mutation program did not resolve on the running host.
    ToolMissing {
        /// Declared program name.
        program: &'static str,
    },
    /// The resolved mutation programs could not form their process environment.
    ToolEnvironment(String),
    /// One direct mutation executable could not be read for identification.
    ToolRead {
        /// Executable role.
        role: &'static str,
        /// Compiled absolute path.
        program: PathBuf,
        /// Filesystem refusal.
        source: io::Error,
    },
    /// Executable bytes changed while the mutation battery was running.
    ToolIdentityDrift {
        /// Identity observed before tool attestation.
        before: Box<MutationToolIdentity>,
        /// Identity observed after all mutation processes completed.
        after: Box<MutationToolIdentity>,
    },
    /// The pinned tool could not produce complete successful version evidence.
    ToolUnavailable(Box<ProcessEvidence>),
    /// Tool version did not equal the distribution pin.
    ToolDrift { expected: String, observed: String },
    /// Inventory command was unavailable, failed, or incomplete.
    InventoryUnavailable(Box<ProcessEvidence>),
    /// Inventory JSON could not be decoded.
    InventoryUnreadable(String),
    /// Inventory named an invalid mutant.
    InvalidMutant(String),
    /// Structured mutation source was absent or outside the declared targets.
    InvalidMutantSource { mutant: String, source: String },
    /// Inventory named one mutant twice.
    DuplicateMutant(String),
    /// Inventory contained no selected mutants.
    EmptyInventory,
    /// Inventory changed across the unmutated baseline.
    InventoryDrift {
        before: Box<MutationInventory>,
        after: Box<MutationInventory>,
    },
    /// Unmutated baseline was not complete and green.
    BaselineNotGreen(Box<ProcessEvidence>),
    /// Caller tree moved while only private snapshots should have been touched.
    CallerTreeChanged {
        inventory: Digest,
        observed_inventory: Digest,
        tree: Digest,
        observed_tree: Digest,
    },
}

impl fmt::Display for KillError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFloor { path, name } => write!(
                formatter,
                "selected mutation arm requires floor {name:?} in {}",
                path.display()
            ),
            Self::Walk(source) => source.fmt(formatter),
            Self::ToolMissing { program } => write!(
                formatter,
                "required mutation tool {program} is not on the captured executable search path"
            ),
            Self::ToolEnvironment(detail) => {
                write!(formatter, "mutation tool environment was refused: {detail}")
            }
            Self::ToolRead {
                role,
                program,
                source,
            } => write!(
                formatter,
                "direct {role} executable {} could not be identified: {source}",
                program.display()
            ),
            Self::ToolIdentityDrift { before, after } => write!(
                formatter,
                "mutation tool identity changed during execution: {} -> {}",
                before.identity(),
                after.identity()
            ),
            Self::ToolUnavailable(evidence) => {
                write!(
                    formatter,
                    "pinned mutation tool is unavailable: {evidence:?}"
                )
            }
            Self::ToolDrift { expected, observed } => write!(
                formatter,
                "mutation tool drifted: expected {expected:?}, observed {observed:?}"
            ),
            Self::InventoryUnavailable(evidence) => {
                write!(formatter, "mutation inventory is unavailable: {evidence:?}")
            }
            Self::InventoryUnreadable(evidence) => {
                write!(formatter, "mutation inventory is undecodable: {evidence}")
            }
            Self::InvalidMutant(mutant) => write!(formatter, "invalid mutant identity {mutant:?}"),
            Self::InvalidMutantSource { mutant, source } => write!(
                formatter,
                "mutation {mutant:?} names undeclared structured source {source:?}"
            ),
            Self::DuplicateMutant(mutant) => {
                write!(formatter, "mutant identity {mutant:?} is repeated")
            }
            Self::EmptyInventory => formatter.write_str("mutation inventory is empty"),
            Self::InventoryDrift { before, after } => write!(
                formatter,
                "mutation inventory drifted from {:?} to {:?}",
                before.mutants, after.mutants
            ),
            Self::BaselineNotGreen(evidence) => {
                write!(
                    formatter,
                    "unmutated mutation baseline is not green: {evidence:?}"
                )
            }
            Self::CallerTreeChanged {
                inventory,
                observed_inventory,
                tree,
                observed_tree,
            } => write!(
                formatter,
                concat!(
                    "caller tree changed during mutation run: inventory {} -> ",
                    "{}, tree {} -> {}"
                ),
                inventory, observed_inventory, tree, observed_tree
            ),
        }
    }
}

impl Error for KillError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::MissingFloor { .. } => None,
            Self::Walk(source) => Some(source),
            Self::ToolRead { source, .. } => Some(source),
            _ => None,
        }
    }
}
