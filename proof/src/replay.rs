//! Exhaustive sealed replay of the generated proof battery.

use std::error::Error;
use std::fmt;

use wire::manifest::{
    PROOF_RECEIPT_VERSION, ProofEnvironment, ProofReceipt, ProofStanding, ReleaseTarget,
    TargetPayload,
};
use wire::{Digest, Domain};

use crate::container::{Execution, ExecutionCompletion};
use crate::environment::SealedEnvironment;

/// The complete generated proof battery, in execution order.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CheckKind {
    Gate,
    MutationCorpus,
    DialectRefusals,
    WaitAndProcess,
    SupplyRebuild,
}

impl CheckKind {
    /// Every required member in deterministic execution order.
    pub const ALL: [Self; 5] = [
        Self::Gate,
        Self::MutationCorpus,
        Self::DialectRefusals,
        Self::WaitAndProcess,
        Self::SupplyRebuild,
    ];

    /// Stable spelling mixed into battery and replay identities.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gate => "gate",
            Self::MutationCorpus => "mutation-corpus",
            Self::DialectRefusals => "dialect-refusals",
            Self::WaitAndProcess => "wait-and-process",
            Self::SupplyRebuild => "supply-rebuild",
        }
    }

    /// Whether this member announces the corpus it selected before testing it.
    ///
    /// Only the mutation member can select nothing and still exit clean. Every
    /// other member names one package and one target, and cargo refuses an
    /// unknown name outright rather than running an empty selection.
    const fn announces_a_corpus(self) -> bool {
        matches!(self, Self::MutationCorpus)
    }
}

/// One explicit command in the generated battery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Check {
    kind: CheckKind,
    program: String,
    arguments: Vec<String>,
}

impl Check {
    fn current(kind: CheckKind) -> Self {
        let arguments = match kind {
            CheckKind::Gate => ["test", "--locked", "-p", "gate", "--all-targets"].as_slice(),
            // Scoped to the one package `SupplyRebuild` already runs, so the two
            // members ask complementary questions about a single corpus: whether
            // the pipeline passes, and whether it would notice if it were wrong.
            // The scope is what makes the member fit its bound. `--workspace`
            // enumerates 14161 mutants and can never finish inside the runtime
            // every member is given, while this corpus is fully killed in
            // seconds, and it drops the worktree the wider corpus required from
            // a copy cargo-mutants takes without one.
            CheckKind::MutationCorpus => ["mutants", "-p", "dist"].as_slice(),
            CheckKind::DialectRefusals => {
                ["test", "--locked", "-p", "listen", "--test", "corpus"].as_slice()
            }
            CheckKind::WaitAndProcess => ["test", "--locked", "-p", "sill", "--lib"].as_slice(),
            CheckKind::SupplyRebuild => {
                ["test", "--locked", "-p", "dist", "--test", "pipeline"].as_slice()
            }
        };
        Self {
            kind,
            program: "cargo".to_owned(),
            arguments: arguments
                .iter()
                .map(|argument| (*argument).to_owned())
                .collect(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> CheckKind {
        self.kind
    }

    #[must_use]
    pub fn program(&self) -> &str {
        &self.program
    }

    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }
}

/// Exactly one command for every required proof class.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Battery {
    checks: Vec<Check>,
    digest: Digest,
}

impl Battery {
    /// The sole compiled battery, generated exhaustively from [`CheckKind::ALL`].
    #[must_use]
    pub fn current() -> Self {
        let checks = CheckKind::ALL
            .into_iter()
            .map(Check::current)
            .collect::<Vec<_>>();
        let digest = battery_digest(&checks);
        Self { checks, digest }
    }

    /// Ordered commands the replay will consume.
    #[must_use]
    pub fn checks(&self) -> &[Check] {
        &self.checks
    }

