//! Digest-identified native proof runner and its exact attestation exchange.

use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sill::{Argv, Completion, Spec, SpecError, WaitError};
use wire::{Digest, Domain};

use crate::challenge::Challenge;
use crate::container::{
    Captured, ContainerError, Execution, ExecutionCompletion, Limits, is_executable,
    validate_command,
};

const PROTOCOL: &str = "spice-proof-native-runner-v1";
const ATTESTATION_PREFIX: &str = "spice-proof-native-attestation-v1";
const ISOLATION_CLAIMS: &str = concat!(
    "repository=read-only:",
    "cargo-home=fresh-empty:",
    "cargo-target=fresh-empty:",
    "network=denied:",
    "host-dependencies=denied"
);

/// One canonical native runner executable and the bytes that identify it.
#[derive(Clone, Debug)]
pub struct NativeRunner {
    binary: PathBuf,
    identity: Digest,
    workspace: PathBuf,
    limits: Limits,
}

impl NativeRunner {
    /// Admit an absolute regular executable by its observed bytes.
    ///
    /// # Errors
    ///
    /// Refuses a relative, missing, non-regular, non-executable, or unreadable
    /// runner and a workspace that is not a canonical directory.
    pub fn attest(
        binary: impl AsRef<Path>,
        workspace: impl AsRef<Path>,
        limits: Limits,
    ) -> Result<Self, NativeError> {
        let requested = binary.as_ref();
        if !requested.is_absolute() {
            return Err(NativeError::RunnerNotAbsolute(requested.to_path_buf()));
        }
        let binary = fs::canonicalize(requested).map_err(|source| NativeError::InspectRunner {
            path: requested.to_path_buf(),
            source,
        })?;
        inspect_runner(&binary)?;
        let bytes = fs::read(&binary).map_err(|source| NativeError::ReadRunner {
            path: binary.clone(),
            source,
        })?;
        let identity = Digest::compute(Domain::ProofNativeRunner, &bytes);

        let requested_workspace = workspace.as_ref();
        let workspace = fs::canonicalize(requested_workspace).map_err(|source| {
            NativeError::InspectWorkspace {
                path: requested_workspace.to_path_buf(),
                source,
            }
        })?;
        if !workspace.is_dir() {
            return Err(NativeError::WorkspaceNotDirectory(workspace));
        }

        Ok(Self {
            binary,
            identity,
            workspace,
            limits,
        })
    }

    /// Digest computed from the exact admitted executable bytes.
    #[must_use]
    pub const fn identity(&self) -> &Digest {
        &self.identity
    }

    /// Canonical executable path inspected during admission.
    #[must_use]
    pub fn binary(&self) -> &Path {
        &self.binary
    }

    pub(crate) fn execute(
        &self,
        program: &str,
        arguments: &[String],
    ) -> Result<Execution, NativeError> {
        validate_command(program, arguments).map_err(NativeError::Command)?;
        self.verify_identity()?;
        let challenge = Challenge::fresh().map_err(NativeError::CreateChallenge)?;
        let spec = self.spec(program, arguments, &challenge)?;
        let running = {
            #[cfg(test)]
            let _spawn_guard = crate::test_support::process_spawn_guard();
            sill::spawn(spec)
        }
        .map_err(NativeError::Spawn)?;
        let outcome = running.wait().map_err(NativeError::Wait)?;
        let completion = match outcome.completion() {
            Completion::Exited(status) => ExecutionCompletion::Exited {
                success: status.success(),
                code: status.code(),
            },
            Completion::TimedOut(status) => ExecutionCompletion::TimedOut {
                code: status.code(),
            },
            Completion::Cancelled(status) => ExecutionCompletion::Cancelled {
                code: status.code(),
            },
        };
        let stdout = outcome
            .stdout()
            .map(Captured::from)
            .ok_or(NativeError::MissingCapture("stdout"))?;
        let mut stderr = outcome
            .stderr()
            .map(Captured::from)
            .ok_or(NativeError::MissingCapture("stderr"))?;
        self.verify_attestation(&mut stderr, &challenge)?;
        Ok(Execution {
            completion,
            stdout,
            stderr,
        })
    }

