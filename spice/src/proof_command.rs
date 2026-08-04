//! The binary rim for the two daemonless proof leaves.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use proof::{
    CheckStanding, ContainerError, EnvironmentError, EvidenceError, EvidencePublishError,
    InputError, NativeError, PayloadError, RederiveError, ReplayRefusal, SealError,
};
use sill::Deadlines;
use wire::{Fault, FaultClass, Outcome, ProofEvidence, Refusal, Reply, ReplyPosition};

use spice::config::{LoadError, Resolved};
use spice::repository::Repository;

const MEMBER_RUNTIME: Duration = Duration::from_secs(60 * 60);
const TERMINATE_HORIZON: Duration = Duration::from_secs(10);
const KILL_HORIZON: Duration = Duration::from_secs(10);
const TERMINATION_GRACE: Duration = Duration::from_secs(5);
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;

/// Seal and atomically publish the detached proof for the compiled target.
pub(crate) fn seal() -> Outcome<ProofEvidence> {
    let invocation = match Invocation::acquire() {
        Ok(invocation) => invocation,
        Err(failure) => return failure.outcome(),
    };
    match proof::seal(
        &invocation.layout,
        invocation.config.root().proof(),
        &invocation.workspace,
        limits(),
    ) {
        Ok(evidence) => direct(evidence),
        Err(error) => Failure::seal(error).outcome(),
    }
}

/// Re-execute and verify the existing detached proof for the compiled target.
pub(crate) fn replay() -> Outcome<ProofEvidence> {
    let invocation = match Invocation::acquire() {
        Ok(invocation) => invocation,
        Err(failure) => return failure.outcome(),
    };
    match proof::rederive(
        &invocation.layout,
        invocation.config.root().proof(),
        &invocation.workspace,
        limits(),
    ) {
        Ok(evidence) => direct(evidence),
        Err(error) => Failure::rederive(error).outcome(),
    }
}

fn direct(body: ProofEvidence) -> Outcome<ProofEvidence> {
    Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body,
    })
}

struct Invocation {
    layout: proof::Layout,
    workspace: PathBuf,
    config: Resolved,
}

impl Invocation {
    fn acquire() -> Result<Self, Failure> {
        let current = std::env::current_dir().map_err(|error| {
            Failure::fault(
                FaultClass::Environment,
                "proof::working-directory-unavailable",
                error.to_string(),
            )
        })?;
        let repository = Repository::discover(&current).map_err(|error| {
            Failure::fault(
                FaultClass::Environment,
                "proof::repository-unresolved",
                error.to_string(),
            )
        })?;
        let config =
            spice::config::load_repository(&repository, &[]).map_err(Failure::configuration)?;
        let state =
            repository
                .admit_state()
                .map_err(|error| match crate::state_failure(&error) {
                    crate::StateFailure::Refusal(refusal) => Failure::Refusal(refusal),
                    crate::StateFailure::Fault(fault) => Failure::Fault(fault),
                })?;
        let target = proof::compiled_target().map_err(|error| {
            Failure::refusal(
                "proof::target-refused",
                "one of the two compiled proof release targets",
                error.to_string(),
                "run the proof leaf from an aarch64 Apple Darwin or x86_64 musl build",
            )
        })?;
        let layout = proof::Layout::new(repository.worktree_root(), state.proof(), target);
        Ok(Self {
            layout,
            workspace: repository.worktree_root().to_path_buf(),
            config,
        })
    }
}

fn limits() -> proof::Limits {
    let deadlines = Deadlines::new(MEMBER_RUNTIME, TERMINATE_HORIZON, KILL_HORIZON)
        .expect("compiled proof deadlines are positive and ordered");
    proof::Limits::new(deadlines, TERMINATION_GRACE, OUTPUT_LIMIT)
        .expect("compiled proof output bound is nonzero")
}

enum Failure {
    Refusal(Refusal),
    Fault(Fault),
}

impl Failure {
    fn refusal(
        reason: &'static str,
        expected: impl Into<String>,
        observed: impl Into<String>,
        remedy: impl Into<String>,
    ) -> Self {
        Self::Refusal(Refusal {
            reason: reason.to_owned(),
            witness: None,
            expected: Some(expected.into()),
            observed: Some(observed.into()),
            remedy: remedy.into(),
        })
    }

