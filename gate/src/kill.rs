//! Deflaked Rust mutation evidence and lower-is-better survivor debt.
//!
//! The mutation tool never sees the caller's tree. One selected snapshot is
//! attested and inventoried, its unmutated battery must pass, and each mutant
//! then receives a fresh private child snapshot. A mutant contributes debt only
//! when every declared attempt agrees; disagreement and incomplete process
//! evidence remain visible without moving the survivor-rate floor.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::measure::spawn;
use serde::{Deserialize, Serialize};
use sill::{
    Argv, Cancellation, CapturedOutput, Deadlines, Environment, Output, ProcessGroup, Redaction,
    RestartDisposition, Spec, SpecError, StdinPolicy, StreamPolicy, WaitPolicy,
};
use wire::{Digest, Domain};

use crate::jurisdiction::{Ignore, Jurisdiction, Snapshot};

mod support;

pub(crate) use support::decode_attempt;
pub use support::{KillError, RateError};
use support::{classify, decide, process_evidence};

/// Exact build-tool identity already carried by the distribution pin set.
pub const PINNED_CARGO_MUTANTS: &str = "cargo-mutants 27.1.0";

pub(crate) const SEED_ENVIRONMENT: &str = wire::SpiceVariable::MutationSeed.as_str();
pub(crate) const ATTEMPT_ENVIRONMENT: &str = wire::SpiceVariable::MutationAttempt.as_str();
const CARGO_ENVIRONMENT: &str = "CARGO";
const CARGO_BUILD_DIRECTORY_ENVIRONMENT: &str = "CARGO_BUILD_BUILD_DIR";
const CARGO_TARGET_DIRECTORY_ENVIRONMENT: &str = "CARGO_TARGET_DIR";
const MUTATION_BUILD_DIRECTORY: &str = "target/mutation-build";
const MUTATION_TARGET_DIRECTORY: &str = "target/mutation-target";
const OUTPUT_DIRECTORY: &str = ".spice-gate-mutants";
const OUTPUT_ARTIFACT_DIRECTORY: &str = "mutants.out";
const VERSION_ARGUMENTS: [&str; 2] = ["mutants", "--version"];
const BASELINE_ARGUMENTS: [&str; 4] = ["test", "--locked", "--workspace", "--all-targets"];
const INVENTORY_ARGUMENTS: [&str; 5] =
    ["mutants", "--no-config", "--no-shuffle", "--list", "--json"];
const ATTEMPT_ARGUMENTS: [&str; 10] = [
    "mutants",
    "--no-config",
    "--no-shuffle",
    "--in-place",
    "--test-workspace=true",
    "--baseline=skip",
    "--colors=never",
    "--no-times",
    "--cargo-test-arg=--locked",
    "--cargo-test-arg=--all-targets",
];

mod profile;
pub use profile::*;

/// One exact rational survivor rate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SurvivorRate {
    numerator: u64,
    denominator: u64,
}

impl SurvivorRate {
    /// Construct and reduce one rate.
    ///
    /// # Errors
    ///
    /// Refuses a zero denominator or more survivors than confirmed mutants.
    pub fn new(numerator: u64, denominator: u64) -> Result<Self, RateError> {
        if denominator == 0 {
            return Err(RateError::ZeroDenominator);
        }
        if numerator > denominator {
            return Err(RateError::SurvivorsExceedConfirmed {
                survivors: numerator,
                confirmed: denominator,
            });
        }
        let divisor = greatest_common_divisor(numerator, denominator);
        Ok(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    /// Reduced numerator.
    #[must_use]
    pub const fn numerator(self) -> u64 {
        self.numerator
    }

    /// Reduced denominator.
    #[must_use]
    pub const fn denominator(self) -> u64 {
        self.denominator
    }

    fn compare(self, other: Self) -> std::cmp::Ordering {
        (u128::from(self.numerator) * u128::from(other.denominator))
            .cmp(&(u128::from(other.numerator) * u128::from(self.denominator)))
    }
}

fn greatest_common_divisor(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        assert!(
            remainder < right,
            "a Euclidean remainder must strictly decrease"
        );
        (left, right) = (right, remainder);
    }
    left.max(1)
}

/// Canonical tool, target, argument, seed, and mutant inventory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationInventory {
    tool: MutationToolIdentity,
    targets: Vec<String>,
    mutants: Vec<String>,
    mutant_sources: Vec<String>,
    attempts: usize,
    seed: u64,
    timeout_seconds: u64,
}

