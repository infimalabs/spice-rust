use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read as _, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;

const SECURITY: &str = "/usr/bin/security";
const CODESIGN: &str = "/usr/bin/codesign";
const DITTO: &str = "/usr/bin/ditto";
const XCRUN: &str = "/usr/bin/xcrun";
const SPCTL: &str = "/usr/sbin/spctl";
const DEVELOPER_ID_PREFIX: &str = "Developer ID Application: ";
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Operator-owned inputs for making one built Darwin executable shareable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShareDarwinRequest {
    executable: PathBuf,
    identity: String,
    notary_profile: String,
}

impl ShareDarwinRequest {
    #[must_use]
    pub fn new(
        executable: impl Into<PathBuf>,
        identity: impl Into<String>,
        notary_profile: impl Into<String>,
    ) -> Self {
        Self {
            executable: executable.into(),
            identity: identity.into(),
            notary_profile: notary_profile.into(),
        }
    }
}

/// Sign, notarize, and verify one built Darwin executable in place.
///
/// The caller supplies only names for credentials already owned by macOS. No
/// certificate, private key, or notarization credential enters the repository.
///
/// # Errors
///
/// Returns [`ShareDarwinError`] when the host, input, credentials, Apple
/// submission, or either verification step refuses the operation.
pub fn share_darwin(
    request: &ShareDarwinRequest,
    output: &mut impl Write,
) -> Result<(), ShareDarwinError> {
    if !cfg!(target_os = "macos") {
        return Err(ShareDarwinError::UnsupportedHost);
    }
    share_with(request, output, &SystemRunner)
}

fn share_with(
    request: &ShareDarwinRequest,
    output: &mut impl Write,
    runner: &impl Runner,
) -> Result<(), ShareDarwinError> {
    admit_executable(&request.executable)?;
    admit_identity(&request.identity, runner)?;
    admit_notary_profile(&request.notary_profile, runner)?;

    required(
        "sign executable",
        runner.run(
            Path::new(CODESIGN),
            &arguments_os([
                OsStr::new("--force"),
                OsStr::new("--options"),
                OsStr::new("runtime"),
                OsStr::new("--timestamp"),
                OsStr::new("--sign"),
                OsStr::new(&request.identity),
                request.executable.as_os_str(),
            ]),
        )?,
    )?;
    verify_signature(&request.executable, runner)?;

    let temporary = TemporaryDirectory::create()?;
    let archive = temporary.path().join("spice.zip");
    required(
        "archive signed executable",
        runner.run(
            Path::new(DITTO),
            &arguments_os([
                OsStr::new("-c"),
                OsStr::new("-k"),
                OsStr::new("--keepParent"),
                request.executable.as_os_str(),
                archive.as_os_str(),
            ]),
        )?,
    )?;

    let submission = runner.run(
        Path::new(XCRUN),
        &arguments_os([
            OsStr::new("notarytool"),
            OsStr::new("submit"),
            archive.as_os_str(),
            OsStr::new("--keychain-profile"),
            OsStr::new(&request.notary_profile),
            OsStr::new("--wait"),
            OsStr::new("--output-format"),
            OsStr::new("json"),
        ]),
    )?;
    let answer =
        serde_json::from_slice::<NotarySubmission>(&submission.stdout).map_err(|source| {
            ShareDarwinError::MalformedNotaryReply {
                source,
                detail: submission.detail(),
            }
        })?;
    if !submission.success || answer.status != "Accepted" {
        return Err(rejected_submission(
            &request.notary_profile,
            answer,
            submission,
            runner,
        ));
    }

    verify_signature(&request.executable, runner)?;
    // `--type execute` judges only app bundles and rejects every bare Mach-O;
    // the primary-signature open assessment is Gatekeeper's verdict on a
    // standalone executable, accepting only a notarized Developer ID.
    required(
        "assess executable with Gatekeeper",
        runner.run(
            Path::new(SPCTL),
            &arguments_os([
                OsStr::new("--assess"),
                OsStr::new("--type"),
                OsStr::new("open"),
                OsStr::new("--context"),
                OsStr::new("context:primary-signature"),
                OsStr::new("--verbose=4"),
                request.executable.as_os_str(),
            ]),
        )?,
    )?;

    writeln!(
        output,
        "signed and notarized {}",
        request.executable.display()
    )
    .map_err(ShareDarwinError::Output)?;
    writeln!(output, "identity: {}", request.identity).map_err(ShareDarwinError::Output)?;
    writeln!(output, "notary profile: {}", request.notary_profile)
        .map_err(ShareDarwinError::Output)?;
    writeln!(output, "submission: {}", answer.id).map_err(ShareDarwinError::Output)?;
    writeln!(
        output,
        "verification: strict code signature and online Gatekeeper assessment accepted"
    )
    .map_err(ShareDarwinError::Output)
}