    fn fault(class: FaultClass, reason: &'static str, detail: impl Into<String>) -> Self {
        Self::Fault(Fault {
            class,
            reason: reason.to_owned(),
            detail: detail.into(),
        })
    }

    fn outcome<B>(self) -> Outcome<B> {
        match self {
            Self::Refusal(refusal) => Outcome::Refusal(refusal),
            Self::Fault(fault) => Outcome::Fault(fault),
        }
    }

    fn configuration(error: LoadError) -> Self {
        if let Some(repository) = error.repository() {
            return Self::fault(
                FaultClass::Environment,
                "proof::repository-unresolved",
                repository.to_string(),
            );
        }
        let diagnostics = configuration_diagnostics(&error);
        if error
            .acquisition()
            .iter()
            .any(|issue| issue.io_error().is_some())
        {
            return Self::fault(
                FaultClass::Environment,
                "proof::configuration-unreadable",
                diagnostics,
            );
        }
        Self::refusal(
            "proof::configuration-refused",
            "valid tracked proof policy and machine-local runtime paths",
            diagnostics,
            "repair the named configuration values before running proof again",
        )
    }

    fn seal(error: SealError) -> Self {
        match error {
            SealError::ReadPayload(error) => Self::input(error, InputKind::Payload),
            SealError::AdmitPayload(error) => Self::payload(error),
            error @ (SealError::Layout(_) | SealError::EnvironmentTarget { .. }) => Self::refusal(
                "proof::layout-refused",
                "one payload, layout, and compiled release target",
                error.to_string(),
                "stage the payload under its exact compiled-target layout and retry",
            ),
            SealError::Environment(error) => Self::environment(error),
            SealError::Replay(error) => Self::fault(
                FaultClass::Environment,
                "proof::process-failed",
                error.to_string(),
            ),
            SealError::Refused(refusal) => Self::battery(refusal),
            SealError::Receipt(error) => Self::refusal(
                "proof::verification-refused",
                "one replay and payload for the same release target",
                error.to_string(),
                "restore the target-matched payload and rerun the complete battery",
            ),
            SealError::EncodeReceipt(error) => {
                Self::fault(FaultClass::Protocol, "proof::encoding-failed", error)
            }
            SealError::Evidence(error) => Self::evidence(error),
            SealError::PrepareDestination { path, source } => Self::fault(
                FaultClass::Store,
                "proof::publication-failed",
                format!("cannot prepare {}: {source}", path.display()),
            ),
            SealError::Publish(error) => Self::publication(error),
            error @ SealError::PublishedReceipt { .. } => Self::fault(
                FaultClass::Protocol,
                "proof::publication-invalid",
                error.to_string(),
            ),
        }
    }

    fn rederive(error: RederiveError) -> Self {
        match error {
            RederiveError::ReadPayload(error) => Self::input(error, InputKind::Payload),
            RederiveError::AdmitPayload(error) => Self::payload(error),
            error @ (RederiveError::Layout(_) | RederiveError::EnvironmentTarget { .. }) => {
                Self::refusal(
                    "proof::layout-refused",
                    "one payload, layout, and compiled release target",
                    error.to_string(),
                    "restore the target-matched staged payload and detached receipt",
                )
            }
            RederiveError::ReadReceipt(error) => Self::input(error, InputKind::Receipt),
            RederiveError::AdmitReceipt(error) => Self::receipt(error),
            RederiveError::Environment(error) => Self::environment(error),
            RederiveError::Replay(error) => Self::fault(
                FaultClass::Environment,
                "proof::process-failed",
                error.to_string(),
            ),
            RederiveError::Refused(refusal) => Self::battery(refusal),
            RederiveError::Evidence(error) => Self::evidence(error),
        }
    }

    fn input(error: InputError, kind: InputKind) -> Self {
        if matches!(&error, InputError::Unreadable { .. }) {
            return Self::fault(
                FaultClass::Environment,
                kind.unreadable_reason(),
                error.to_string(),
            );
        }
        Self::refusal(
            kind.refused_reason(),
            kind.expected(),
            error.to_string(),
            kind.remedy(),
        )
    }