impl MutationInventory {
    /// Content-bound tool identity observed before attestation and inventory.
    #[must_use]
    pub const fn tool(&self) -> &MutationToolIdentity {
        &self.tool
    }

    /// Selected targets in canonical order.
    #[must_use]
    pub fn targets(&self) -> &[String] {
        &self.targets
    }

    /// Current mutant identities in canonical order.
    #[must_use]
    pub fn mutants(&self) -> &[String] {
        &self.mutants
    }

    /// Exact structured source path for each mutation identity.
    #[must_use]
    pub fn mutant_sources(&self) -> &[String] {
        &self.mutant_sources
    }

    /// Attempts required for every inventoried mutant.
    #[must_use]
    pub const fn attempts(&self) -> usize {
        self.attempts
    }

    /// Seed repeated into every mutation attempt.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// Integer timeout supplied to the pinned mutation tool.
    #[must_use]
    pub const fn timeout_seconds(&self) -> u64 {
        self.timeout_seconds
    }

    /// Canonical text binding the tool and every argument-bearing input.
    #[must_use]
    pub fn render(&self) -> String {
        use std::fmt::Write as _;

        let mut rendered = String::new();
        let _ = writeln!(rendered, "tool {}", self.tool.expected());
        let _ = writeln!(rendered, "tool-declaration {}", self.tool.declaration());
        let _ = writeln!(rendered, "cargo-executable {}", self.tool.cargo());
        let _ = writeln!(rendered, "cargo-mutants-executable {}", self.tool.mutants());
        let _ = writeln!(rendered, "tool-identity {}", self.tool.identity());
        for argument in VERSION_ARGUMENTS {
            let _ = writeln!(rendered, "version-argument {argument}");
        }
        for argument in BASELINE_ARGUMENTS {
            let _ = writeln!(rendered, "baseline-argument {argument}");
        }
        for argument in INVENTORY_ARGUMENTS {
            let _ = writeln!(rendered, "inventory-argument {argument}");
        }
        for argument in ATTEMPT_ARGUMENTS {
            let _ = writeln!(rendered, "attempt-argument {argument}");
        }
        let _ = writeln!(
            rendered,
            "attempt-argument --timeout={}",
            self.timeout_seconds
        );
        let _ = writeln!(rendered, "attempt-argument --output");
        let _ = writeln!(rendered, "attempt-argument {OUTPUT_DIRECTORY}");
        let _ = writeln!(rendered, "attempts {}", self.attempts);
        let _ = writeln!(rendered, "seed {}", self.seed);
        for target in &self.targets {
            let _ = writeln!(rendered, "inventory-argument --file");
            let _ = writeln!(rendered, "inventory-argument {target}");
            let _ = writeln!(rendered, "attempt-argument --file");
            let _ = writeln!(rendered, "attempt-argument {target}");
            let _ = writeln!(rendered, "target {target}");
        }
        for (mutant, source) in self.mutants.iter().zip(&self.mutant_sources) {
            let _ = writeln!(rendered, "mutant {mutant}");
            let _ = writeln!(rendered, "mutant-source {source}");
            let _ = writeln!(rendered, "mutant-argument --re");
            let _ = writeln!(rendered, "mutant-argument {}", exact_regex(mutant));
        }
        rendered
    }
}

/// Captured bytes and the completeness state of one process stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamEvidence {
    /// Bytes retained under the declared capture bound.
    pub bytes: Vec<u8>,
    /// Whether bytes beyond the bound were discarded.
    pub truncated: bool,
    /// Whether the stream reached its end before the drain deadline.
    pub complete: bool,
}

impl From<&CapturedOutput> for StreamEvidence {
    fn from(captured: &CapturedOutput) -> Self {
        Self {
            bytes: captured.bytes().to_vec(),
            truncated: captured.truncated(),
            complete: captured.complete(),
        }
    }
}

