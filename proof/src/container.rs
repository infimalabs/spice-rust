//! The sealed OCI process contract used by proof replay.

use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sill::{
    Argv, Cancellation, Completion, Deadlines, Environment, Output, ProcessGroup, Redaction,
    RestartDisposition, Spec, SpecError, StdinPolicy, StreamPolicy, WaitError, WaitPolicy,
};

const WORKSPACE_MOUNT: &str = "/workspace";
const CARGO_HOME: &str = "/cargo-home";
const CARGO_TARGET: &str = "/cargo-target";
const SHA256_HEX_LENGTH: usize = 64;
const ATTESTATION_PREFIX: &str = "spice-proof-attestation-v1:";
const PREFLIGHT_SCRIPT: &str = r#"
refuse() {
    printf 'spice-proof-preflight-refused:%s\n' "$1" >&2
    exit 70
}

[ "${CARGO_HOME:-}" = /cargo-home ] || refuse cargo-home-environment
[ "${CARGO_TARGET_DIR:-}" = /cargo-target ] || refuse cargo-target-environment
[ -d /cargo-home ] || refuse cargo-home-missing
[ -d /cargo-target ] || refuse cargo-target-missing
[ -d /sys/class/net ] || refuse network-state-unavailable

workspace_read_only=false
cargo_home_tmpfs=false
cargo_target_tmpfs=false
while IFS= read -r mount; do
    case "$mount" in
        *" /workspace ro,"*|*" /workspace ro "*) workspace_read_only=true ;;
        *" /cargo-home "*" - tmpfs "*) cargo_home_tmpfs=true ;;
        *" /cargo-target "*" - tmpfs "*) cargo_target_tmpfs=true ;;
    esac
done < /proc/self/mountinfo
[ "$workspace_read_only" = true ] || refuse workspace-not-read-only
[ "$cargo_home_tmpfs" = true ] || refuse cargo-home-not-tmpfs
[ "$cargo_target_tmpfs" = true ] || refuse cargo-target-not-tmpfs

for cache in /cargo-home /cargo-target; do
    for entry in "$cache"/* "$cache"/.[!.]* "$cache"/..?*; do
        [ ! -e "$entry" ] || refuse cache-not-empty
    done
done
for interface in /sys/class/net/*; do
    [ -e "$interface" ] || continue
    [ "${interface##*/}" = lo ] || refuse network-interface-present
done

dialect=$1
shift
printf 'spice-proof-attestation-v1:%s\n' "$dialect" >&2
exec "$@"
"#;

/// OCI runtime contract whose network-isolation spelling is known here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OciRuntime {
    kind: RuntimeKind,
    binary: PathBuf,
}

impl OciRuntime {
    /// Admit one Docker executable.
    ///
    /// # Errors
    ///
    /// Refuses non-absolute, missing, non-regular, or non-executable paths.
    pub fn docker(binary: impl AsRef<Path>) -> Result<Self, ContainerError> {
        Self::new(RuntimeKind::Docker, binary.as_ref())
    }

    /// Admit one Podman executable.
    ///
    /// # Errors
    ///
    /// Refuses non-absolute, missing, non-regular, or non-executable paths.
    pub fn podman(binary: impl AsRef<Path>) -> Result<Self, ContainerError> {
        Self::new(RuntimeKind::Podman, binary.as_ref())
    }

    fn new(kind: RuntimeKind, binary: &Path) -> Result<Self, ContainerError> {
        if !binary.is_absolute() {
            return Err(ContainerError::RuntimeNotAbsolute(binary.to_path_buf()));
        }
        let binary = fs::canonicalize(binary).map_err(|source| ContainerError::InspectRuntime {
            path: binary.to_path_buf(),
            source,
        })?;
        let metadata = fs::metadata(&binary).map_err(|source| ContainerError::InspectRuntime {
            path: binary.clone(),
            source,
        })?;
        if !metadata.is_file() {
            return Err(ContainerError::RuntimeNotRegular(binary));
        }
        if !is_executable(&metadata) {
            return Err(ContainerError::RuntimeNotExecutable(binary));
        }
        Ok(Self { kind, binary })
    }

    /// Exact runtime executable admitted above.
    #[must_use]
    pub fn binary(&self) -> &Path {
        &self.binary
    }

    /// Runtime dialect whose isolation flags are emitted.
    #[must_use]
    pub const fn kind(&self) -> RuntimeKind {
        self.kind
    }
}