    /// Identity of every command and argument in the complete inventory.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }
}

/// One member's typed result and complete bounded output evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckVerdict {
    kind: CheckKind,
    standing: CheckStanding,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl CheckVerdict {
    #[must_use]
    pub const fn kind(&self) -> CheckKind {
        self.kind
    }

    #[must_use]
    pub const fn standing(&self) -> CheckStanding {
        self.standing
    }

    #[must_use]
    pub fn stdout(&self) -> &[u8] {
        &self.stdout
    }

    #[must_use]
    pub fn stderr(&self) -> &[u8] {
        &self.stderr
    }
}

/// Exit code for a member whose scope selected nothing.
///
/// The original spice gritctl exited 5 when its scope selected nothing, after
/// pytest's 5 for "no tests collected", and this member keeps that meaning. It
/// is a standing of its own rather than a [`CheckStanding::Failed`] code
/// because the pinned cargo-mutants already spends 5 on `FilterDiffMismatch`,
/// and an empty corpus has to stay distinguishable from a real failure just as
/// much as from a clean run.
const EMPTY_SCOPE: i32 = 5;

/// Why one declared battery member did or did not pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckStanding {
    Clean,
    EmptyScope,
    Failed { code: Option<i32> },
    TimedOut { code: Option<i32> },
    Cancelled { code: Option<i32> },
    OutputTruncated,
    OutputIncomplete,
}

impl CheckStanding {
    #[must_use]
    pub const fn passes(self) -> bool {
        matches!(self, Self::Clean)
    }

    /// Shared gate standing for this proof member.
    #[must_use]
    pub const fn gate_standing(self) -> gate::Standing {
        if self.passes() {
            gate::Standing::Clean
        } else {
            gate::Standing::Refused
        }
    }

    /// Stable spelling mixed into replay identities and shown to operators.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::EmptyScope => "empty-scope",
            Self::Failed { .. } => "failed",
            Self::TimedOut { .. } => "timed-out",
            Self::Cancelled { .. } => "cancelled",
            Self::OutputTruncated => "output-truncated",
            Self::OutputIncomplete => "output-incomplete",
        }
    }

    /// Exit code this standing reports, which for an empty scope is the
    /// conventional 5 for selecting nothing rather than the 0 the tool exited.
    #[must_use]
    pub const fn code(self) -> Option<i32> {
        match self {
            Self::EmptyScope => Some(EMPTY_SCOPE),
            Self::Failed { code } | Self::TimedOut { code } | Self::Cancelled { code } => code,
            Self::Clean | Self::OutputTruncated | Self::OutputIncomplete => None,
        }
    }
}

/// One typed verdict over the complete battery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayVerdict {
    target: ReleaseTarget,
    environment: ProofEnvironment,
    battery: Digest,
    replay: Digest,
    standing: ProofStanding,
    checks: Vec<CheckVerdict>,
}

impl ReplayVerdict {
    #[must_use]
    pub const fn target(&self) -> ReleaseTarget {
        self.target
    }

    #[must_use]
    pub const fn environment(&self) -> &ProofEnvironment {
        &self.environment
    }

    #[must_use]
    pub const fn battery_digest(&self) -> &Digest {
        &self.battery
    }

    #[must_use]
    pub const fn replay_digest(&self) -> &Digest {
        &self.replay
    }

    #[must_use]
    pub const fn standing(&self) -> ProofStanding {
        self.standing
    }

    #[must_use]
    pub fn checks(&self) -> &[CheckVerdict] {
        &self.checks
    }

    /// Bind this complete replay to one exact receipt-free target payload.
    pub fn receipt(
        &self,
        payload: &TargetPayload,
        payload_digest: Digest,
    ) -> Result<ProofReceipt, ReceiptError> {
        if payload.target != self.target {
            return Err(ReceiptError {
                payload: payload.target,
                replay: self.target,
            });
        }
        Ok(ProofReceipt {
            version: PROOF_RECEIPT_VERSION,
            target: self.target,
            payload: payload_digest,
            battery: self.battery.clone(),
            replay: self.replay.clone(),
            environment: self.environment.clone(),
            standing: self.standing,
        })
    }

    #[cfg(test)]
    pub(crate) fn fixture_clean(
        target: ReleaseTarget,
        environment: ProofEnvironment,
        battery: Digest,
    ) -> Self {
        Self::fixture_with(
            target,
            environment,
            battery,
            [CheckStanding::Clean; CheckKind::ALL.len()],
        )
    }