    fn spec(
        &self,
        program: &str,
        arguments: &[String],
        challenge: &Challenge,
    ) -> Result<Spec, NativeError> {
        validate_command(program, arguments).map_err(NativeError::Command)?;
        let mut runner_arguments = vec![
            PROTOCOL.to_owned(),
            self.identity.to_string(),
            challenge.as_str().to_owned(),
            program.to_owned(),
        ];
        runner_arguments.extend(arguments.iter().cloned());
        let argv = Argv::new(
            self.binary.as_os_str().to_owned(),
            runner_arguments.into_iter().map(OsString::from),
        )
        .map_err(NativeError::Spec)?;
        self.limits
            .process_spec(
                argv,
                self.workspace.clone(),
                format!("sealed native proof battery member {program:?}"),
            )
            .map_err(NativeError::Spec)
    }

    fn verify_identity(&self) -> Result<(), NativeError> {
        inspect_runner(&self.binary)?;
        let bytes = fs::read(&self.binary).map_err(|source| NativeError::ReadRunner {
            path: self.binary.clone(),
            source,
        })?;
        let observed = Digest::compute(Domain::ProofNativeRunner, &bytes);
        if observed != self.identity {
            return Err(NativeError::RunnerChanged {
                expected: self.identity.clone(),
                observed,
            });
        }
        Ok(())
    }

    fn verify_attestation(
        &self,
        stderr: &mut Captured,
        challenge: &Challenge,
    ) -> Result<(), NativeError> {
        if stderr.truncated || !stderr.complete {
            return Err(NativeError::IncompleteAttestation {
                truncated: stderr.truncated,
                complete: stderr.complete,
            });
        }
        let expected = format!(
            "{ATTESTATION_PREFIX}:runner={}:challenge={}:{}\n",
            self.identity,
            challenge.as_str(),
            ISOLATION_CLAIMS
        );
        if !stderr.bytes.starts_with(expected.as_bytes()) {
            return Err(NativeError::RunnerNotAttested {
                expected,
                observed: String::from_utf8_lossy(&stderr.bytes).into_owned(),
            });
        }
        stderr.bytes.drain(..expected.len());
        Ok(())
    }
}