/// OCI command dialects with the sealed flags proof relies on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeKind {
    Docker,
    Podman,
}

impl RuntimeKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }
}

/// Explicit process bounds for every battery member.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    deadlines: Deadlines,
    grace: Duration,
    output_limit: usize,
}

impl Limits {
    /// State all execution and evidence bounds.
    ///
    /// # Errors
    ///
    /// Refuses a zero output limit; deadline admission is owned by [`Deadlines`].
    pub fn new(
        deadlines: Deadlines,
        grace: Duration,
        output_limit: usize,
    ) -> Result<Self, ContainerError> {
        if output_limit == 0 {
            return Err(ContainerError::ZeroOutputLimit);
        }
        Ok(Self {
            deadlines,
            grace,
            output_limit,
        })
    }
}

/// A network-disabled build environment with fresh empty cache mounts.
#[derive(Clone, Debug)]
pub struct Container {
    runtime: OciRuntime,
    image: String,
    workspace: PathBuf,
    limits: Limits,
}

impl Container {
    /// Admit one sealed OCI environment.
    ///
    /// Every invocation gets a new tmpfs at Cargo's home and target paths,
    /// disables networking, mounts the source read-only, and clears the host
    /// environment before the runtime starts.
    ///
    /// # Errors
    ///
    /// Refuses an unpinned image, an unreadable workspace, or a mount path the
    /// OCI argument grammar cannot represent exactly.
    pub fn new(
        runtime: OciRuntime,
        image: impl Into<String>,
        workspace: impl AsRef<Path>,
        limits: Limits,
    ) -> Result<Self, ContainerError> {
        let image = image.into();
        if !is_digest_pinned(&image) {
            return Err(ContainerError::ImageNotDigestPinned(image));
        }
        let requested = workspace.as_ref();
        let workspace =
            fs::canonicalize(requested).map_err(|source| ContainerError::InspectWorkspace {
                path: requested.to_path_buf(),
                source,
            })?;
        if !workspace.is_dir() {
            return Err(ContainerError::WorkspaceNotDirectory(workspace));
        }
        let Some(spelling) = workspace.to_str() else {
            return Err(ContainerError::UnrepresentableWorkspace(workspace));
        };
        if spelling.contains([',', '\n', '\r']) {
            return Err(ContainerError::UnrepresentableWorkspace(workspace));
        }

        Ok(Self {
            runtime,
            image,
            workspace,
            limits,
        })
    }

    /// Digest-pinned environment identity executed by every battery member.
    #[must_use]
    pub fn image(&self) -> &str {
        &self.image
    }

    /// Build the exact process contract for one command inside the sealed environment.
    ///
    /// # Errors
    ///
    /// Refuses an empty program, a NUL-bearing argument, or an invalid sill contract.
    pub fn spec(&self, program: &str, arguments: &[String]) -> Result<Spec, ContainerError> {
        validate_command(program, arguments)?;
        let mount = format!(
            "type=bind,source={},destination={WORKSPACE_MOUNT},readonly",
            self.workspace.display()
        );
        let mut runtime_arguments = vec![
            "run".to_owned(),
            "--rm".to_owned(),
            "--network=none".to_owned(),
            "--read-only".to_owned(),
            "--cap-drop=ALL".to_owned(),
            "--security-opt=no-new-privileges".to_owned(),
            "--mount".to_owned(),
            mount,
            "--mount".to_owned(),
            format!("type=tmpfs,destination={CARGO_HOME},tmpfs-mode=0700"),
            "--mount".to_owned(),
            format!("type=tmpfs,destination={CARGO_TARGET},tmpfs-mode=0700"),
            "--workdir".to_owned(),
            WORKSPACE_MOUNT.to_owned(),
            "--env".to_owned(),
            format!("CARGO_HOME={CARGO_HOME}"),
            "--env".to_owned(),
            format!("CARGO_TARGET_DIR={CARGO_TARGET}"),
            "--env".to_owned(),
            "CARGO_NET_OFFLINE=true".to_owned(),
            self.image.clone(),
            "/bin/sh".to_owned(),
            "-ceu".to_owned(),
            PREFLIGHT_SCRIPT.to_owned(),
            "spice-proof-attestation".to_owned(),
            self.runtime.kind.as_str().to_owned(),
            program.to_owned(),
        ];
        runtime_arguments.extend(arguments.iter().cloned());

        Ok(Spec {
            argv: Argv::new(
                self.runtime.binary.as_os_str().to_owned(),
                runtime_arguments.into_iter().map(OsString::from),
            )
            .map_err(ContainerError::Spec)?,
            cwd: self.workspace.clone(),
            environment: Environment::empty(),
            group: ProcessGroup::New,
            deadlines: self.limits.deadlines,
            wait: WaitPolicy::new(
                format!("sealed proof battery member {program:?}"),
                Cancellation::Never,
                RestartDisposition::Never,
            )
            .map_err(ContainerError::Spec)?,
            output: Output::new(
                StdinPolicy::Null,
                StreamPolicy::Capture {
                    limit: self.limits.output_limit,
                },
                StreamPolicy::Capture {
                    limit: self.limits.output_limit,
                },
            ),
            redaction: Redaction::None,
            grace: self.limits.grace,
        })
    }