    fn payload(error: PayloadError) -> Self {
        if matches!(&error, PayloadError::Encode(_)) {
            return Self::fault(
                FaultClass::Protocol,
                "proof::encoding-failed",
                error.to_string(),
            );
        }
        Self::refusal(
            "proof::payload-refused",
            "one canonical current target payload with complete bound identities",
            error.to_string(),
            "regenerate the exact target payload from the current distribution pipeline",
        )
    }

    fn receipt(error: EvidenceError) -> Self {
        if matches!(&error, EvidenceError::Encode(_)) {
            return Self::fault(
                FaultClass::Protocol,
                "proof::encoding-failed",
                error.to_string(),
            );
        }
        Self::refusal(
            "proof::receipt-refused",
            "one exact canonical current detached proof receipt",
            error.to_string(),
            "restore or reseal the receipt selected by the admitted payload",
        )
    }

    fn evidence(error: EvidenceError) -> Self {
        if matches!(&error, EvidenceError::Encode(_)) {
            return Self::fault(
                FaultClass::Protocol,
                "proof::encoding-failed",
                error.to_string(),
            );
        }
        Self::refusal(
            "proof::verification-refused",
            "one payload, environment, battery, replay, and receipt identity chain",
            error.to_string(),
            "restore matching proof inputs and rerun the current battery",
        )
    }

    fn environment(error: EnvironmentError) -> Self {
        match &error {
            EnvironmentError::UnsupportedHost(_) | EnvironmentError::PayloadTarget { .. } => {
                return Self::refusal(
                    "proof::target-refused",
                    "the payload target selected by this compiled host",
                    error.to_string(),
                    "stage the payload built for this executable's exact release target",
                );
            }
            EnvironmentError::ImageMismatch { .. } => {
                return Self::refusal(
                    "proof::runner-refused",
                    "the digest-pinned image bound by payload and configuration",
                    error.to_string(),
                    "align proof.image with the admitted musl payload pin",
                );
            }
            EnvironmentError::MissingContainerPath | EnvironmentError::MissingRunnerPath => {}
            EnvironmentError::Oci(error) => {
                return Self::runtime(error.to_string(), container_disposition(error));
            }
            EnvironmentError::Native(error) => {
                return Self::runtime(error.to_string(), native_disposition(error));
            }
        }
        Self::refusal(
            "proof::runner-refused",
            "one configured executable for the compiled proof target",
            error.to_string(),
            "set the target-selected proof.container or proof.runner local configuration value",
        )
    }

    fn runtime(observed: String, disposition: RuntimeDisposition) -> Self {
        match disposition {
            RuntimeDisposition::Refusal => Self::refusal(
                "proof::runner-refused",
                "one exact executable satisfying the sealed proof runtime contract",
                observed,
                "repair the configured runtime path or attestation contract and retry",
            ),
            RuntimeDisposition::Io => Self::fault(
                FaultClass::Environment,
                "proof::runner-unavailable",
                observed,
            ),
            RuntimeDisposition::Process => {
                Self::fault(FaultClass::Environment, "proof::process-failed", observed)
            }
            RuntimeDisposition::Protocol => {
                Self::fault(FaultClass::Protocol, "proof::execution-invalid", observed)
            }
        }
    }

    fn battery(refusal: ReplayRefusal) -> Self {
        let replay = refusal.replay();
        let mut observed = replay
            .checks()
            .iter()
            .filter(|check| !check.standing().passes())
            .map(|check| format!("{}={}", check.kind().as_str(), standing(check.standing())))
            .collect::<Vec<_>>();
        if observed.is_empty() {
            observed.push("standing=refused".to_owned());
        }
        Self::Refusal(Refusal {
            reason: "proof::battery-refused".to_owned(),
            witness: Some(replay.replay_digest().to_string()),
            expected: Some("every generated proof battery member to complete cleanly".to_owned()),
            observed: Some(observed.join(", ")),
            remedy: "repair every named battery failure and rerun the proof leaf".to_owned(),
        })
    }

