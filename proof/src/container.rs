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
use wire::manifest::ProofImage;

use crate::challenge::Challenge;

const WORKSPACE_MOUNT: &str = "/workspace";
const CARGO_HOME: &str = "/cargo-home";
const CARGO_TARGET: &str = "/cargo-target";
const ATTESTATION_PREFIX: &str = "spice-proof-attestation-v2:";

/// The rule deciding whether a network namespace can carry traffic off
/// loopback, as one shell function over a `/sys/class/net` directory.
///
/// A fresh namespace is not an empty one. The kernel materializes DOWN tunnel
/// stubs — `gre0`, `sit0`, `tunl0`, `ip6tnl0` and their relatives — as soon as
/// those modules are loaded anywhere on the host, and `/sys/class/net` also
/// holds `bonding_masters`, which is a control file and not an interface at
/// all. Deciding on the name `lo` therefore turned away exactly the isolated
/// `--network=none` container this boundary exists to admit.
///
/// So the rule asks what an interface can do rather than what it is called.
/// Only an administratively up interface can carry a packet, which admits
/// every stub while it stays down, and `IFF_LOOPBACK` is set by the kernel for
/// the loopback device alone, which is what a name comparison could not check:
/// a real adapter renamed `lo` used to pass on its name and no longer does.
///
/// Kept as its own text so the rule the tests exercise is the rule the sealed
/// container runs, rather than a second copy that could drift from it.
macro_rules! network_rule {
    () => {
        r#"
network_isolated() {
    for interface in "$1"/*; do
        [ -d "$interface" ] || continue
        read -r flags < "$interface/flags" || return 1
        [ $(( flags & 0x1 )) -ne 0 ] || continue
        [ $(( flags & 0x8 )) -ne 0 ] || return 1
    done
    return 0
}
"#
    };
}

/// The rule deciding whether the caller's mounts are the ones this boundary
/// grants, as one shell function over a `mountinfo` file.
///
/// It names the first bound not met rather than returning a bare failure, so
/// the refusal a member sees still says which mount was wrong. Kept as its own
/// text for the same reason as [`network_rule`]: the mount flags on the other
/// side of this contract change, and an assertion nothing exercises is an
/// assertion that could stop biting without anyone noticing.
macro_rules! mount_rule {
    () => {
        r#"
mount_violation() {
    workspace_read_only=false
    cargo_home_tmpfs=false
    cargo_target_tmpfs=false
    while IFS= read -r mount; do
        case "$mount" in
            *" /workspace ro,"*|*" /workspace ro "*) workspace_read_only=true ;;
            *" /cargo-home "*" - tmpfs "*) cargo_home_tmpfs=true ;;
            *" /cargo-target "*" - tmpfs "*) cargo_target_tmpfs=true ;;
        esac
    done < "$1"
    [ "$workspace_read_only" = true ] || { echo workspace-not-read-only; return; }
    [ "$cargo_home_tmpfs" = true ] || { echo cargo-home-not-tmpfs; return; }
    [ "$cargo_target_tmpfs" = true ] || { echo cargo-target-not-tmpfs; return; }
}
"#
    };
}

const PREFLIGHT_SCRIPT: &str = concat!(
    network_rule!(),
    mount_rule!(),
    r#"
refuse() {
    printf 'spice-proof-preflight-refused:%s\n' "$1" >&2
    exit 70
}

[ "${CARGO_HOME:-}" = /cargo-home ] || refuse cargo-home-environment
[ "${CARGO_TARGET_DIR:-}" = /cargo-target ] || refuse cargo-target-environment
[ "${CARGO_MUTANTS_OUTPUT:-}" = /cargo-target ] || refuse mutants-output-environment
[ -d /cargo-home ] || refuse cargo-home-missing
[ -d /cargo-target ] || refuse cargo-target-missing
[ -d /sys/class/net ] || refuse network-state-unavailable

violation=$(mount_violation /proc/self/mountinfo)
[ -z "$violation" ] || refuse "$violation"

for cache in /cargo-home /cargo-target; do
    for entry in "$cache"/* "$cache"/.[!.]* "$cache"/..?*; do
        [ ! -e "$entry" ] || refuse cache-not-empty
    done
done
network_isolated /sys/class/net || refuse network-reaches-past-loopback

dialect=$1
challenge=$2
shift 2
printf 'spice-proof-attestation-v2:%s:%s\n' "$dialect" "$challenge" >&2
exec "$@"
"#
);

/// One observed OCI runtime interface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OciRuntime {
    kind: RuntimeKind,
    binary: PathBuf,
}

impl OciRuntime {
    /// Admit one executable whose bounded identity response names Docker or Podman.
    ///
    /// # Errors
    ///
    /// Refuses non-absolute, missing, non-regular, or non-executable paths and
    /// any failed, incomplete, ambiguous, or unrecognized identity exchange.
    pub fn attest(binary: impl AsRef<Path>, limits: Limits) -> Result<Self, ContainerError> {
        let binary = binary.as_ref();
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
        let kind = observe_runtime_kind(&binary, limits)?;
        Ok(Self { kind, binary })
    }

    /// Exact runtime executable admitted above.
    #[must_use]
    pub fn binary(&self) -> &Path {
        &self.binary
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RuntimeKind {
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct Attestation {
    challenge: Challenge,
}

impl Attestation {
    fn fresh() -> Result<Self, ContainerError> {
        Ok(Self {
            challenge: Challenge::fresh().map_err(ContainerError::CreateAttestationChallenge)?,
        })
    }

    fn marker(&self, kind: RuntimeKind) -> String {
        format!(
            "{ATTESTATION_PREFIX}{}:{}\n",
            kind.as_str(),
            self.challenge.as_str()
        )
    }

    #[cfg(test)]
    fn fixture(challenge: &str) -> Self {
        Self {
            challenge: Challenge::fixture(challenge),
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

    pub(crate) fn process_spec(
        self,
        argv: Argv,
        cwd: PathBuf,
        label: impl Into<String>,
    ) -> Result<Spec, SpecError> {
        Ok(Spec {
            argv,
            cwd,
            environment: Environment::empty(),
            group: ProcessGroup::New,
            deadlines: self.deadlines,
            wait: WaitPolicy::new(label, Cancellation::Never, RestartDisposition::Never)?,
            output: Output::new(
                StdinPolicy::Null,
                StreamPolicy::Capture {
                    limit: self.output_limit,
                },
                StreamPolicy::Capture {
                    limit: self.output_limit,
                },
            ),
            redaction: Redaction::None,
            grace: self.grace,
        })
    }
}

/// Why one bounded runtime probe produced no evidence.
///
/// The three process failures stay typed because they say something about this
/// host rather than about the runtime's answer; everything else collapses into
/// the single observation string its caller reports.
enum ProbeRefusal {
    Spec(SpecError),
    Spawn(io::Error),
    Wait(WaitError),
    Observed(String),
}

impl ProbeRefusal {
    fn dialect(self) -> ContainerError {
        match self {
            Self::Spec(source) => ContainerError::RuntimeProbeSpec(source),
            Self::Spawn(source) => ContainerError::SpawnRuntimeProbe(source),
            Self::Wait(source) => ContainerError::WaitRuntimeProbe(source),
            Self::Observed(observed) => ContainerError::RuntimeDialectNotAttested { observed },
        }
    }

    fn image(self, image: &ProofImage) -> ContainerError {
        match self {
            Self::Spec(source) => ContainerError::RuntimeProbeSpec(source),
            Self::Spawn(source) => ContainerError::SpawnRuntimeProbe(source),
            Self::Wait(source) => ContainerError::WaitRuntimeProbe(source),
            Self::Observed(observed) => ContainerError::ImageNotResolved {
                image: image.as_str().to_owned(),
                observed,
            },
        }
    }
}

/// Run one bounded runtime question and return the exact bytes it answered.
///
/// An answer counts only when the process exited successfully, both streams
/// were captured whole, and stderr stayed silent. Anything else is the
/// observation, and the caller decides which refusal it names.
fn probe(
    binary: &Path,
    arguments: &[&str],
    label: &'static str,
    limits: Limits,
) -> Result<Vec<u8>, ProbeRefusal> {
    let spec = Spec {
        argv: Argv::new(
            binary.as_os_str().to_owned(),
            arguments.iter().map(OsString::from),
        )
        .map_err(ProbeRefusal::Spec)?,
        cwd: binary
            .parent()
            .expect("a canonical absolute executable has a parent")
            .to_owned(),
        environment: Environment::empty(),
        group: ProcessGroup::New,
        deadlines: limits.deadlines,
        wait: WaitPolicy::new(label, Cancellation::Never, RestartDisposition::Never)
            .map_err(ProbeRefusal::Spec)?,
        output: Output::new(
            StdinPolicy::Null,
            StreamPolicy::Capture {
                limit: limits.output_limit,
            },
            StreamPolicy::Capture {
                limit: limits.output_limit,
            },
        ),
        redaction: Redaction::None,
        grace: limits.grace,
    };
    let running = {
        #[cfg(test)]
        let _spawn_guard = crate::test_support::process_spawn_guard();
        sill::spawn(spec)
    }
    .map_err(ProbeRefusal::Spawn)?;
    let outcome = running.wait().map_err(ProbeRefusal::Wait)?;
    let completion = format!("{:?}", outcome.completion());
    let stdout = outcome.stdout().map(Captured::from).ok_or_else(|| {
        ProbeRefusal::Observed(format!("completion={completion}, stdout capture missing"))
    })?;
    let stderr = outcome.stderr().map(Captured::from).ok_or_else(|| {
        ProbeRefusal::Observed(format!("completion={completion}, stderr capture missing"))
    })?;
    let observed = format!(
        "completion={completion}, stdout={:?}, stderr={:?}",
        String::from_utf8_lossy(&stdout.bytes),
        String::from_utf8_lossy(&stderr.bytes)
    );
    if !matches!(outcome.completion(), Completion::Exited(status) if status.success())
        || stdout.truncated
        || stderr.truncated
        || !stdout.complete
        || !stderr.complete
        || !stderr.bytes.is_empty()
    {
        return Err(ProbeRefusal::Observed(observed));
    }
    Ok(stdout.bytes)
}

fn observe_runtime_kind(binary: &Path, limits: Limits) -> Result<RuntimeKind, ContainerError> {
    let stdout = probe(
        binary,
        &["--version"],
        "sealed OCI runtime identity probe",
        limits,
    )
    .map_err(ProbeRefusal::dialect)?;
    parse_runtime_kind(&stdout).ok_or_else(|| ContainerError::RuntimeDialectNotAttested {
        observed: format!("stdout={:?}", String::from_utf8_lossy(&stdout)),
    })
}

/// The one platform the sealed Linux appliance is assembled for.
const IMAGE_PLATFORM: &str = "amd64 linux";

/// Resolve the pinned identity against what this host actually holds.
///
/// A digest reference does not verify itself. Docker looks a bare image ID up
/// directly, but Podman resolves a manifest digest through its local store and
/// can answer with an image carrying a different identity, so the identity is
/// read back rather than assumed. Resolution is also the only step that tells a
/// host which never loaded the appliance apart from one holding the wrong one,
/// and the platform is read in the same breath because an `arm64` image would
/// otherwise satisfy every later check while building the wrong binaries.
fn resolve_image(
    runtime: &OciRuntime,
    image: &ProofImage,
    limits: Limits,
) -> Result<(), ContainerError> {
    let stdout = probe(
        &runtime.binary,
        &[
            "image",
            "inspect",
            "--format",
            "{{.Id}} {{.Architecture}} {{.Os}}",
            image.as_str(),
        ],
        "sealed proof image resolution",
        limits,
    )
    .map_err(|refusal| refusal.image(image))?;
    let observed = String::from_utf8_lossy(&stdout).trim().to_owned();
    let expected = format!("{} {IMAGE_PLATFORM}", image.as_str());
    if observed != expected {
        return Err(ContainerError::ImageIdentityDrift { expected, observed });
    }
    Ok(())
}

fn parse_runtime_kind(bytes: &[u8]) -> Option<RuntimeKind> {
    let version = std::str::from_utf8(bytes).ok()?;
    let version = version
        .strip_suffix("\r\n")
        .or_else(|| version.strip_suffix('\n'))
        .unwrap_or(version);
    if version.contains(['\r', '\n']) {
        return None;
    }
    if version
        .strip_prefix("Docker version ")
        .is_some_and(|remainder| !remainder.is_empty())
    {
        Some(RuntimeKind::Docker)
    } else if version
        .strip_prefix("podman version ")
        .is_some_and(|remainder| !remainder.is_empty())
    {
        Some(RuntimeKind::Podman)
    } else {
        None
    }
}

/// A network-disabled build environment with fresh empty cache mounts.
#[derive(Clone, Debug)]
pub struct Container {
    runtime: OciRuntime,
    image: ProofImage,
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
        let image = image
            .parse::<ProofImage>()
            .map_err(|_| ContainerError::ImageNotDigestPinned(image))?;
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
        resolve_image(&runtime, &image, limits)?;

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
        self.image.as_str()
    }

    pub(crate) const fn proof_image(&self) -> &ProofImage {
        &self.image
    }

    fn spec(
        &self,
        program: &str,
        arguments: &[String],
        attestation: &Attestation,
    ) -> Result<Spec, ContainerError> {
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
            // Spelled with `--tmpfs` rather than `--mount type=tmpfs` because
            // only this flag takes the execute permission below: both admitted
            // runtimes accept arbitrary mount options here, while `--mount`
            // accepts only a size and a mode and rejects anything else. The
            // size default is unchanged by the switch, half of memory either
            // way, and both mounts stay `nosuid,nodev` and stay tmpfs, which is
            // what the preflight reads them for.
            "--tmpfs".to_owned(),
            format!("{CARGO_HOME}:rw,noexec,mode=0700"),
            // The one mount that may execute, because it is the one members
            // write programs to: every member compiles into `CARGO_TARGET_DIR`
            // and then runs what it compiled, build scripts first and test
            // binaries after. Granted here alone rather than to both, so the
            // Cargo home stays a directory nothing can be run out of.
            "--tmpfs".to_owned(),
            format!("{CARGO_TARGET}:rw,exec,mode=0700"),
            "--workdir".to_owned(),
            WORKSPACE_MOUNT.to_owned(),
            "--env".to_owned(),
            format!("CARGO_HOME={CARGO_HOME}"),
            "--env".to_owned(),
            format!("CARGO_TARGET_DIR={CARGO_TARGET}"),
            // The mutation member writes `mutants.out` beside the sources it
            // mutates unless it is told otherwise, and the sources are the
            // read-only mount above. Pointed at the target tmpfs, which is the
            // writable scratch this boundary already grants, so the member is
            // given the one thing it needs rather than the mount being relaxed.
            "--env".to_owned(),
            format!("CARGO_MUTANTS_OUTPUT={CARGO_TARGET}"),
            "--env".to_owned(),
            "CARGO_NET_OFFLINE=true".to_owned(),
            self.image.to_string(),
            "/bin/sh".to_owned(),
            "-ceu".to_owned(),
            PREFLIGHT_SCRIPT.to_owned(),
            "spice-proof-attestation".to_owned(),
            self.runtime.kind.as_str().to_owned(),
            attestation.challenge.as_str().to_owned(),
            program.to_owned(),
        ];
        runtime_arguments.extend(arguments.iter().cloned());

        self.limits
            .process_spec(
                Argv::new(
                    self.runtime.binary.as_os_str().to_owned(),
                    runtime_arguments.into_iter().map(OsString::from),
                )
                .map_err(ContainerError::Spec)?,
                self.workspace.clone(),
                format!("sealed proof battery member {program:?}"),
            )
            .map_err(ContainerError::Spec)
    }

    pub(crate) fn execute(
        &self,
        program: &str,
        arguments: &[String],
    ) -> Result<Execution, ContainerError> {
        let attestation = Attestation::fresh()?;
        let running = {
            #[cfg(test)]
            let _spawn_guard = crate::test_support::process_spawn_guard();
            sill::spawn(self.spec(program, arguments, &attestation)?)
        }
        .map_err(ContainerError::SpawnRuntime)?;
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
        self.verify_attestation(&mut stderr, &attestation)?;
        Ok(Execution {
            completion,
            stdout,
            stderr,
        })
    }

    fn verify_attestation(
        &self,
        stderr: &mut Captured,
        attestation: &Attestation,
    ) -> Result<(), ContainerError> {
        let expected = attestation.marker(self.runtime.kind);
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
    RuntimeProbeSpec(SpecError),
    SpawnRuntimeProbe(io::Error),
    WaitRuntimeProbe(WaitError),
    RuntimeDialectNotAttested { observed: String },
    ImageNotDigestPinned(String),
    ImageNotResolved { image: String, observed: String },
    ImageIdentityDrift { expected: String, observed: String },
    InspectWorkspace { path: PathBuf, source: io::Error },
    WorkspaceNotDirectory(PathBuf),
    UnrepresentableWorkspace(PathBuf),
    ZeroOutputLimit,
    EmptyProgram,
    NulInCommand,
    CreateAttestationChallenge(io::Error),
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
            Self::RuntimeProbeSpec(source) => {
                write!(
                    formatter,
                    "invalid sealed OCI runtime identity probe: {source}"
                )
            }
            Self::SpawnRuntimeProbe(source) => {
                write!(
                    formatter,
                    "cannot start sealed OCI runtime identity probe: {source}"
                )
            }
            Self::WaitRuntimeProbe(source) => {
                write!(
                    formatter,
                    "sealed OCI runtime identity probe failed: {source}"
                )
            }
            Self::RuntimeDialectNotAttested { observed } => write!(
                formatter,
                "proof runtime did not attest a Docker or Podman interface: {observed}"
            ),
            Self::ImageNotDigestPinned(image) => write!(
                formatter,
                "proof image must be an exact sha256 digest reference, not a tag: {image:?}"
            ),
            Self::ImageNotResolved { image, observed } => write!(
                formatter,
                "proof runtime does not hold proof image {image}, so load it with \
                 dist/proof-image/load.sh before sealing: {observed}"
            ),
            Self::ImageIdentityDrift { expected, observed } => write!(
                formatter,
                "proof runtime resolved a different appliance: expected {expected:?}, \
                 observed {observed:?}"
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
            Self::CreateAttestationChallenge(source) => {
                write!(
                    formatter,
                    "cannot create sealed runtime challenge: {source}"
                )
            }
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
            | Self::CreateAttestationChallenge(source)
            | Self::SpawnRuntimeProbe(source)
            | Self::SpawnRuntime(source) => Some(source),
            Self::RuntimeProbeSpec(source) | Self::Spec(source) => Some(source),
            Self::WaitRuntimeProbe(source) | Self::Wait(source) => Some(source),
            Self::RuntimeNotAbsolute(_)
            | Self::RuntimeNotRegular(_)
            | Self::RuntimeNotExecutable(_)
            | Self::RuntimeDialectNotAttested { .. }
            | Self::ImageNotDigestPinned(_)
            | Self::ImageNotResolved { .. }
            | Self::ImageIdentityDrift { .. }
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

pub(crate) fn validate_command(program: &str, arguments: &[String]) -> Result<(), ContainerError> {
    if program.is_empty() {
        return Err(ContainerError::EmptyProgram);
    }
    if program.contains('\0') || arguments.iter().any(|argument| argument.contains('\0')) {
        return Err(ContainerError::NulInCommand);
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
pub(crate) fn is_executable(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use sill::{Deadlines, StdinPolicy, StreamPolicy};

    use super::{Attestation, Container, ContainerError, Limits, OciRuntime, RuntimeKind};

    const SILENT_RUNTIME: &str = r#"#!/bin/sh
if [ "$1" = image ]; then
    printf '%s amd64 linux\n' "$5"
    exit 0
fi
if [ "$1" = --version ]; then
    printf 'Docker version 1.0.0, build fixture\n'
    exit 0
fi
exit 0
"#;
    const FIXED_MARKER_RUNTIME: &str = r#"#!/bin/sh
if [ "$1" = image ]; then
    printf '%s amd64 linux\n' "$5"
    exit 0
fi
if [ "$1" = --version ]; then
    printf 'Docker version 1.0.0, build fixture\n'
    exit 0
fi
printf 'spice-proof-attestation-v2:docker:fixed-challenge\n' >&2
exit 0
"#;
    const PODMAN_RUNTIME: &str = r#"#!/bin/sh
if [ "$1" = image ]; then
    printf '%s amd64 linux\n' "$5"
    exit 0
fi
if [ "$1" = --version ]; then
    printf 'podman version 1.0.0\n'
    exit 0
fi
exit 0
"#;

    /// A runtime that resolves the sealed digest to some other stored image.
    const SUBSTITUTING_RUNTIME: &str = r#"#!/bin/sh
if [ "$1" = image ]; then
    printf 'sha256:0000000000000000000000000000000000000000000000000000000000000000 amd64 linux\n'
    exit 0
fi
if [ "$1" = --version ]; then
    printf 'Docker version 1.0.0, build fixture\n'
    exit 0
fi
exit 0
"#;

    /// A runtime holding the sealed digest built for the wrong machine.
    const FOREIGN_PLATFORM_RUNTIME: &str = r#"#!/bin/sh
if [ "$1" = image ]; then
    printf '%s arm64 linux\n' "$5"
    exit 0
fi
if [ "$1" = --version ]; then
    printf 'Docker version 1.0.0, build fixture\n'
    exit 0
fi
exit 0
"#;

    /// A runtime whose store has never seen the sealed appliance.
    const EMPTY_STORE_RUNTIME: &str = r#"#!/bin/sh
if [ "$1" = image ]; then
    printf 'Error: no such image\n' >&2
    exit 1
fi
if [ "$1" = --version ]; then
    printf 'Docker version 1.0.0, build fixture\n'
    exit 0
fi
exit 0
"#;

    #[test]
    fn every_spec_disables_network_and_mounts_fresh_empty_caches() {
        let fixture = Fixture::new();
        let container = fixture.container();
        let attestation = Attestation::fixture("fixture-challenge");
        let spec = container
            .spec(
                "cargo",
                &["test".to_owned(), "--locked".to_owned()],
                &attestation,
            )
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
                "--tmpfs".to_owned(),
                "/cargo-home:rw,noexec,mode=0700".to_owned(),
                "--tmpfs".to_owned(),
                "/cargo-target:rw,exec,mode=0700".to_owned(),
                "--workdir".to_owned(),
                "/workspace".to_owned(),
                "--env".to_owned(),
                "CARGO_HOME=/cargo-home".to_owned(),
                "--env".to_owned(),
                "CARGO_TARGET_DIR=/cargo-target".to_owned(),
                "--env".to_owned(),
                "CARGO_MUTANTS_OUTPUT=/cargo-target".to_owned(),
                "--env".to_owned(),
                "CARGO_NET_OFFLINE=true".to_owned(),
                format!("registry.example/proof@sha256:{}", "4".repeat(64)),
                "/bin/sh".to_owned(),
                "-ceu".to_owned(),
                super::PREFLIGHT_SCRIPT.to_owned(),
                "spice-proof-attestation".to_owned(),
                "docker".to_owned(),
                "fixture-challenge".to_owned(),
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
            OciRuntime::attest(&fixture.runtime, limits()).expect("fixture runtime"),
            "registry.example/proof:latest",
            &fixture.workspace,
            limits(),
        )
        .expect_err("tagged images are not sealed");

        assert!(matches!(error, ContainerError::ImageNotDigestPinned(_)));
    }

    #[test]
    fn a_runtime_that_cannot_produce_the_sealed_image_refuses_before_any_work() {
        let sealed = format!("registry.example/proof@sha256:{}", "4".repeat(64));
        for (script, expected) in [
            (SUBSTITUTING_RUNTIME, "resolved a different appliance"),
            (FOREIGN_PLATFORM_RUNTIME, "arm64"),
            (EMPTY_STORE_RUNTIME, "does not hold proof image"),
        ] {
            let fixture = Fixture::with_runtime(script);
            let error = Container::new(
                OciRuntime::attest(&fixture.runtime, limits()).expect("fixture runtime"),
                sealed.clone(),
                &fixture.workspace,
                limits(),
            )
            .expect_err("a runtime that cannot produce the sealed image is not sealed");

            assert!(
                error.to_string().contains(expected),
                "the refusal did not name {expected}: {error}",
            );
        }
    }

    #[test]
    fn an_executable_that_ignores_the_oci_contract_cannot_produce_a_replay_verdict() {
        let fixture = Fixture::new();
        let environment = crate::environment::SealedEnvironment::fixture_oci(fixture.container());
        let error = crate::replay::replay(&environment)
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
    fn a_replayed_public_marker_cannot_produce_a_replay_verdict() {
        let fixture = Fixture::with_runtime(FIXED_MARKER_RUNTIME);
        let environment = crate::environment::SealedEnvironment::fixture_oci(fixture.container());
        let error = crate::replay::replay(&environment)
            .expect_err("a fixed marker cannot answer a fresh challenge");

        assert_eq!(error.check(), crate::replay::CheckKind::Gate);
        assert!(
            error
                .evidence()
                .contains("did not prove its OCI isolation contract"),
            "the refusal did not name the stale attestation: {error:?}",
        );
    }

    #[test]
    fn runtime_dialect_is_observed_from_the_executable() {
        let docker = Fixture::new();
        let podman = Fixture::with_runtime(PODMAN_RUNTIME);

        assert_eq!(
            OciRuntime::attest(&docker.runtime, limits())
                .expect("observed Docker")
                .kind,
            RuntimeKind::Docker
        );
        assert_eq!(
            OciRuntime::attest(&podman.runtime, limits())
                .expect("observed Podman")
                .kind,
            RuntimeKind::Podman
        );
    }

    #[test]
    fn attestation_is_required_once_and_removed_from_command_evidence() {
        let fixture = Fixture::new();
        let container = fixture.container();
        let attestation = Attestation::fixture("fresh-challenge");
        let mut stderr = super::Captured {
            bytes: b"spice-proof-attestation-v2:docker:fresh-challenge\ncompiler evidence".to_vec(),
            truncated: false,
            complete: true,
        };

        container
            .verify_attestation(&mut stderr, &attestation)
            .expect("the declared dialect attests");

        assert_eq!(stderr.bytes, b"compiler evidence");
    }

    #[test]
    fn attestation_must_name_the_declared_runtime_dialect() {
        let fixture = Fixture::new();
        let container = fixture.container();
        let attestation = Attestation::fixture("fresh-challenge");
        let mut stderr = super::Captured {
            bytes: b"spice-proof-attestation-v2:podman:fresh-challenge\n".to_vec(),
            truncated: false,
            complete: true,
        };

        let error = container
            .verify_attestation(&mut stderr, &attestation)
            .expect_err("a Podman attestation cannot answer for Docker");

        assert!(matches!(error, ContainerError::RuntimeNotAttested { .. }));
    }

    #[test]
    fn attestation_must_answer_the_current_execution_challenge() {
        let fixture = Fixture::new();
        let container = fixture.container();
        let attestation = Attestation::fixture("fresh-challenge");
        let mut stderr = super::Captured {
            bytes: b"spice-proof-attestation-v2:docker:stale-challenge\n".to_vec(),
            truncated: false,
            complete: true,
        };

        let error = container
            .verify_attestation(&mut stderr, &attestation)
            .expect_err("a marker from another execution cannot be replayed");

        assert!(matches!(error, ContainerError::RuntimeNotAttested { .. }));
    }

    #[test]
    fn the_embedded_preflight_is_valid_posix_shell() {
        let _spawn_guard = crate::test_support::process_spawn_guard();
        let status = std::process::Command::new("/bin/sh")
            .args(["-n", "-c", super::PREFLIGHT_SCRIPT])
            .status()
            .expect("host POSIX shell runs the syntax check");

        assert!(
            status.success(),
            "the sealed preflight must parse before use"
        );
    }

    /// A `/sys/class/net` directory holding the named interfaces and their
    /// flags, alongside the `bonding_masters` control file the kernel puts
    /// there, which is a file rather than an interface directory.
    fn network_directory(interfaces: &[(&str, u32)]) -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-proof-net-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("fixture network directory");
        fs::write(root.join("bonding_masters"), "\n").expect("fixture control file");
        for (name, flags) in interfaces {
            let interface = root.join(name);
            fs::create_dir_all(&interface).expect("fixture interface");
            fs::write(interface.join("flags"), format!("{flags:#x}\n")).expect("fixture flags");
        }
        root
    }

    /// Whether the shipped rule calls that directory isolated.
    fn network_isolated(directory: &std::path::Path) -> bool {
        let _spawn_guard = crate::test_support::process_spawn_guard();
        std::process::Command::new("/bin/sh")
            .args([
                std::ffi::OsStr::new("-c"),
                std::ffi::OsStr::new(concat!(network_rule!(), "\nnetwork_isolated \"$1\"\n")),
                std::ffi::OsStr::new("sh"),
                directory.as_os_str(),
            ])
            .status()
            .expect("host POSIX shell runs the network rule")
            .success()
    }

    /// The mountinfo line the boundary produces for one destination.
    fn mountinfo_line(destination: &str, options: &str, filesystem: &str) -> String {
        format!("153 144 0:77 / {destination} {options},relatime - {filesystem} {filesystem} rw")
    }

    /// Which bound the shipped rule reports unmet, empty when all are met.
    fn mount_violation(lines: &[String]) -> String {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let _spawn_guard = crate::test_support::process_spawn_guard();
        let path = std::env::temp_dir().join(format!(
            "spice-proof-mountinfo-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, lines.join("\n") + "\n").expect("fixture mountinfo");
        let output = std::process::Command::new("/bin/sh")
            .args([
                std::ffi::OsStr::new("-c"),
                std::ffi::OsStr::new(concat!(mount_rule!(), "\nmount_violation \"$1\"\n")),
                std::ffi::OsStr::new("sh"),
                path.as_os_str(),
            ])
            .output()
            .expect("host POSIX shell runs the mount rule");
        String::from_utf8(output.stdout)
            .expect("refusal names are ASCII")
            .trim()
            .to_owned()
    }

    /// The mounts this boundary actually grants, as the runtime reports them.
    fn granted_mounts() -> Vec<String> {
        vec![
            mountinfo_line("/workspace", "ro,relatime", "ext4"),
            mountinfo_line("/cargo-home", "rw,nosuid,nodev,noexec", "tmpfs"),
            mountinfo_line("/cargo-target", "rw,nosuid,nodev", "tmpfs"),
        ]
    }

    #[test]
    fn the_mounts_this_boundary_grants_meet_every_bound_it_asserts() {
        assert_eq!(
            mount_violation(&granted_mounts()),
            "",
            "the executable target mount must still satisfy the preflight it passes today"
        );
    }

    #[test]
    fn each_unmet_mount_bound_is_refused_by_its_own_name() {
        let writable_workspace = vec![
            mountinfo_line("/workspace", "rw,relatime", "ext4"),
            mountinfo_line("/cargo-home", "rw,nosuid,nodev,noexec", "tmpfs"),
            mountinfo_line("/cargo-target", "rw,nosuid,nodev", "tmpfs"),
        ];
        let disk_backed_home = vec![
            mountinfo_line("/workspace", "ro,relatime", "ext4"),
            mountinfo_line("/cargo-home", "rw,nosuid,nodev,noexec", "ext4"),
            mountinfo_line("/cargo-target", "rw,nosuid,nodev", "tmpfs"),
        ];
        let absent_target = vec![
            mountinfo_line("/workspace", "ro,relatime", "ext4"),
            mountinfo_line("/cargo-home", "rw,nosuid,nodev,noexec", "tmpfs"),
        ];

        assert_eq!(
            mount_violation(&writable_workspace),
            "workspace-not-read-only"
        );
        assert_eq!(mount_violation(&disk_backed_home), "cargo-home-not-tmpfs");
        assert_eq!(mount_violation(&absent_target), "cargo-target-not-tmpfs");
    }

    #[test]
    fn a_fresh_namespace_of_down_tunnel_stubs_is_isolated() {
        let directory = network_directory(&[
            ("lo", 0x9),
            ("erspan0", 0x1002),
            ("gre0", 0x80),
            ("gretap0", 0x1002),
            ("ip6tnl0", 0x80),
            ("sit0", 0x80),
            ("tunl0", 0x80),
        ]);

        assert!(
            network_isolated(&directory),
            "a --network=none namespace carries no traffic through interfaces that are all down"
        );
    }

    #[test]
    fn an_interface_that_can_carry_traffic_is_refused_whatever_it_is_named() {
        let named_lo = network_directory(&[("lo", 0x1003)]);
        let named_eth0 = network_directory(&[("lo", 0x9), ("eth0", 0x1003)]);

        assert!(
            !network_isolated(&named_lo),
            "an up adapter without IFF_LOOPBACK reaches the network under any name"
        );
        assert!(
            !network_isolated(&named_eth0),
            "an up adapter beside real loopback reaches the network"
        );
    }

    struct Fixture {
        root: std::path::PathBuf,
        runtime: std::path::PathBuf,
        workspace: std::path::PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with_runtime(SILENT_RUNTIME)
        }

        fn with_runtime(script: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "spice-proof-container-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let workspace = root.join("workspace");
            let runtime = root.join("docker");
            fs::create_dir_all(&workspace).expect("fixture workspace");
            {
                let _write_guard = crate::test_support::executable_write_guard();
                fs::write(&runtime, script).expect("fixture runtime");
                make_executable(&runtime);
            }
            Self {
                root,
                runtime,
                workspace,
            }
        }

        fn container(&self) -> Container {
            let limits = limits();
            Container::new(
                OciRuntime::attest(&self.runtime, limits).expect("fixture runtime"),
                format!("registry.example/proof@sha256:{}", "4".repeat(64)),
                &self.workspace,
                limits,
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