    pub(crate) fn execute(
        &self,
        program: &str,
        arguments: &[String],
    ) -> Result<Execution, ContainerError> {
        let running =
            sill::spawn(self.spec(program, arguments)?).map_err(ContainerError::SpawnRuntime)?;
        let outcome = running.wait().map_err(ContainerError::Wait)?;
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
            .ok_or(ContainerError::MissingCapture("stdout"))?;
        let mut stderr = outcome
            .stderr()
            .map(Captured::from)
            .ok_or(ContainerError::MissingCapture("stderr"))?;
        self.verify_attestation(&mut stderr)?;
        Ok(Execution {
            completion,
            stdout,
            stderr,
        })
    }

    fn verify_attestation(&self, stderr: &mut Captured) -> Result<(), ContainerError> {
        let expected = format!("{ATTESTATION_PREFIX}{}\n", self.runtime.kind.as_str());
        if !stderr.bytes.starts_with(expected.as_bytes()) {
            return Err(ContainerError::RuntimeNotAttested {
                expected,
                observed: String::from_utf8_lossy(&stderr.bytes).into_owned(),
            });
        }
        stderr.bytes.drain(..expected.len());
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Execution {
    pub(crate) completion: ExecutionCompletion,
    pub(crate) stdout: Captured,
    pub(crate) stderr: Captured,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExecutionCompletion {
    Exited { success: bool, code: Option<i32> },
    TimedOut { code: Option<i32> },
    Cancelled { code: Option<i32> },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Captured {
    pub(crate) bytes: Vec<u8>,
    pub(crate) truncated: bool,
    pub(crate) complete: bool,
}

impl From<&sill::CapturedOutput> for Captured {
    fn from(output: &sill::CapturedOutput) -> Self {
        Self {
            bytes: output.bytes().to_vec(),
            truncated: output.truncated(),
            complete: output.complete(),
        }
    }
}

/// Why a sealed environment was not established or executed.
#[derive(Debug)]
pub enum ContainerError {
    RuntimeNotAbsolute(PathBuf),
    InspectRuntime { path: PathBuf, source: io::Error },
    RuntimeNotRegular(PathBuf),
    RuntimeNotExecutable(PathBuf),
    ImageNotDigestPinned(String),
    InspectWorkspace { path: PathBuf, source: io::Error },
    WorkspaceNotDirectory(PathBuf),
    UnrepresentableWorkspace(PathBuf),
    ZeroOutputLimit,
    EmptyProgram,
    NulInCommand,
    Spec(SpecError),
    SpawnRuntime(io::Error),
    Wait(WaitError),
    MissingCapture(&'static str),
    RuntimeNotAttested { expected: String, observed: String },
}

impl fmt::Display for ContainerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeNotAbsolute(path) => {
                write!(
                    formatter,
                    "proof runtime path must be absolute: {}",
                    path.display()
                )
            }
            Self::InspectRuntime { path, source } => {
                write!(
                    formatter,
                    "cannot inspect proof runtime {}: {source}",
                    path.display()
                )
            }
            Self::RuntimeNotRegular(path) => {
                write!(
                    formatter,
                    "proof runtime is not a regular file: {}",
                    path.display()
                )
            }
            Self::RuntimeNotExecutable(path) => {
                write!(
                    formatter,
                    "proof runtime is not executable: {}",
                    path.display()
                )
            }
            Self::ImageNotDigestPinned(image) => write!(
                formatter,
                "proof image must be an exact sha256 digest reference, not a tag: {image:?}"
            ),
            Self::InspectWorkspace { path, source } => write!(
                formatter,
                "cannot inspect proof workspace {}: {source}",
                path.display()
            ),
            Self::WorkspaceNotDirectory(path) => {
                write!(
                    formatter,
                    "proof workspace is not a directory: {}",
                    path.display()
                )
            }
            Self::UnrepresentableWorkspace(path) => write!(
                formatter,
                "proof workspace cannot be represented in an OCI mount: {}",
                path.display()
            ),
            Self::ZeroOutputLimit => {
                formatter.write_str("proof output capture limit must be nonzero")
            }
            Self::EmptyProgram => formatter.write_str("proof battery program must not be empty"),
            Self::NulInCommand => formatter.write_str("proof battery command must not contain NUL"),
            Self::Spec(source) => write!(formatter, "invalid sealed process contract: {source}"),
            Self::SpawnRuntime(source) => {
                write!(formatter, "cannot start sealed runtime: {source}")
            }
            Self::Wait(source) => write!(formatter, "sealed runtime wait failed: {source}"),
            Self::MissingCapture(stream) => {
                write!(formatter, "sealed runtime returned no {stream} capture")
            }
            Self::RuntimeNotAttested { expected, observed } => write!(
                formatter,
                "sealed runtime did not prove its OCI isolation contract: expected {expected:?}, \
                 observed {observed:?}"
            ),
        }
    }
}

impl Error for ContainerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InspectRuntime { source, .. }
            | Self::InspectWorkspace { source, .. }
            | Self::SpawnRuntime(source) => Some(source),
            Self::Spec(source) => Some(source),
            Self::Wait(source) => Some(source),
            Self::RuntimeNotAbsolute(_)
            | Self::RuntimeNotRegular(_)
            | Self::RuntimeNotExecutable(_)
            | Self::ImageNotDigestPinned(_)
            | Self::WorkspaceNotDirectory(_)
            | Self::UnrepresentableWorkspace(_)
            | Self::ZeroOutputLimit
            | Self::EmptyProgram
            | Self::NulInCommand
            | Self::MissingCapture(_)
            | Self::RuntimeNotAttested { .. } => None,
        }
    }
}