fn admit_executable(path: &Path) -> Result<(), ShareDarwinError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| ShareDarwinError::Input {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.file_type().is_file() {
        return Err(ShareDarwinError::NotRegular(path.to_path_buf()));
    }
    let mut file = File::open(path).map_err(|source| ShareDarwinError::Input {
        path: path.to_path_buf(),
        source,
    })?;
    let mut magic = [0_u8; 4];
    file.read_exact(&mut magic)
        .map_err(|source| ShareDarwinError::Input {
            path: path.to_path_buf(),
            source,
        })?;
    if !matches!(
        magic,
        [0xfe, 0xed, 0xfa, 0xce]
            | [0xce, 0xfa, 0xed, 0xfe]
            | [0xfe, 0xed, 0xfa, 0xcf]
            | [0xcf, 0xfa, 0xed, 0xfe]
            | [0xca, 0xfe, 0xba, 0xbe]
            | [0xbe, 0xba, 0xfe, 0xca]
            | [0xca, 0xfe, 0xba, 0xbf]
            | [0xbf, 0xba, 0xfe, 0xca]
    ) {
        return Err(ShareDarwinError::NotMachO(path.to_path_buf()));
    }
    Ok(())
}

fn admit_identity(identity: &str, runner: &impl Runner) -> Result<(), ShareDarwinError> {
    if !identity.starts_with(DEVELOPER_ID_PREFIX) {
        return Err(ShareDarwinError::MissingIdentity(identity.to_owned()));
    }
    let answer = runner.run(
        Path::new(SECURITY),
        &arguments(["find-identity", "-v", "-p", "codesigning"]),
    )?;
    let quoted = format!("\"{identity}\"");
    let found = answer.success
        && String::from_utf8_lossy(&answer.stdout)
            .lines()
            .any(|line| line.trim_end().ends_with(&quoted));
    if !found {
        return Err(ShareDarwinError::MissingIdentity(identity.to_owned()));
    }
    Ok(())
}

fn admit_notary_profile(profile: &str, runner: &impl Runner) -> Result<(), ShareDarwinError> {
    if profile.is_empty() {
        return Err(ShareDarwinError::MissingNotaryProfile {
            profile: profile.to_owned(),
            detail: "empty profile name".to_owned(),
        });
    }
    let answer = runner.run(
        Path::new(XCRUN),
        &arguments([
            "notarytool",
            "history",
            "--keychain-profile",
            profile,
            "--output-format",
            "json",
        ]),
    )?;
    if !answer.success {
        return Err(ShareDarwinError::MissingNotaryProfile {
            profile: profile.to_owned(),
            detail: answer.detail(),
        });
    }
    Ok(())
}

fn verify_signature(path: &Path, runner: &impl Runner) -> Result<(), ShareDarwinError> {
    required(
        "verify code signature",
        runner.run(
            Path::new(CODESIGN),
            &arguments_os([
                OsStr::new("--verify"),
                OsStr::new("--strict"),
                OsStr::new("--verbose=2"),
                path.as_os_str(),
            ]),
        )?,
    )
}

fn rejected_submission(
    profile: &str,
    answer: NotarySubmission,
    submission: ProcessOutput,
    runner: &impl Runner,
) -> ShareDarwinError {
    let log = runner.run(
        Path::new(XCRUN),
        &arguments([
            "notarytool",
            "log",
            answer.id.as_str(),
            "--keychain-profile",
            profile,
        ]),
    );
    let log = match log {
        Ok(log) => log.detail(),
        Err(error) => format!("notarization log retrieval failed: {error}"),
    };
    ShareDarwinError::NotarizationRejected {
        id: answer.id,
        status: answer.status,
        message: answer.message,
        submission: submission.detail(),
        log,
    }
}

fn required(operation: &'static str, answer: ProcessOutput) -> Result<(), ShareDarwinError> {
    if answer.success {
        Ok(())
    } else {
        Err(ShareDarwinError::CommandFailed {
            operation,
            code: answer.code,
            detail: answer.detail(),
        })
    }
}

fn arguments<const N: usize>(values: [&str; N]) -> Vec<OsString> {
    values.into_iter().map(OsString::from).collect()
}

fn arguments_os<const N: usize>(values: [&OsStr; N]) -> Vec<OsString> {
    values.into_iter().map(OsStr::to_owned).collect()
}

#[derive(Debug, Deserialize)]
struct NotarySubmission {
    id: String,
    status: String,
    #[serde(default)]
    message: String,
}