/// How a bounded tool process completed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProcessCompletion {
    /// Ordinary process exit, including signals whose portable code is absent.
    Exited(Option<i32>),
    /// Sill terminated the process at its runtime deadline.
    TimedOut(Option<i32>),
    /// Cancellation terminated the process.
    Cancelled(Option<i32>),
    /// The executable could not be launched safely.
    LaunchError(String),
    /// Sill could not finish waiting or reaping.
    WaitError(String),
    /// The fixed process description itself was invalid.
    SpecError(String),
}

/// Complete bounded-process evidence available to the gate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessEvidence {
    /// Completion state.
    pub completion: ProcessCompletion,
    /// Captured standard output, absent only before capture was established.
    pub stdout: Option<StreamEvidence>,
    /// Captured standard error, absent only before capture was established.
    pub stderr: Option<StreamEvidence>,
}

impl ProcessEvidence {
    fn outputs_complete(&self) -> bool {
        [&self.stdout, &self.stderr].into_iter().all(|stream| {
            stream
                .as_ref()
                .is_some_and(|stream| stream.complete && !stream.truncated)
        })
    }

    fn exited(&self, code: i32) -> bool {
        self.completion == ProcessCompletion::Exited(Some(code))
    }
}

/// Meaning of one attempt before retries are combined.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttemptKind {
    /// The mutant made the pinned battery fail.
    Killed,
    /// The pinned battery stayed green.
    Survived,
    /// Either cargo-mutants or sill reached a deadline.
    TimedOut,
    /// The executable could not start safely or its fixed spec was invalid.
    LaunchError,
    /// Waiting or reaping failed.
    WaitError,
    /// Exit or captured output did not prove a cargo-mutants outcome.
    Undecodable,
}

/// One ordered attempt and all evidence it produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptEvidence {
    /// One-based attempt number.
    pub number: usize,
    /// Seed repeated for this attempt.
    pub seed: u64,
    /// Decoded meaning.
    pub kind: AttemptKind,
    /// Complete process evidence under the capture bound.
    pub process: ProcessEvidence,
    /// Raw pinned cargo-mutants outcome artifact or why it was unavailable.
    pub outcome: OutcomeEvidence,
}

/// Evidence obtained from the pinned cargo-mutants `outcomes.json` artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutcomeEvidence {
    /// Exact artifact bytes.
    Present(Vec<u8>),
    /// The tool published no outcome artifact.
    Absent,
    /// The artifact existed but could not be safely replaced or read.
    Unreadable(String),
}

/// What unanimous retry evidence proved about one mutant.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MutantDisposition {
    /// Every attempt killed the mutant.
    ConfirmedKilled,
    /// Every attempt let the mutant survive.
    ConfirmedSurvived,
    /// Attempts disagreed or at least one did not decode completely.
    Unconfirmed,
}

/// One mutant and its complete ordered retry evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutantEvidence {
    /// Stable identity from the current tool inventory.
    pub identity: String,
    /// Exact structured source path from the current tool inventory.
    pub source: String,
    /// Exactly the declared number of attempts, in execution order.
    pub attempts: Vec<AttemptEvidence>,
    /// Classification derived from every attempt.
    pub disposition: MutantDisposition,
}

/// Lower-is-better comparison against the committed survivor floor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationMovement {
    /// Confirmed rate equals the floor.
    Held { floor: SurvivorRate },
    /// Confirmed rate is lower and proposes a smaller floor.
    Shrank {
        floor: SurvivorRate,
        measured: SurvivorRate,
    },
    /// Confirmed rate is higher and changes no floor.
    Regressed {
        floor: SurvivorRate,
        measured: SurvivorRate,
    },
    /// A fully confirmed battery has no committed floor yet.
    Unbaselined { measured: SurvivorRate },
    /// At least one mutant lacked unanimous complete evidence.
    Unconfirmed { confirmed: usize, total: usize },
}

/// A complete mutation run after every preflight succeeded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KillRun {
    source: Digest,
    floor: Option<SurvivorRate>,
    inventory: MutationInventory,
    baseline: ProcessEvidence,
    mutants: Vec<MutantEvidence>,
    rate: Option<SurvivorRate>,
    movement: MutationMovement,
}

impl KillRun {
    /// Exact caller-tree identity proven unchanged across the mutation run.
    #[must_use]
    pub const fn source(&self) -> &Digest {
        &self.source
    }