pub(crate) fn is_digest_pinned(image: &str) -> bool {
    let digest = if let Some(digest) = image.strip_prefix("sha256:") {
        Some(digest)
    } else if let Some((repository, digest)) = image.rsplit_once("@sha256:") {
        (!repository.is_empty()).then_some(digest)
    } else {
        None
    };
    digest.is_some_and(|digest| {
        digest.len() == SHA256_HEX_LENGTH
            && digest.bytes().all(|byte| {
                byte.is_ascii_digit() || (byte.is_ascii_lowercase() && byte.is_ascii_hexdigit())
            })
    })
}

fn validate_command(program: &str, arguments: &[String]) -> Result<(), ContainerError> {
    if program.is_empty() {
        return Err(ContainerError::EmptyProgram);
    }
    if program.contains('\0') || arguments.iter().any(|argument| argument.contains('\0')) {
        return Err(ContainerError::NulInCommand);
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use sill::{Deadlines, StdinPolicy, StreamPolicy};

    use super::{Container, ContainerError, Limits, OciRuntime};

    #[test]
    fn every_spec_disables_network_and_mounts_fresh_empty_caches() {
        let fixture = Fixture::new();
        let container = fixture.container();
        let spec = container
            .spec("cargo", &["test".to_owned(), "--locked".to_owned()])
            .expect("sealed command");
        let arguments = spec
            .argv
            .arguments()
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(
            arguments,
            [
                "run".to_owned(),
                "--rm".to_owned(),
                "--network=none".to_owned(),
                "--read-only".to_owned(),
                "--cap-drop=ALL".to_owned(),
                "--security-opt=no-new-privileges".to_owned(),
                "--mount".to_owned(),
                format!(
                    "type=bind,source={},destination=/workspace,readonly",
                    fixture
                        .workspace
                        .canonicalize()
                        .expect("canonical workspace")
                        .display()
                ),
                "--mount".to_owned(),
                "type=tmpfs,destination=/cargo-home,tmpfs-mode=0700".to_owned(),
                "--mount".to_owned(),
                "type=tmpfs,destination=/cargo-target,tmpfs-mode=0700".to_owned(),
                "--workdir".to_owned(),
                "/workspace".to_owned(),
                "--env".to_owned(),
                "CARGO_HOME=/cargo-home".to_owned(),
                "--env".to_owned(),
                "CARGO_TARGET_DIR=/cargo-target".to_owned(),
                "--env".to_owned(),
                "CARGO_NET_OFFLINE=true".to_owned(),
                format!("registry.example/proof@sha256:{}", "4".repeat(64)),
                "/bin/sh".to_owned(),
                "-ceu".to_owned(),
                super::PREFLIGHT_SCRIPT.to_owned(),
                "spice-proof-attestation".to_owned(),
                "docker".to_owned(),
                "cargo".to_owned(),
                "test".to_owned(),
                "--locked".to_owned(),
            ]
        );
        assert!(spec.environment.entries().is_empty());
        assert_eq!(spec.output.stdin(), StdinPolicy::Null);
        assert!(matches!(
            spec.output.stdout(),
            StreamPolicy::Capture { limit: 4096 }
        ));
    }

    #[test]
    fn an_unpinned_image_refuses_instead_of_falling_back_to_a_tag() {
        let fixture = Fixture::new();
        let error = Container::new(
            OciRuntime::docker(&fixture.runtime).expect("fixture runtime"),
            "registry.example/proof:latest",
            &fixture.workspace,
            limits(),
        )
        .expect_err("tagged images are not sealed");

        assert!(matches!(error, ContainerError::ImageNotDigestPinned(_)));
    }

    #[test]
    fn an_executable_that_ignores_the_oci_contract_cannot_produce_a_replay_verdict() {
        let fixture = Fixture::new();
        let error = crate::replay::replay(&fixture.container())
            .expect_err("an unattested runtime cannot produce any verdict");

        assert_eq!(error.check(), crate::replay::CheckKind::Gate);
        assert!(
            error
                .evidence()
                .contains("did not prove its OCI isolation contract"),
            "the refusal did not name the missing attestation: {error:?}",
        );
    }

    #[test]
    fn attestation_is_required_once_and_removed_from_command_evidence() {
        let fixture = Fixture::new();
        let container = fixture.container();
        let mut stderr = super::Captured {
            bytes: b"spice-proof-attestation-v1:docker\ncompiler evidence".to_vec(),
            truncated: false,
            complete: true,
        };

        container
            .verify_attestation(&mut stderr)
            .expect("the declared dialect attests");

        assert_eq!(stderr.bytes, b"compiler evidence");
    }

    #[test]
    fn attestation_must_name_the_declared_runtime_dialect() {
        let fixture = Fixture::new();
        let container = fixture.container();
        let mut stderr = super::Captured {
            bytes: b"spice-proof-attestation-v1:podman\n".to_vec(),
            truncated: false,
            complete: true,
        };

        let error = container
            .verify_attestation(&mut stderr)
            .expect_err("a Podman attestation cannot answer for Docker");

        assert!(matches!(error, ContainerError::RuntimeNotAttested { .. }));
    }

    #[test]
    fn the_embedded_preflight_is_valid_posix_shell() {
        let status = std::process::Command::new("/bin/sh")
            .args(["-n", "-c", super::PREFLIGHT_SCRIPT])
            .status()
            .expect("host POSIX shell runs the syntax check");

        assert!(
            status.success(),
            "the sealed preflight must parse before use"
        );
    }

    struct Fixture {
        root: std::path::PathBuf,
        runtime: std::path::PathBuf,
        workspace: std::path::PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "spice-proof-container-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let workspace = root.join("workspace");
            let runtime = root.join("docker");
            fs::create_dir_all(&workspace).expect("fixture workspace");
            fs::write(&runtime, "#!/bin/sh\nexit 0\n").expect("fixture runtime");
            make_executable(&runtime);
            Self {
                root,
                runtime,
                workspace,
            }
        }

        fn container(&self) -> Container {
            Container::new(
                OciRuntime::docker(&self.runtime).expect("fixture runtime"),
                format!("registry.example/proof@sha256:{}", "4".repeat(64)),
                &self.workspace,
                limits(),
            )
            .expect("sealed fixture")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).expect("remove fixture");
        }
    }

    fn limits() -> Limits {
        Limits::new(
            Deadlines::new(
                Duration::from_secs(30),
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .expect("deadlines"),
            Duration::from_secs(1),
            4096,
        )
        .expect("limits")
    }

    #[cfg(unix)]
    fn make_executable(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .expect("make runtime executable");
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &std::path::Path) {}
}