    #[cfg(test)]
    pub(crate) fn fixture_with(
        target: ReleaseTarget,
        environment: ProofEnvironment,
        battery: Digest,
        standings: [CheckStanding; CheckKind::ALL.len()],
    ) -> Self {
        let checks = CheckKind::ALL
            .into_iter()
            .zip(standings)
            .map(|(kind, standing)| CheckVerdict {
                kind,
                standing,
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
            .collect::<Vec<_>>();
        let standing = if checks.iter().all(|check| check.standing.passes()) {
            ProofStanding::Clean
        } else {
            ProofStanding::Refused
        };
        let replay = replay_digest(target, &environment, &battery, standing, &checks);
        Self {
            target,
            environment,
            battery,
            replay,
            standing,
            checks,
        }
    }
}

/// Execute the complete current battery through one sealed environment.
///
/// # Errors
///
/// Returns the exact member whose container process could not be established
/// or observed. Ordinary nonzero exits and timeouts remain inside the one
/// typed [`ReplayVerdict`].
pub fn replay(environment: &SealedEnvironment) -> Result<ReplayVerdict, ReplayError> {
    run_with(environment, &Battery::current())
}

trait Executor {
    fn target(&self) -> ReleaseTarget;
    fn environment(&self) -> ProofEnvironment;
    fn execute(&self, check: &Check) -> Result<Execution, String>;
}

impl Executor for SealedEnvironment {
    fn target(&self) -> ReleaseTarget {
        self.target()
    }

    fn environment(&self) -> ProofEnvironment {
        self.identity()
    }

    fn execute(&self, check: &Check) -> Result<Execution, String> {
        self.execute(check.program(), check.arguments())
    }
}

fn run_with<E: Executor>(executor: &E, battery: &Battery) -> Result<ReplayVerdict, ReplayError> {
    let mut checks = Vec::with_capacity(battery.checks.len());
    for check in &battery.checks {
        let execution = executor.execute(check).map_err(|source| ReplayError {
            check: check.kind,
            source,
        })?;
        checks.push(classify(check.kind, execution));
    }
    let standing = if checks.iter().all(|check| check.standing.passes()) {
        ProofStanding::Clean
    } else {
        ProofStanding::Refused
    };
    let target = executor.target();
    let environment = executor.environment();
    let replay = replay_digest(target, &environment, battery.digest(), standing, &checks);
    Ok(ReplayVerdict {
        target,
        environment,
        battery: battery.digest.clone(),
        replay,
        standing,
        checks,
    })
}

fn classify(kind: CheckKind, execution: Execution) -> CheckVerdict {
    let standing = match execution.completion {
        ExecutionCompletion::Exited {
            success: false,
            code,
        } => CheckStanding::Failed { code },
        ExecutionCompletion::TimedOut { code } => CheckStanding::TimedOut { code },
        ExecutionCompletion::Cancelled { code } => CheckStanding::Cancelled { code },
        ExecutionCompletion::Exited { success: true, .. }
            if execution.stdout.truncated || execution.stderr.truncated =>
        {
            CheckStanding::OutputTruncated
        }
        ExecutionCompletion::Exited { success: true, .. }
            if !execution.stdout.complete || !execution.stderr.complete =>
        {
            CheckStanding::OutputIncomplete
        }
        ExecutionCompletion::Exited { success: true, .. }
            if kind.announces_a_corpus() && !proved_a_corpus(&execution.stdout.bytes) =>
        {
            CheckStanding::EmptyScope
        }
        ExecutionCompletion::Exited { success: true, .. } => CheckStanding::Clean,
    };
    CheckVerdict {
        kind,
        standing,
        stdout: execution.stdout.bytes,
        stderr: execution.stderr.bytes,
    }
}

/// Whether a clean mutation run proved it had a corpus to test.
///
/// The pinned cargo-mutants announces its corpus on stdout before testing
/// anything, and an empty scope is not an error to it: measured on 27.1.0,
/// `cargo mutants -p dist -f dist/src/no-such-file.rs` prints `Found 0 mutants
/// to test`, warns on stderr, and exits 0, exactly as the real member exits 0
/// after `Found 36 mutants to test`. The announced count is the only place the
/// two differ. A run that announced no count at all proved no corpus either,
/// and is refused for the same reason zero is: the member would otherwise pass
/// while proving nothing, and the receipt would still name it.
fn proved_a_corpus(stdout: &[u8]) -> bool {
    String::from_utf8_lossy(stdout).lines().any(|line| {
        line.strip_prefix("Found ")
            .and_then(|announced| announced.strip_suffix(" to test"))
            // Counted positionally so the tool's own pluralisation of the noun
            // after it is never a spelling this reading has to know.
            .and_then(|announced| announced.split_whitespace().next())
            .and_then(|count| count.parse::<u64>().ok())
            .is_some_and(|count| count > 0)
    })
}

fn battery_digest(checks: &[Check]) -> Digest {
    let mut bytes = Vec::new();
    frame_usize(&mut bytes, checks.len());
    for check in checks {
        frame_bytes(&mut bytes, check.kind.as_str().as_bytes());
        frame_bytes(&mut bytes, check.program.as_bytes());
        frame_usize(&mut bytes, check.arguments.len());
        for argument in &check.arguments {
            frame_bytes(&mut bytes, argument.as_bytes());
        }
    }
    Digest::compute(Domain::ProofBattery, &bytes)
}

fn replay_digest(
    target: ReleaseTarget,
    environment: &ProofEnvironment,
    battery: &Digest,
    standing: ProofStanding,
    checks: &[CheckVerdict],
) -> Digest {
    let mut bytes = Vec::new();
    frame_bytes(&mut bytes, target.as_str().as_bytes());
    frame_bytes(
        &mut bytes,
        &wire::encode(environment).expect("ProofEnvironment always has canonical wire bytes"),
    );
    frame_bytes(&mut bytes, battery.as_str().as_bytes());
    frame_bytes(
        &mut bytes,
        match standing {
            ProofStanding::Clean => b"clean",
            ProofStanding::Refused => b"refused",
        },
    );
    frame_usize(&mut bytes, checks.len());
    for check in checks {
        frame_bytes(&mut bytes, check.kind.as_str().as_bytes());
        frame_bytes(&mut bytes, check.standing.as_str().as_bytes());
        match check.standing.code() {
            Some(code) => {
                bytes.push(1);
                bytes.extend_from_slice(&code.to_be_bytes());
            }
            None => bytes.push(0),
        }
        frame_bytes(&mut bytes, &check.stdout);
        frame_bytes(&mut bytes, &check.stderr);
    }
    Digest::compute(Domain::ProofReplayV2, &bytes)
}

fn frame_usize(bytes: &mut Vec<u8>, value: usize) {
    bytes.extend_from_slice(&(value as u64).to_be_bytes());
}

fn frame_bytes(bytes: &mut Vec<u8>, value: &[u8]) {
    frame_usize(bytes, value.len());
    bytes.extend_from_slice(value);
}

/// Infrastructure failure that prevented one complete replay verdict.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayError {
    check: CheckKind,
    source: String,
}

impl ReplayError {
    #[must_use]
    pub const fn check(&self) -> CheckKind {
        self.check
    }