    fn publication(error: EvidencePublishError) -> Self {
        match error {
            EvidencePublishError::Encode { source } => {
                Self::fault(FaultClass::Protocol, "proof::encoding-failed", source)
            }
            EvidencePublishError::Publish(atomic::Error::Refused(refusal)) => Self::refusal(
                "proof::publication-refused",
                "one regular detached receipt destination in its resolved parent",
                refusal.to_string(),
                "repair the proof destination shape and rerun the seal operation",
            ),
            EvidencePublishError::Publish(error @ atomic::Error::Io { .. }) => Self::fault(
                FaultClass::Store,
                "proof::publication-failed",
                error.to_string(),
            ),
        }
    }
}

#[derive(Clone, Copy)]
enum InputKind {
    Payload,
    Receipt,
}

impl InputKind {
    const fn refused_reason(self) -> &'static str {
        match self {
            Self::Payload => "proof::payload-refused",
            Self::Receipt => "proof::receipt-refused",
        }
    }

    const fn unreadable_reason(self) -> &'static str {
        match self {
            Self::Payload => "proof::payload-unreadable",
            Self::Receipt => "proof::receipt-unreadable",
        }
    }

    const fn expected(self) -> &'static str {
        match self {
            Self::Payload => "one regular target payload at the exact staged path",
            Self::Receipt => "one regular detached receipt selected by the admitted payload",
        }
    }

    const fn remedy(self) -> &'static str {
        match self {
            Self::Payload => "stage the exact target payload as a regular file and retry",
            Self::Receipt => "seal or restore the exact detached receipt as a regular file",
        }
    }
}

#[derive(Clone, Copy)]
enum RuntimeDisposition {
    Refusal,
    Io,
    Process,
    Protocol,
}

fn container_disposition(error: &ContainerError) -> RuntimeDisposition {
    match error {
        ContainerError::InspectRuntime { source, .. }
            if source.kind() != io::ErrorKind::NotFound =>
        {
            RuntimeDisposition::Io
        }
        ContainerError::InspectWorkspace { .. } => RuntimeDisposition::Io,
        ContainerError::SpawnRuntimeProbe(_)
        | ContainerError::WaitRuntimeProbe(_)
        | ContainerError::CreateAttestationChallenge(_)
        | ContainerError::SpawnRuntime(_)
        | ContainerError::Wait(_)
        | ContainerError::MissingCapture(_) => RuntimeDisposition::Process,
        ContainerError::RuntimeProbeSpec(_)
        | ContainerError::ZeroOutputLimit
        | ContainerError::EmptyProgram
        | ContainerError::NulInCommand
        | ContainerError::Spec(_) => RuntimeDisposition::Protocol,
        ContainerError::RuntimeNotAbsolute(_)
        | ContainerError::InspectRuntime { .. }
        | ContainerError::RuntimeNotRegular(_)
        | ContainerError::RuntimeNotExecutable(_)
        | ContainerError::RuntimeDialectNotAttested { .. }
        | ContainerError::ImageNotDigestPinned(_)
        | ContainerError::ImageNotResolved { .. }
        | ContainerError::ImageIdentityDrift { .. }
        | ContainerError::WorkspaceNotDirectory(_)
        | ContainerError::UnrepresentableWorkspace(_)
        | ContainerError::RuntimeNotAttested { .. } => RuntimeDisposition::Refusal,
    }
}

fn native_disposition(error: &NativeError) -> RuntimeDisposition {
    match error {
        NativeError::InspectRunner { source, .. } if source.kind() != io::ErrorKind::NotFound => {
            RuntimeDisposition::Io
        }
        NativeError::ReadRunner { .. } | NativeError::InspectWorkspace { .. } => {
            RuntimeDisposition::Io
        }
        NativeError::CreateChallenge(_)
        | NativeError::Spawn(_)
        | NativeError::Wait(_)
        | NativeError::MissingCapture(_)
        | NativeError::PreflightOutputIncomplete
        | NativeError::PreflightFailed { .. }
        | NativeError::PreflightTimedOut { .. }
        | NativeError::PreflightCancelled { .. } => RuntimeDisposition::Process,
        NativeError::Command(error) => container_disposition(error),
        NativeError::Spec(_) => RuntimeDisposition::Protocol,
        NativeError::RunnerNotAbsolute(_)
        | NativeError::InspectRunner { .. }
        | NativeError::RunnerNotRegular(_)
        | NativeError::RunnerNotExecutable(_)
        | NativeError::RunnerChanged { .. }
        | NativeError::WorkspaceNotDirectory(_)
        | NativeError::IncompleteAttestation { .. }
        | NativeError::RunnerNotAttested { .. } => RuntimeDisposition::Refusal,
    }
}