    /// Committed survivor floor supplied to this run, if one existed.
    #[must_use]
    pub const fn floor(&self) -> Option<SurvivorRate> {
        self.floor
    }

    /// Exact inventory this run consumed.
    #[must_use]
    pub const fn inventory(&self) -> &MutationInventory {
        &self.inventory
    }

    /// Green unmutated baseline evidence.
    #[must_use]
    pub const fn baseline(&self) -> &ProcessEvidence {
        &self.baseline
    }

    /// Mutant evidence in stable inventory order.
    #[must_use]
    pub fn mutants(&self) -> &[MutantEvidence] {
        &self.mutants
    }

    /// Confirmed rate, absent when any mutant is unconfirmed.
    #[must_use]
    pub const fn rate(&self) -> Option<SurvivorRate> {
        self.rate
    }

    /// Comparison against the supplied floor.
    #[must_use]
    pub const fn movement(&self) -> MutationMovement {
        self.movement
    }

    /// Shrink-only floor proposal.
    ///
    /// Regression and unconfirmed evidence deliberately propose nothing, so a
    /// caller cannot accidentally latch either one.
    #[must_use]
    pub const fn proposed_floor(&self) -> Option<SurvivorRate> {
        match self.movement {
            MutationMovement::Held { floor } => Some(floor),
            MutationMovement::Shrank { measured, .. }
            | MutationMovement::Unbaselined { measured } => Some(measured),
            MutationMovement::Regressed { .. } | MutationMovement::Unconfirmed { .. } => None,
        }
    }
}

/// One live observation from an otherwise unchanged mutation campaign.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KillProgress<'a> {
    /// Mutant currently executing, absent while campaign setup completes.
    pub mutant: Option<&'a str>,
    /// Mutants whose complete retry battery has finished.
    pub completed: usize,
    /// Mutants in the stable inventory.
    pub total: usize,
    /// Attempts configured for every mutant.
    pub attempts: usize,
    /// Seed every attempt repeats; with the inventory it names the campaign.
    pub seed: u64,
    /// Final disposition, present only on a mutant-completion observation.
    pub disposition: Option<MutantDisposition>,
}

/// One pinned, bounded mutation gate.
#[derive(Clone, Debug)]
pub struct Kill {
    tool: MutationTool,
    battery: MutationBattery,
    bounds: KillBounds,
}

impl Kill {
    /// Bind the tool, battery, and process bounds used by every stage.
    #[must_use]
    pub const fn new(tool: MutationTool, battery: MutationBattery, bounds: KillBounds) -> Self {
        Self {
            tool,
            battery,
            bounds,
        }
    }

    /// Direct pinned executables and complete explicit environment.
    #[must_use]
    pub const fn tool(&self) -> &MutationTool {
        &self.tool
    }

    /// Compiled mutation targets, retry count, and seed.
    #[must_use]
    pub const fn battery(&self) -> &MutationBattery {
        &self.battery
    }

    /// Compiled mutation process deadlines, grace, and capture.
    #[must_use]
    pub const fn bounds(&self) -> &KillBounds {
        &self.bounds
    }

    /// Run the full preflight and deflaked mutation battery.
    ///
    /// # Errors
    ///
    /// Refuses before scoring when selection, tool attestation, inventory,
    /// baseline, or caller-tree integrity is unavailable or drifted.
    pub fn run(
        &self,
        root: &Path,
        ignore: &Ignore,
        floor: Option<SurvivorRate>,
    ) -> Result<KillRun, KillError> {
        self.run_with_progress(root, ignore, floor, &mut |_| {})
    }