    #[must_use]
    pub fn evidence(&self) -> &str {
        &self.source
    }

    #[cfg(test)]
    pub(crate) fn fixture(check: CheckKind, source: impl Into<String>) -> Self {
        Self {
            check,
            source: source.into(),
        }
    }
}

impl fmt::Display for ReplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "sealed {} battery member could not execute: {}",
            self.check.as_str(),
            self.source
        )
    }
}

impl Error for ReplayError {}

/// A replay cannot mint a receipt for another release target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceiptError {
    payload: ReleaseTarget,
    replay: ReleaseTarget,
}

impl fmt::Display for ReceiptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cannot bind {} replay to {} target payload",
            self.replay, self.payload
        )
    }
}

impl Error for ReceiptError {}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use wire::manifest::{ProofEnvironment, ReleaseTarget};

    use super::{Battery, Check, CheckKind, CheckStanding, Executor, ProofStanding, run_with};
    use crate::container::{Captured, Execution, ExecutionCompletion};

    #[test]
    fn the_current_battery_pins_every_member_and_command_in_order() {
        let battery = Battery::current();
        let commands = battery
            .checks()
            .iter()
            .map(|check| {
                (
                    check.kind(),
                    check.program(),
                    check
                        .arguments()
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            commands,
            [
                (
                    CheckKind::Gate,
                    "cargo",
                    vec!["test", "--locked", "-p", "gate", "--all-targets"]
                ),
                (
                    CheckKind::MutationCorpus,
                    "cargo",
                    vec!["mutants", "-p", "dist"]
                ),
                (
                    CheckKind::DialectRefusals,
                    "cargo",
                    vec!["test", "--locked", "-p", "listen", "--test", "corpus"]
                ),
                (
                    CheckKind::WaitAndProcess,
                    "cargo",
                    vec!["test", "--locked", "-p", "sill", "--lib"]
                ),
                (
                    CheckKind::SupplyRebuild,
                    "cargo",
                    vec!["test", "--locked", "-p", "dist", "--test", "pipeline"]
                ),
            ]
        );
    }

    /// One clean mutation run, in the words the pinned cargo-mutants uses.
    const MUTATION_EVIDENCE: &[u8] = concat!(
        "Found 36 mutants to test\n",
        "ok       Unmutated baseline in 7s build + 0s test\n",
        "36 mutants tested in 22s: 12 caught, 24 unviable\n",
    )
    .as_bytes();

    /// The same clean exit after the scope stopped selecting code.
    const EMPTY_CORPUS_EVIDENCE: &[u8] = b"Found 0 mutants to test\n";

    #[test]
    fn replay_consumes_every_member_and_returns_one_typed_verdict() {
        let battery = Battery::current();
        let executor = FakeExecutor::new(vec![
            exited(true, b"gate evidence"),
            exited(true, MUTATION_EVIDENCE),
            exited(true, b"dialect evidence"),
            exited(true, b"process evidence"),
            exited(true, b"supply evidence"),
        ]);

        let verdict = run_with(&executor, &battery).expect("complete replay");

        assert_eq!(verdict.standing(), ProofStanding::Clean);
        assert_eq!(verdict.checks().len(), CheckKind::ALL.len());
        assert_eq!(executor.ran.borrow().as_slice(), CheckKind::ALL);
        assert_eq!(verdict.battery_digest(), battery.digest());
        assert_eq!(verdict.environment(), &environment());
        assert_eq!(verdict.target(), ReleaseTarget::X86_64UnknownLinuxMusl);
    }

    #[test]
    fn a_refused_gate_and_a_later_failure_remain_in_the_one_complete_verdict() {
        let battery = Battery::current();
        let executor = FakeExecutor::new(vec![
            exited(false, b"gate failed"),
            exited(true, MUTATION_EVIDENCE),
            exited(false, b"dialect failed"),
            exited(true, b"process evidence"),
            exited(true, b"supply evidence"),
        ]);

        let verdict = run_with(&executor, &battery).expect("typed refused replay");

        assert_eq!(verdict.standing(), ProofStanding::Refused);
        assert_eq!(
            verdict.checks()[0].standing(),
            CheckStanding::Failed { code: Some(1) }
        );
        assert_eq!(
            verdict.checks()[0].standing().gate_standing(),
            gate::Standing::Refused
        );
        assert_eq!(
            verdict.checks()[2].standing(),
            CheckStanding::Failed { code: Some(1) }
        );
        assert_eq!(executor.ran.borrow().as_slice(), CheckKind::ALL);
    }

    /// An empty corpus is a clean exit, and it proves nothing.
    ///
    /// cargo-mutants prints this and exits 0 the moment the member's scope
    /// stops selecting code, so a renamed or folded package leaves every
    /// member clean and the whole battery clean with it, while nothing was
    /// mutated at all and the receipt still names the member.
    #[test]
    fn a_mutation_member_that_selected_no_corpus_refuses_the_battery() {
        let executor = FakeExecutor::new(vec![
            exited(true, b"gate evidence"),
            exited(true, EMPTY_CORPUS_EVIDENCE),
            exited(true, b"dialect evidence"),
            exited(true, b"process evidence"),
            exited(true, b"supply evidence"),
        ]);

        let verdict = run_with(&executor, &Battery::current()).expect("typed empty-corpus replay");

        assert_eq!(verdict.checks()[1].kind(), CheckKind::MutationCorpus);
        assert_eq!(verdict.checks()[1].standing(), CheckStanding::EmptyScope);
        assert_eq!(verdict.checks()[1].standing().code(), Some(5));
        assert_eq!(verdict.standing(), ProofStanding::Refused);
    }

    /// A member that announced no corpus proved no corpus.
    ///
    /// The reading admits only a run that said how much it selected, so a tool
    /// that stops announcing cannot quietly become a clean member again: the
    /// count is the whole evidence, and its absence is not a small one.
    #[test]
    fn a_mutation_member_that_announced_no_corpus_refuses_the_battery() {
        let executor = FakeExecutor::new(vec![
            exited(true, b"gate evidence"),
            exited(true, b"mutation evidence"),
            exited(true, b"dialect evidence"),
            exited(true, b"process evidence"),
            exited(true, b"supply evidence"),
        ]);

        let verdict = run_with(&executor, &Battery::current()).expect("typed silent-corpus replay");

        assert_eq!(verdict.checks()[1].standing(), CheckStanding::EmptyScope);
        assert_eq!(verdict.standing(), ProofStanding::Refused);
    }

    #[test]
    fn receipt_construction_refuses_a_payload_for_another_target() {
        use wire::manifest::{BuildPins, TargetPayload};
        use wire::{Digest, Domain};

        let verdict = run_with(
            &FakeExecutor::new(vec![
                exited(true, b"gate"),
                exited(true, MUTATION_EVIDENCE),
                exited(true, b"dialect"),
                exited(true, b"process"),
                exited(true, b"supply"),
            ]),
            &Battery::current(),
        )
        .expect("fixture replay");
        let digest = || Digest::compute(Domain::DistArtifact, b"fixture");
        let payload = TargetPayload {
            version: 1,
            target: ReleaseTarget::Aarch64AppleDarwin,
            source: digest(),
            app: digest(),
            embedded_app: digest(),
            native: digest(),
            pins: BuildPins {
                rust: String::new(),
                wasm_target: String::new(),
                trunk: String::new(),
                cargo_mutants: String::new(),
                proof_container: ENVIRONMENT.to_owned(),
            },
            pins_digest: digest(),
            inventory: Vec::new(),
            inventory_digest: digest(),
        };

        assert!(verdict.receipt(&payload, digest()).is_err());
    }

    const ENVIRONMENT: &str = concat!(
        "registry.example/proof@sha256:",
        "4444444444444444444444444444444444444444444444444444444444444444"
    );

    fn environment() -> ProofEnvironment {
        ProofEnvironment::oci(ENVIRONMENT).expect("fixture image is pinned")
    }

    struct FakeExecutor {
        outcomes: RefCell<VecDeque<Execution>>,
        ran: RefCell<Vec<CheckKind>>,
    }

    impl FakeExecutor {
        fn new(outcomes: Vec<Execution>) -> Self {
            Self {
                outcomes: RefCell::new(outcomes.into()),
                ran: RefCell::new(Vec::new()),
            }
        }
    }

    impl Executor for FakeExecutor {
        fn target(&self) -> ReleaseTarget {
            ReleaseTarget::X86_64UnknownLinuxMusl
        }

        fn environment(&self) -> ProofEnvironment {
            environment()
        }

        fn execute(&self, check: &Check) -> Result<Execution, String> {
            self.ran.borrow_mut().push(check.kind());
            self.outcomes
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| "fixture has no execution outcome".to_owned())
        }
    }

    fn exited(success: bool, stdout: &[u8]) -> Execution {
        Execution {
            completion: ExecutionCompletion::Exited {
                success,
                code: Some(if success { 0 } else { 1 }),
            },
            stdout: Captured {
                bytes: stdout.to_vec(),
                truncated: false,
                complete: true,
            },
            stderr: Captured {
                bytes: Vec::new(),
                truncated: false,
                complete: true,
            },
        }
    }
}