fn inspect_runner(path: &Path) -> Result<(), NativeError> {
    let metadata = fs::metadata(path).map_err(|source| NativeError::InspectRunner {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(NativeError::RunnerNotRegular(path.to_path_buf()));
    }
    if !is_executable(&metadata) {
        return Err(NativeError::RunnerNotExecutable(path.to_path_buf()));
    }
    Ok(())
}

/// Why a native sealed environment was not admitted or attested.
#[derive(Debug)]
pub enum NativeError {
    RunnerNotAbsolute(PathBuf),
    InspectRunner { path: PathBuf, source: io::Error },
    RunnerNotRegular(PathBuf),
    RunnerNotExecutable(PathBuf),
    ReadRunner { path: PathBuf, source: io::Error },
    RunnerChanged { expected: Digest, observed: Digest },
    InspectWorkspace { path: PathBuf, source: io::Error },
    WorkspaceNotDirectory(PathBuf),
    Command(ContainerError),
    CreateChallenge(io::Error),
    Spec(SpecError),
    Spawn(io::Error),
    Wait(WaitError),
    MissingCapture(&'static str),
    IncompleteAttestation { truncated: bool, complete: bool },
    RunnerNotAttested { expected: String, observed: String },
}

impl fmt::Display for NativeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RunnerNotAbsolute(path) => write!(
                formatter,
                "native proof runner path must be absolute: {}",
                path.display()
            ),
            Self::InspectRunner { path, source } => {
                write!(
                    formatter,
                    "cannot inspect native proof runner {}: {source}",
                    path.display()
                )
            }
            Self::RunnerNotRegular(path) => write!(
                formatter,
                "native proof runner is not a regular file: {}",
                path.display()
            ),
            Self::RunnerNotExecutable(path) => write!(
                formatter,
                "native proof runner is not executable: {}",
                path.display()
            ),
            Self::ReadRunner { path, source } => write!(
                formatter,
                "cannot read native proof runner {}: {source}",
                path.display()
            ),
            Self::RunnerChanged { expected, observed } => write!(
                formatter,
                concat!(
                    "native proof runner bytes changed after admission: ",
                    "expected {}, observed {}"
                ),
                expected, observed
            ),
            Self::InspectWorkspace { path, source } => write!(
                formatter,
                "cannot inspect native proof workspace {}: {source}",
                path.display()
            ),
            Self::WorkspaceNotDirectory(path) => write!(
                formatter,
                "native proof workspace is not a directory: {}",
                path.display()
            ),
            Self::Command(source) => write!(formatter, "invalid native proof command: {source}"),
            Self::CreateChallenge(source) => {
                write!(formatter, "cannot create native proof challenge: {source}")
            }
            Self::Spec(source) => write!(formatter, "invalid native proof process: {source}"),
            Self::Spawn(source) => write!(formatter, "cannot start native proof runner: {source}"),
            Self::Wait(source) => write!(formatter, "native proof runner wait failed: {source}"),
            Self::MissingCapture(stream) => {
                write!(
                    formatter,
                    "native proof runner returned no {stream} capture"
                )
            }
            Self::IncompleteAttestation {
                truncated,
                complete,
            } => write!(
                formatter,
                concat!(
                    "native proof runner attestation capture is incomplete: ",
                    "truncated={}, complete={}"
                ),
                truncated, complete
            ),
            Self::RunnerNotAttested { expected, observed } => write!(
                formatter,
                concat!(
                    "native proof runner did not attest its isolation contract: ",
                    "expected {:?}, observed {:?}"
                ),
                expected, observed
            ),
        }
    }
}

