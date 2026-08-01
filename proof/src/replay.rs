//! Exhaustive sealed replay of the generated proof battery.

use std::error::Error;
use std::fmt;

use wire::manifest::{ProofReceipt, ProofStanding, TargetPayload};
use wire::{Digest, Domain};

use crate::container::{Container, Execution, ExecutionCompletion};

const RECEIPT_VERSION: u16 = 1;

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
            CheckKind::MutationCorpus => ["mutants", "--workspace"].as_slice(),
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

/// Why one declared battery member did or did not pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckStanding {
    Clean,
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

    const fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Failed { .. } => "failed",
            Self::TimedOut { .. } => "timed-out",
            Self::Cancelled { .. } => "cancelled",
            Self::OutputTruncated => "output-truncated",
            Self::OutputIncomplete => "output-incomplete",
        }
    }

    const fn code(self) -> Option<i32> {
        match self {
            Self::Failed { code } | Self::TimedOut { code } | Self::Cancelled { code } => code,
            Self::Clean | Self::OutputTruncated | Self::OutputIncomplete => None,
        }
    }
}

/// One typed verdict over the complete battery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayVerdict {
    environment: String,
    battery: Digest,
    replay: Digest,
    standing: ProofStanding,
    checks: Vec<CheckVerdict>,
}

impl ReplayVerdict {
    #[must_use]
    pub fn environment(&self) -> &str {
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
    #[must_use]
    pub fn receipt(&self, payload: &TargetPayload, payload_digest: Digest) -> ProofReceipt {
        ProofReceipt {
            version: RECEIPT_VERSION,
            target: payload.target,
            payload: payload_digest,
            battery: self.battery.clone(),
            replay: self.replay.clone(),
            environment: self.environment.clone(),
            standing: self.standing,
        }
    }

    #[cfg(test)]
    pub(crate) fn fixture_clean(environment: String, battery: Digest) -> Self {
        let checks = CheckKind::ALL
            .into_iter()
            .map(|kind| CheckVerdict {
                kind,
                standing: CheckStanding::Clean,
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
            .collect::<Vec<_>>();
        let standing = ProofStanding::Clean;
        let replay = replay_digest(&environment, &battery, standing, &checks);
        Self {
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
pub fn replay(container: &Container) -> Result<ReplayVerdict, ReplayError> {
    run_with(container, &Battery::current())
}

trait Executor {
    fn environment(&self) -> &str;
    fn execute(&self, check: &Check) -> Result<Execution, String>;
}

impl Executor for Container {
    fn environment(&self) -> &str {
        self.image()
    }

    fn execute(&self, check: &Check) -> Result<Execution, String> {
        self.execute(check.program(), check.arguments())
            .map_err(|source| source.to_string())
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
    let environment = executor.environment().to_owned();
    let replay = replay_digest(&environment, battery.digest(), standing, &checks);
    Ok(ReplayVerdict {
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
        ExecutionCompletion::Exited { success: true, .. } => CheckStanding::Clean,
    };
    CheckVerdict {
        kind,
        standing,
        stdout: execution.stdout.bytes,
        stderr: execution.stderr.bytes,
    }
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
    environment: &str,
    battery: &Digest,
    standing: ProofStanding,
    checks: &[CheckVerdict],
) -> Digest {
    let mut bytes = Vec::new();
    frame_bytes(&mut bytes, environment.as_bytes());
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
    Digest::compute(Domain::ProofReplay, &bytes)
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

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
                    vec!["mutants", "--workspace"]
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

    #[test]
    fn replay_consumes_every_member_and_returns_one_typed_verdict() {
        let battery = Battery::current();
        let executor = FakeExecutor::new(vec![
            exited(true, b"gate evidence"),
            exited(true, b"mutation evidence"),
            exited(true, b"dialect evidence"),
            exited(true, b"process evidence"),
            exited(true, b"supply evidence"),
        ]);

        let verdict = run_with(&executor, &battery).expect("complete replay");

        assert_eq!(verdict.standing(), ProofStanding::Clean);
        assert_eq!(verdict.checks().len(), CheckKind::ALL.len());
        assert_eq!(executor.ran.borrow().as_slice(), CheckKind::ALL);
        assert_eq!(verdict.battery_digest(), battery.digest());
        assert_eq!(verdict.environment(), ENVIRONMENT);
    }

    #[test]
    fn a_refused_gate_and_a_later_failure_remain_in_the_one_complete_verdict() {
        let battery = Battery::current();
        let executor = FakeExecutor::new(vec![
            exited(false, b"gate failed"),
            exited(true, b"mutation evidence"),
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

    const ENVIRONMENT: &str = "registry.example/proof@sha256:4444444444444444444444444444444444444444444444444444444444444444";

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
        fn environment(&self) -> &str {
            ENVIRONMENT
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