fn standing(standing: CheckStanding) -> String {
    match standing {
        CheckStanding::Clean => "clean".to_owned(),
        CheckStanding::Failed { code } => format!("failed{}", code_suffix(code)),
        CheckStanding::TimedOut { code } => format!("timed-out{}", code_suffix(code)),
        CheckStanding::Cancelled { code } => format!("cancelled{}", code_suffix(code)),
        CheckStanding::OutputTruncated => "output-truncated".to_owned(),
        CheckStanding::OutputIncomplete => "output-incomplete".to_owned(),
    }
}

fn code_suffix(code: Option<i32>) -> String {
    code.map_or_else(String::new, |code| format!("(code={code})"))
}

fn configuration_diagnostics(error: &LoadError) -> String {
    let mut diagnostics = error
        .acquisition()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if let Some(resolution) = error.resolution() {
        diagnostics.extend(resolution.issues().iter().map(ToString::to_string));
    }
    if diagnostics.is_empty() {
        error.to_string()
    } else {
        diagnostics.join(" | ")
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::PathBuf;

    use proof::{EvidencePublishError, Input, InputError, PayloadError, SealError};
    use wire::{FaultClass, Outcome};

    use super::Failure;

    #[test]
    fn input_shape_refuses_while_io_and_encoding_defects_fault() {
        let missing = Failure::seal(SealError::ReadPayload(InputError::Missing {
            input: Input::Payload,
            path: PathBuf::from("payload.json"),
        }))
        .outcome::<()>();
        let Outcome::Refusal(missing) = missing else {
            panic!("missing payload must refuse")
        };
        assert_eq!(missing.reason, "proof::payload-refused");

        let unreadable = Failure::seal(SealError::ReadPayload(InputError::Unreadable {
            input: Input::Payload,
            path: PathBuf::from("payload.json"),
            source: io::Error::new(io::ErrorKind::PermissionDenied, "fixture"),
        }))
        .outcome::<()>();
        let Outcome::Fault(unreadable) = unreadable else {
            panic!("payload I/O must fault")
        };
        assert_eq!(unreadable.reason, "proof::payload-unreadable");
        assert_eq!(unreadable.class, FaultClass::Environment);

        let encoding = Failure::seal(SealError::AdmitPayload(PayloadError::Encode(
            "fixture encoder".to_owned(),
        )))
        .outcome::<()>();
        let Outcome::Fault(encoding) = encoding else {
            panic!("encoding defect must fault")
        };
        assert_eq!(encoding.reason, "proof::encoding-failed");
        assert_eq!(encoding.class, FaultClass::Protocol);
    }

    #[test]
    fn publication_refusal_and_io_failure_remain_distinct() {
        let refused = Failure::seal(SealError::Publish(EvidencePublishError::Publish(
            atomic::Error::Refused(atomic::Refusal::DestinationHasNoFileName {
                destination: PathBuf::from("/"),
            }),
        )))
        .outcome::<()>();
        let Outcome::Refusal(refused) = refused else {
            panic!("publication shape must refuse")
        };
        assert_eq!(refused.reason, "proof::publication-refused");

        let failed = Failure::seal(SealError::Publish(EvidencePublishError::Publish(
            atomic::Error::Io {
                operation: atomic::Operation::Replace,
                path: PathBuf::from("receipt.json"),
                source: io::Error::other("fixture"),
                cleanup: None,
            },
        )))
        .outcome::<()>();
        let Outcome::Fault(failed) = failed else {
            panic!("publication I/O must fault")
        };
        assert_eq!(failed.reason, "proof::publication-failed");
        assert_eq!(failed.class, FaultClass::Store);
    }
}