    /// Run the same battery while reporting bounded campaign state.
    ///
    /// The observer has no decision authority. It sees the stable inventory,
    /// each mutant start, and each mutant completion in canonical order.
    ///
    /// # Errors
    ///
    /// Refuses under the same conditions as [`Self::run`].
    pub fn run_with_progress(
        &self,
        root: &Path,
        ignore: &Ignore,
        floor: Option<SurvivorRate>,
        progress: &mut dyn FnMut(KillProgress<'_>),
    ) -> Result<KillRun, KillError> {
        let caller = Jurisdiction::walk(root, ignore).map_err(KillError::Walk)?;
        let caller_inventory = caller.inventory_digest();
        let caller_tree = caller.tree_digest().map_err(KillError::Walk)?;
        let selected = caller.snapshot().map_err(KillError::Walk)?;
        let build_root = selected.jurisdiction().root();
        let tool = self.tool.identify()?;

        let preflight = selected
            .jurisdiction()
            .snapshot()
            .map_err(KillError::Walk)?;
        let preflight_root = preflight.jurisdiction().root();
        self.attest_tool(preflight_root, build_root)?;
        let before = self.inventory(preflight_root, build_root, &tool)?;
        if before.mutants.is_empty() {
            return Err(KillError::EmptyInventory);
        }
        let baseline = self.baseline(preflight_root, build_root)?;
        let after = self.inventory(preflight_root, build_root, &tool)?;
        if before != after {
            return Err(KillError::InventoryDrift {
                before: Box::new(before),
                after: Box::new(after),
            });
        }

        let campaign = KillProgress {
            mutant: None,
            completed: 0,
            total: after.mutants.len(),
            attempts: self.battery.attempts,
            seed: self.battery.seed,
            disposition: None,
        };
        progress(campaign);
        let mut mutants = Vec::with_capacity(campaign.total);
        for (index, (identity, source)) in
            after.mutants.iter().zip(&after.mutant_sources).enumerate()
        {
            progress(KillProgress {
                mutant: Some(identity),
                completed: index,
                ..campaign
            });
            let mutant = self.mutant(&selected, build_root, identity, source)?;
            let disposition = mutant.disposition;
            mutants.push(mutant);
            progress(KillProgress {
                mutant: Some(identity),
                completed: index + 1,
                disposition: Some(disposition),
                ..campaign
            });
        }

        let observed = Jurisdiction::walk(root, ignore).map_err(KillError::Walk)?;
        let observed_inventory = observed.inventory_digest();
        let observed_tree = observed.tree_digest().map_err(KillError::Walk)?;
        if caller_inventory != observed_inventory || caller_tree != observed_tree {
            return Err(KillError::CallerTreeChanged {
                inventory: caller_inventory,
                observed_inventory,
                tree: caller_tree,
                observed_tree,
            });
        }
        let observed_tool = self.tool.identify()?;
        if tool != observed_tool {
            return Err(KillError::ToolIdentityDrift {
                before: Box::new(tool),
                after: Box::new(observed_tool),
            });
        }

        let (rate, movement) = decide(&mutants, floor);

        Ok(KillRun {
            source: caller_tree,
            floor,
            inventory: after,
            baseline,
            mutants,
            rate,
            movement,
        })
    }

    /// Run every declared attempt of one mutant in its own private snapshot.
    fn mutant(
        &self,
        selected: &Snapshot,
        build_root: &Path,
        identity: &str,
        source: &str,
    ) -> Result<MutantEvidence, KillError> {
        let mut attempts = Vec::with_capacity(self.battery.attempts);
        for number in 1..=self.battery.attempts {
            let private = selected
                .jurisdiction()
                .snapshot()
                .map_err(KillError::Walk)?;
            attempts.push(self.attempt(
                private.jurisdiction().root(),
                build_root,
                identity,
                number,
            ));
        }
        Ok(MutantEvidence {
            identity: identity.to_owned(),
            source: source.to_owned(),
            disposition: classify(&attempts),
            attempts,
        })
    }

    fn attest_tool(&self, root: &Path, build_root: &Path) -> Result<(), KillError> {
        let evidence = self.process(
            self.tool.mutants_program(),
            root,
            build_root,
            VERSION_ARGUMENTS,
            None,
        );
        if !evidence.exited(0) || !evidence.outputs_complete() {
            return Err(KillError::ToolUnavailable(Box::new(evidence)));
        }
        let stdout = evidence
            .stdout
            .as_ref()
            .expect("complete evidence has stdout");
        let observed = std::str::from_utf8(&stdout.bytes)
            .map(str::trim)
            .unwrap_or("<non-utf8>");
        if observed != self.tool.identity() {
            return Err(KillError::ToolDrift {
                expected: self.tool.identity().to_owned(),
                observed: observed.to_owned(),
            });
        }
        Ok(())
    }

    fn inventory(
        &self,
        root: &Path,
        build_root: &Path,
        tool: &MutationToolIdentity,
    ) -> Result<MutationInventory, KillError> {
        let arguments = inventory_arguments(&self.battery.targets);
        let evidence = self.process(
            self.tool.mutants_program(),
            root,
            build_root,
            arguments,
            None,
        );
        if !evidence.exited(0) || !evidence.outputs_complete() {
            return Err(KillError::InventoryUnavailable(Box::new(evidence)));
        }
        let stdout = evidence
            .stdout
            .as_ref()
            .expect("complete evidence has stdout");
        let listed: Vec<ListedMutant> = serde_json::from_slice(&stdout.bytes)
            .map_err(|source| KillError::InventoryUnreadable(source.to_string()))?;
        let mut listed = listed;
        for mutant in &mut listed {
            if mutant.name.is_empty() || mutant.name.contains(['\n', '\r']) {
                return Err(KillError::InvalidMutant(mutant.name.clone()));
            }
            if !self.battery.targets.contains(&mutant.file) {
                return Err(KillError::InvalidMutantSource {
                    mutant: mutant.name.clone(),
                    source: mutant.file.clone(),
                });
            }
        }
        listed.sort_by(|left, right| left.name.cmp(&right.name));
        if let Some(pair) = listed.windows(2).find(|pair| pair[0].name == pair[1].name) {
            return Err(KillError::DuplicateMutant(pair[0].name.clone()));
        }
        let (mutants, mutant_sources) = listed
            .into_iter()
            .map(|listed| (listed.name, listed.file))
            .unzip();
        Ok(MutationInventory {
            tool: tool.clone(),
            targets: self.battery.targets.clone(),
            mutants,
            mutant_sources,
            attempts: self.battery.attempts,
            seed: self.battery.seed,
            timeout_seconds: self.bounds.timeout_seconds(),
        })
    }

    fn baseline(&self, root: &Path, build_root: &Path) -> Result<ProcessEvidence, KillError> {
        let evidence = self.process(
            self.tool.cargo_program(),
            root,
            build_root,
            BASELINE_ARGUMENTS,
            None,
        );
        if evidence.exited(0) && evidence.outputs_complete() {
            Ok(evidence)
        } else {
            Err(KillError::BaselineNotGreen(Box::new(evidence)))
        }
    }

    fn attempt(
        &self,
        root: &Path,
        build_root: &Path,
        identity: &str,
        number: usize,
    ) -> AttemptEvidence {
        if let Err(source) = make_mutation_targets_writable(root, &self.battery.targets) {
            return attempt_launch_error(
                number,
                self.battery.seed,
                format!(
                    "preparing private mutation targets below {}",
                    root.display()
                ),
                source,
            );
        }
        let output = root.join(OUTPUT_DIRECTORY);
        if let Err(source) = remove_existing_output(&output) {
            return attempt_launch_error(
                number,
                self.battery.seed,
                format!("refusing stale mutation output at {}", output.display()),
                source,
            );
        }
        let arguments = attempt_arguments(
            &self.battery.targets,
            identity,
            self.bounds.timeout_seconds(),
            Path::new(OUTPUT_DIRECTORY),
        );
        let process = self.process(
            self.tool.mutants_program(),
            root,
            build_root,
            arguments,
            Some(number),
        );
        let outcome = match fs::read(output.join(OUTPUT_ARTIFACT_DIRECTORY).join("outcomes.json")) {
            Ok(bytes) => OutcomeEvidence::Present(bytes),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => OutcomeEvidence::Absent,
            Err(source) => OutcomeEvidence::Unreadable(source.to_string()),
        };
        let kind = decode_attempt(&process, &outcome);
        AttemptEvidence {
            number,
            seed: self.battery.seed,
            kind,
            process,
            outcome,
        }
    }

    fn process(
        &self,
        program: &Path,
        root: &Path,
        build_root: &Path,
        arguments: impl IntoIterator<Item = impl Into<OsString>>,
        attempt: Option<usize>,
    ) -> ProcessEvidence {
        let spec = self.spec(program, root, build_root, arguments, attempt);
        let spec = match spec {
            Ok(spec) => spec,
            Err(source) => {
                return ProcessEvidence {
                    completion: ProcessCompletion::SpecError(source.to_string()),
                    stdout: None,
                    stderr: None,
                };
            }
        };
        let running = match spawn(spec) {
            Ok(running) => running,
            Err(source) => {
                return ProcessEvidence {
                    completion: ProcessCompletion::LaunchError(source.to_string()),
                    stdout: None,
                    stderr: None,
                };
            }
        };
        match running.wait() {
            Ok(outcome) => process_evidence(&outcome),
            Err(source) => ProcessEvidence {
                completion: ProcessCompletion::WaitError(source.to_string()),
                stdout: None,
                stderr: None,
            },
        }
    }

    fn spec(
        &self,
        program: &Path,
        root: &Path,
        build_root: &Path,
        arguments: impl IntoIterator<Item = impl Into<OsString>>,
        attempt: Option<usize>,
    ) -> Result<Spec, SpecError> {
        Ok(Spec {
            argv: Argv::new(program.as_os_str(), arguments)?,
            cwd: root.to_path_buf(),
            environment: self
                .tool
                .environment(build_root, self.battery.seed, attempt)?,
            group: ProcessGroup::New,
            deadlines: self.bounds.deadlines,
            wait: self.bounds.wait.clone(),
            output: Output::new(
                StdinPolicy::Null,
                StreamPolicy::Capture {
                    limit: self.bounds.capture,
                },
                StreamPolicy::Capture {
                    limit: self.bounds.capture,
                },
            ),
            redaction: Redaction::None,
            grace: self.bounds.grace,
        })
    }
}

fn attempt_launch_error(
    number: usize,
    seed: u64,
    context: String,
    source: io::Error,
) -> AttemptEvidence {
    AttemptEvidence {
        number,
        seed,
        kind: AttemptKind::LaunchError,
        process: ProcessEvidence {
            completion: ProcessCompletion::LaunchError(format!("{context}: {source}")),
            stdout: None,
            stderr: None,
        },
        outcome: OutcomeEvidence::Unreadable(source.to_string()),
    }
}

fn make_mutation_targets_writable(root: &Path, targets: &[String]) -> io::Result<()> {
    for target in targets {
        let path = root.join(target);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "mutation target is not one regular file: {}",
                    path.display()
                ),
            ));
        }
        #[cfg(target_family = "unix")]
        {
            use std::os::unix::fs::PermissionsExt as _;

            let mut permissions = metadata.permissions();
            permissions.set_mode(permissions.mode() | 0o200);
            fs::set_permissions(&path, permissions)?;
        }
        #[cfg(not(target_family = "unix"))]
        crate::permissions::set_owner_writable(&path, false)?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct ListedMutant {
    name: String,
    #[serde(default)]
    file: String,
}