#[derive(Clone, Debug)]
struct ProcessOutput {
    success: bool,
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl ProcessOutput {
    fn detail(&self) -> String {
        let stdout = String::from_utf8_lossy(&self.stdout);
        let stderr = String::from_utf8_lossy(&self.stderr);
        match (stdout.trim(), stderr.trim()) {
            ("", "") => format!("process exited {:?}", self.code),
            (stdout, "") => stdout.to_owned(),
            ("", stderr) => stderr.to_owned(),
            (stdout, stderr) => format!("{stdout}\n{stderr}"),
        }
    }
}

trait Runner {
    fn run(
        &self,
        program: &Path,
        arguments: &[OsString],
    ) -> Result<ProcessOutput, ShareDarwinError>;
}

struct SystemRunner;

impl Runner for SystemRunner {
    fn run(
        &self,
        program: &Path,
        arguments: &[OsString],
    ) -> Result<ProcessOutput, ShareDarwinError> {
        let answer = Command::new(program)
            .args(arguments)
            .output()
            .map_err(|source| ShareDarwinError::Launch {
                program: program.to_path_buf(),
                source,
            })?;
        Ok(ProcessOutput {
            success: answer.status.success(),
            code: answer.status.code(),
            stdout: answer.stdout,
            stderr: answer.stderr,
        })
    }
}

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn create() -> Result<Self, ShareDarwinError> {
        let base = std::env::temp_dir();
        for _ in 0..100 {
            let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = base.join(format!(
                "spice-darwin-notary-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {}
                Err(source) => {
                    return Err(ShareDarwinError::Temporary { path, source });
                }
            }
        }
        Err(ShareDarwinError::TemporaryExhausted(base))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug)]
pub enum ShareDarwinError {
    UnsupportedHost,
    Input {
        path: PathBuf,
        source: io::Error,
    },
    NotRegular(PathBuf),
    NotMachO(PathBuf),
    MissingIdentity(String),
    MissingNotaryProfile {
        profile: String,
        detail: String,
    },
    Launch {
        program: PathBuf,
        source: io::Error,
    },
    CommandFailed {
        operation: &'static str,
        code: Option<i32>,
        detail: String,
    },
    MalformedNotaryReply {
        source: serde_json::Error,
        detail: String,
    },
    NotarizationRejected {
        id: String,
        status: String,
        message: String,
        submission: String,
        log: String,
    },
    Temporary {
        path: PathBuf,
        source: io::Error,
    },
    TemporaryExhausted(PathBuf),
    Output(io::Error),
}

impl fmt::Display for ShareDarwinError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedHost => formatter.write_str("Darwin sharing requires a macOS host"),
            Self::Input { path, source } => {
                write!(formatter, "cannot inspect {}: {source}", path.display())
            }
            Self::NotRegular(path) => write!(
                formatter,
                "Darwin sharing input is not one regular file: {}",
                path.display()
            ),
            Self::NotMachO(path) => write!(
                formatter,
                "Darwin sharing input is not a Mach-O executable: {}",
                path.display()
            ),
            Self::MissingIdentity(identity) => write!(
                formatter,
                "Developer ID Application identity {identity:?} is unavailable; install its certificate and private key in the login keychain, then verify it with `security find-identity -v -p codesigning`"
            ),
            Self::MissingNotaryProfile { profile, detail } => write!(
                formatter,
                "notarytool keychain profile {profile:?} is unavailable; create it with `xcrun notarytool store-credentials {profile}`: {detail}"
            ),
            Self::Launch { program, source } => {
                write!(
                    formatter,
                    "could not launch {}: {source}",
                    program.display()
                )
            }
            Self::CommandFailed {
                operation,
                code,
                detail,
            } => write!(
                formatter,
                "{operation} failed with exit code {code:?}: {detail}"
            ),
            Self::MalformedNotaryReply { source, detail } => write!(
                formatter,
                "notarytool returned malformed submission JSON: {source}: {detail}"
            ),
            Self::NotarizationRejected {
                id,
                status,
                message,
                submission,
                log,
            } => write!(
                formatter,
                "Apple notarization {id} was {status}: {message}\nsubmission: {submission}\nnotarization log:\n{log}"
            ),
            Self::Temporary { path, source } => write!(
                formatter,
                "cannot create temporary notarization directory {}: {source}",
                path.display()
            ),
            Self::TemporaryExhausted(path) => write!(
                formatter,
                "cannot reserve a temporary notarization directory beneath {}",
                path.display()
            ),
            Self::Output(source) => write!(formatter, "Darwin sharing output failed: {source}"),
        }
    }
}

impl std::error::Error for ShareDarwinError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Input { source, .. }
            | Self::Launch { source, .. }
            | Self::Temporary { source, .. }
            | Self::Output(source) => Some(source),
            Self::MalformedNotaryReply { source, .. } => Some(source),
            Self::UnsupportedHost
            | Self::NotRegular(_)
            | Self::NotMachO(_)
            | Self::MissingIdentity(_)
            | Self::MissingNotaryProfile { .. }
            | Self::CommandFailed { .. }
            | Self::NotarizationRejected { .. }
            | Self::TemporaryExhausted(_) => None,
        }
    }
}

#[cfg(test)]
mod tests;