impl Error for NativeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InspectRunner { source, .. }
            | Self::ReadRunner { source, .. }
            | Self::InspectWorkspace { source, .. }
            | Self::CreateChallenge(source)
            | Self::Spawn(source) => Some(source),
            Self::Command(source) => Some(source),
            Self::Spec(source) => Some(source),
            Self::Wait(source) => Some(source),
            Self::RunnerNotAbsolute(_)
            | Self::RunnerNotRegular(_)
            | Self::RunnerNotExecutable(_)
            | Self::RunnerChanged { .. }
            | Self::WorkspaceNotDirectory(_)
            | Self::MissingCapture(_)
            | Self::IncompleteAttestation { .. }
            | Self::RunnerNotAttested { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use sill::{Deadlines, ProcessGroup, StdinPolicy, StreamPolicy};

    use super::{NativeError, NativeRunner};
    use crate::challenge::Challenge;
    use crate::container::{Captured, Limits};
    use crate::environment::SealedEnvironment;
    use crate::replay::{CheckKind, replay};

    const VALID_RUNNER: &str = r#"#!/bin/sh
[ "$1" = spice-proof-native-runner-v1 ] || exit 71
identity=$2
challenge=$3
shift 3
marker=spice-proof-native-attestation-v1
claims=repository=read-only:
claims=${claims}cargo-home=fresh-empty:
claims=${claims}cargo-target=fresh-empty:
claims=${claims}network=denied:
claims=${claims}host-dependencies=denied
printf '%s:runner=%s:challenge=%s:%s\n' "$marker" "$identity" "$challenge" "$claims" >&2
printf 'challenge:%s\nexecuted:%s\n' "$challenge" "$*"
exit 0
"#;
    const FIXED_RUNNER: &str = r#"#!/bin/sh
marker=spice-proof-native-attestation-v1
claims=repository=read-only:cargo-home=fresh-empty:
claims=${claims}cargo-target=fresh-empty:network=denied:
claims=${claims}host-dependencies=denied
printf '%s:runner=fixed:challenge=fixed:%s\n' "$marker" "$claims" >&2
exit 0
"#;
    const STALE_RUNNER: &str = r#"#!/bin/sh
identity=$2
marker=spice-proof-native-attestation-v1
claims=repository=read-only:cargo-home=fresh-empty:
claims=${claims}cargo-target=fresh-empty:network=denied:
claims=${claims}host-dependencies=denied
printf '%s:runner=%s:challenge=stale-challenge:%s\n' "$marker" "$identity" "$claims" >&2
exit 0
"#;
    const WRONG_IDENTITY_RUNNER: &str = r#"#!/bin/sh
challenge=$3
marker=spice-proof-native-attestation-v1
wrong=00000000000000000000000000000000
wrong=${wrong}00000000000000000000000000000000
claims=repository=read-only:cargo-home=fresh-empty:
claims=${claims}cargo-target=fresh-empty:network=denied:
claims=${claims}host-dependencies=denied
printf '%s:runner=%s:challenge=%s:%s\n' "$marker" "$wrong" "$challenge" "$claims" >&2
exit 0
"#;
    const SILENT_RUNNER: &str = "#!/bin/sh\nexit 0\n";
    const FAILED_RUNNER: &str = "#!/bin/sh\nexit 70\n";
    const TIMED_OUT_RUNNER: &str = "#!/bin/sh\n/bin/sleep 2\n";

    #[test]
    fn native_spec_uses_the_shared_sealed_process_laws() {
        let fixture = Fixture::new(VALID_RUNNER, limits());
        let runner = fixture.runner();
        let spec = runner
            .spec(
                "cargo",
                &["test".to_owned(), "--locked".to_owned()],
                &Challenge::fixture("fixture-challenge"),
            )
            .expect("native spec");
        let arguments = spec
            .argv
            .arguments()
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(spec.group, ProcessGroup::New);
        assert!(spec.environment.entries().is_empty());
        assert_eq!(
            spec.cwd,
            fixture.workspace.canonicalize().expect("workspace")
        );
        assert_eq!(spec.output.stdin(), StdinPolicy::Null);
        assert!(matches!(
            spec.output.stdout(),
            StreamPolicy::Capture { limit: 4096 }
        ));
        assert!(matches!(
            spec.output.stderr(),
            StreamPolicy::Capture { limit: 4096 }
        ));
        assert_eq!(spec.deadlines.runtime(), Duration::from_secs(30));
        assert_eq!(
            arguments[0], "spice-proof-native-runner-v1",
            "the versioned protocol is the first and only runner operation"
        );
        assert_eq!(arguments[1], runner.identity().as_str());
        assert_eq!(arguments[2], "fixture-challenge");
        assert_eq!(&arguments[3..], ["cargo", "test", "--locked"]);
    }

    #[test]
    fn every_battery_member_runs_and_attestation_never_enters_evidence() {
        let fixture = Fixture::new(VALID_RUNNER, limits());
        let environment = SealedEnvironment::fixture_native(fixture.runner());
        let verdict = replay(&environment).expect("all native exchanges attest");
        let mut challenges = std::collections::BTreeSet::new();

        assert_eq!(verdict.checks().len(), CheckKind::ALL.len());
        for (verdict, expected) in verdict.checks().iter().zip(CheckKind::ALL) {
            assert_eq!(verdict.kind(), expected);
            assert!(verdict.stderr().is_empty());
            let output = std::str::from_utf8(verdict.stdout()).expect("fixture output is UTF-8");
            let mut lines = output.lines();
            let challenge = lines
                .next()
                .and_then(|line| line.strip_prefix("challenge:"))
                .expect("fixture reports the received challenge");
            assert_eq!(challenge.len(), 64);
            assert!(challenges.insert(challenge.to_owned()));
            assert!(
                lines
                    .next()
                    .is_some_and(|line| line.starts_with("executed:cargo "))
            );
        }
        assert_eq!(challenges.len(), CheckKind::ALL.len());
    }

    #[test]
    fn fixed_stale_wrong_missing_and_failed_attestations_refuse_execution() {
        for script in [
            SILENT_RUNNER,
            FIXED_RUNNER,
            STALE_RUNNER,
            WRONG_IDENTITY_RUNNER,
            FAILED_RUNNER,
        ] {
            let fixture = Fixture::new(script, limits());
            let error = fixture
                .runner()
                .execute("cargo", &["test".to_owned()])
                .expect_err("invalid attestation cannot create Execution");
            assert!(matches!(error, NativeError::RunnerNotAttested { .. }));
        }
    }

    #[test]
    fn a_timed_out_exchange_without_a_marker_refuses_execution() {
        let fixture = Fixture::new(TIMED_OUT_RUNNER, short_limits());
        let error = fixture
            .runner()
            .execute("cargo", &["test".to_owned()])
            .expect_err("timed-out attestation cannot create Execution");

        assert!(matches!(error, NativeError::RunnerNotAttested { .. }));
    }

    #[test]
    fn truncated_and_incomplete_attestation_captures_refuse() {
        let fixture = Fixture::new(VALID_RUNNER, limits());
        let runner = fixture.runner();
        let challenge = Challenge::fixture("fixture-challenge");
        for (truncated, complete) in [(true, true), (false, false), (true, false)] {
            let mut capture = Captured {
                bytes: Vec::new(),
                truncated,
                complete,
            };
            assert!(matches!(
                runner.verify_attestation(&mut capture, &challenge),
                Err(NativeError::IncompleteAttestation { .. })
            ));
        }
    }

    #[test]
    fn runner_byte_mutation_after_admission_refuses_before_process_start() {
        let fixture = Fixture::new(VALID_RUNNER, limits());
        let runner = fixture.runner();
        {
            let _write_guard = crate::test_support::executable_write_guard();
            fs::write(&fixture.runner, FAILED_RUNNER).expect("mutate admitted runner");
            make_executable(&fixture.runner);
        }

        assert!(matches!(
            runner.execute("cargo", &["test".to_owned()]),
            Err(NativeError::RunnerChanged { .. })
        ));
    }

    struct Fixture {
        root: std::path::PathBuf,
        runner: std::path::PathBuf,
        workspace: std::path::PathBuf,
        limits: Limits,
    }

    impl Fixture {
        fn new(script: &str, limits: Limits) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "spice-proof-native-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let runner = root.join("runner");
            let workspace = root.join("workspace");
            fs::create_dir_all(&workspace).expect("fixture workspace");
            {
                let _write_guard = crate::test_support::executable_write_guard();
                fs::write(&runner, script).expect("fixture runner");
                make_executable(&runner);
            }
            Self {
                root,
                runner,
                workspace,
                limits,
            }
        }

        fn runner(&self) -> NativeRunner {
            NativeRunner::attest(&self.runner, &self.workspace, self.limits)
                .expect("fixture runner admits")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).expect("remove fixture");
        }
    }

    fn limits() -> Limits {
        limits_with_runtime(Duration::from_secs(30))
    }

    fn short_limits() -> Limits {
        limits_with_runtime(Duration::from_millis(20))
    }

    fn limits_with_runtime(runtime: Duration) -> Limits {
        Limits::new(
            Deadlines::new(runtime, Duration::from_secs(1), Duration::from_secs(1))
                .expect("deadlines"),
            Duration::from_millis(10),
            4096,
        )
        .expect("limits")
    }

    #[cfg(unix)]
    fn make_executable(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .expect("make runner executable");
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &std::path::Path) {}
}