#[derive(Deserialize)]
struct ToolOutcomes {
    cargo_mutants_version: String,
    total_mutants: usize,
    missed: usize,
    caught: usize,
    timeout: usize,
    unviable: usize,
    outcomes: Vec<ToolOutcome>,
}

#[derive(Deserialize)]
struct ToolOutcome {
    summary: String,
}

fn inventory_arguments(targets: &[String]) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = INVENTORY_ARGUMENTS.map(OsString::from).to_vec();
    for target in targets {
        arguments.push("--file".into());
        arguments.push(target.into());
    }
    arguments
}

fn attempt_arguments(
    targets: &[String],
    identity: &str,
    timeout_seconds: u64,
    output: &Path,
) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = ATTEMPT_ARGUMENTS.map(OsString::from).to_vec();
    arguments.push(format!("--timeout={timeout_seconds}").into());
    arguments.push("--output".into());
    arguments.push(output.as_os_str().to_owned());
    arguments.push("--re".into());
    arguments.push(exact_regex(identity).into());
    for target in targets {
        arguments.push("--file".into());
        arguments.push(target.into());
    }
    arguments
}

fn remove_existing_output(output: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(output) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(source),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(output)
    } else {
        fs::remove_file(output)
    }
}

fn exact_regex(identity: &str) -> String {
    let mut escaped = String::with_capacity(identity.len() + 2);
    escaped.push('^');
    for character in identity.chars() {
        if matches!(
            character,
            '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' | '\\'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('$');
    escaped
}
