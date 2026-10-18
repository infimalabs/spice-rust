//! Provider client for disposable hosted Windows execution.

use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    Command as HostedCommand, Completion, GITHUB_PROVIDER, ImagePin, Limits, MAX_OUTPUT_BYTES,
    MAX_RUNTIME_SECONDS, Request, ResultDocument, ToolchainPins, WORKFLOW, admit_result,
};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use wire::{Digest, Domain};
const POLICY: &str = ::std::include_str!("../../dist/windows-hosted.toml");
const REMOTE: &str = "origin";
const WORKFLOW_PATH: &str = ".github/workflows/windows-hosted.yml";

/// Complete observed outcome of one provider-authenticated command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostedOutput {
    pub completion: Completion,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Complete observed outcome of one provider-authenticated batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostedExecution {
    pub result: ResultDocument,
    pub outputs: Vec<HostedOutput>,
}

impl HostedExecution {
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self.outputs[0].completion {
            Completion::Exited { code } => code,
            Completion::TimedOut => 124,
        }
    }
}

/// Dispatch one argv with the exact selected worktree to a fresh hosted machine.
pub fn windows_hosted(
    arguments: Vec<OsString>,
    output: &mut impl Write,
    error_output: &mut impl Write,
) -> Result<HostedExecution, WindowsHostedError> {
    let (program, command_arguments) = arguments
        .split_first()
        .ok_or(WindowsHostedError::MissingProgram)?;
    let command = HostedCommand {
        program: unicode("program", program)?,
        arguments: command_arguments
            .iter()
            .map(|argument| unicode("argument", argument))
            .collect::<Result<Vec<_>, _>>()?,
    };
    let execution = windows_hosted_commands(vec![command])?;
    let first = &execution.outputs[0];
    output
        .write_all(&first.stdout)
        .map_err(WindowsHostedError::Output)?;
    error_output
        .write_all(&first.stderr)
        .map_err(WindowsHostedError::Output)?;
    Ok(execution)
}

/// Dispatch a bounded argv batch on one fresh hosted machine and signed run.
pub fn windows_hosted_commands(
    commands: Vec<HostedCommand>,
) -> Result<HostedExecution, WindowsHostedError> {
    windows_hosted_commands_into(commands, None)
}

/// Dispatch a hosted batch and publish its signed artifact files when requested.
pub fn windows_hosted_commands_into(
    commands: Vec<HostedCommand>,
    destination: Option<&Path>,
) -> Result<HostedExecution, WindowsHostedError> {
    let policy: Policy = toml::from_str(POLICY).map_err(WindowsHostedError::Policy)?;
    policy.admit()?;
    let root = workspace_root()?;
    let destination = destination.map(|path| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        }
    });
    let scratch = Scratch::new()?;
    let challenge = challenge()?;
    let source_ref = format!("spice-hosted-{}", &challenge[..24]);
    let commit = snapshot(&root, scratch.path(), &challenge)?;
    let source = source_digest(&root, &commit)?;
    let request = Request {
        version: crate::PROTOCOL_VERSION,
        challenge,
        provider: policy.provider,
        repository: policy.repository.clone(),
        source_ref: source_ref.clone(),
        source,
        image: policy.image,
        toolchain: policy.toolchain,
        limits: Limits {
            runtime_seconds: MAX_RUNTIME_SECONDS,
            output_bytes: MAX_OUTPUT_BYTES,
        },
        commands,
    };
    request.admit().map_err(WindowsHostedError::Admission)?;

    push_source(&root, &commit, &source_ref)?;
    let operation = dispatch(
        &root,
        scratch.path(),
        &policy.repository,
        &source_ref,
        &commit,
        &request,
        destination.as_deref(),
    );
    let cleanup = delete_source(&root, &source_ref);
    match (operation, cleanup) {
        (Ok(execution), Ok(())) => Ok(execution),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(error), Err(cleanup)) => Err(WindowsHostedError::OperationAndCleanup {
            operation: Box::new(error),
            cleanup: Box::new(cleanup),
        }),
    }
}

fn dispatch(
    root: &Path,
    scratch: &Path,
    repository: &str,
    source_ref: &str,
    source_commit: &str,
    request: &Request,
    destination: Option<&Path>,
) -> Result<HostedExecution, WindowsHostedError> {
    let request_json = serde_json::to_string(request).map_err(WindowsHostedError::Encode)?;
    let dispatch_input = serde_json::json!({ "request": request_json });
    let dispatched = command_with_input(
        root,
        "gh",
        [
            OsString::from("workflow"),
            OsString::from("run"),
            OsString::from(WORKFLOW_PATH),
            OsString::from("--repo"),
            OsString::from(repository),
            OsString::from("--ref"),
            OsString::from(source_ref),
            OsString::from("--json"),
        ],
        serde_json::to_vec(&dispatch_input).map_err(WindowsHostedError::Encode)?,
    )?;
    let url = successful_text("workflow dispatch", dispatched)?;
    let run_id = url
        .trim()
        .rsplit('/')
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            WindowsHostedError::ProviderResponse(format!("dispatch returned no run URL: {url:?}"))
        })?;

    successful(
        "workflow execution",
        command(
            root,
            "gh",
            [
                OsString::from("run"),
                OsString::from("watch"),
                OsString::from(run_id.to_string()),
                OsString::from("--repo"),
                OsString::from(repository),
                OsString::from("--compact"),
                OsString::from("--exit-status"),
            ],
        )?,
    )?;
    let (artifact_dir, runner_path, result_path) = download_attested_artifact(
        root,
        scratch,
        repository,
        source_ref,
        source_commit,
        request,
        run_id,
    )?;
    let runner = digest_file(Domain::ProofNativeRunner, &runner_path)?;
    let result_bytes = fs::read(&result_path).map_err(|source| WindowsHostedError::Io {
        path: result_path.clone(),
        source,
    })?;
    let result: ResultDocument =
        serde_json::from_slice(&result_bytes).map_err(WindowsHostedError::Decode)?;
    let identity =
        admit_result(request, &result, &runner).map_err(WindowsHostedError::Admission)?;
    if identity.run_id != run_id {
        return Err(WindowsHostedError::RunIdentity {
            dispatched: run_id,
            signed: identity.run_id,
        });
    }
    let outputs = result
        .outputs
        .iter()
        .map(|output| {
            Ok(HostedOutput {
                completion: output.completion.clone(),
                stdout: output
                    .stdout
                    .bytes()
                    .map_err(WindowsHostedError::Admission)?,
                stderr: output
                    .stderr
                    .bytes()
                    .map_err(WindowsHostedError::Admission)?,
            })
        })
        .collect::<Result<Vec<_>, WindowsHostedError>>()?;
    if let Some(destination) = destination {
        if outputs
            .iter()
            .all(|output| matches!(output.completion, Completion::Exited { code: 0 }))
        {
            publish_artifacts(
                &artifact_dir.join(format!("spice-hosted-stage-{}", request.challenge)),
                &result.artifacts,
                destination,
            )?;
        }
    } else if !result.artifacts.is_empty() {
        return Err(WindowsHostedError::ProviderResponse(
            "hosted execution returned unrequested artifacts".to_owned(),
        ));
    }
    delete_artifact(root, repository, run_id)?;
    Ok(HostedExecution { result, outputs })
}

fn download_attested_artifact(
    root: &Path,
    scratch: &Path,
    repository: &str,
    source_ref: &str,
    source_commit: &str,
    request: &Request,
    run_id: u64,
) -> Result<(PathBuf, PathBuf, PathBuf), WindowsHostedError> {
    let artifact_dir = scratch.join("artifact");
    fs::create_dir(&artifact_dir).map_err(|source| WindowsHostedError::Io {
        path: artifact_dir.clone(),
        source,
    })?;
    successful(
        "artifact download",
        command(
            root,
            "gh",
            [
                OsString::from("run"),
                OsString::from("download"),
                OsString::from(run_id.to_string()),
                OsString::from("--repo"),
                OsString::from(repository),
                OsString::from("--name"),
                OsString::from(format!("spice-hosted-{}", request.challenge)),
                OsString::from("--dir"),
                artifact_dir.as_os_str().to_owned(),
            ],
        )?,
    )?;
    let result_path = artifact_dir.join("result.json");
    let runner_path = artifact_dir.join("bootstrap-target/release/hosted-windows.exe");
    for path in [&runner_path, &result_path] {
        let verification = command(
            root,
            "gh",
            [
                OsString::from("attestation"),
                OsString::from("verify"),
                path.as_os_str().to_owned(),
                OsString::from("--repo"),
                OsString::from(repository),
                OsString::from("--signer-workflow"),
                OsString::from(format!("{repository}/{WORKFLOW}")),
                OsString::from("--signer-digest"),
                OsString::from(source_commit),
                OsString::from("--source-ref"),
                OsString::from(format!("refs/tags/{source_ref}")),
                OsString::from("--source-digest"),
                OsString::from(source_commit),
                OsString::from("--deny-self-hosted-runners"),
                OsString::from("--format=json"),
            ],
        )?;
        successful("artifact attestation", verification)?;
    }
    Ok((artifact_dir, runner_path, result_path))
}

fn publish_artifacts(
    source: &Path,
    artifacts: &[crate::Artifact],
    destination: &Path,
) -> Result<(), WindowsHostedError> {
    for artifact in artifacts {
        let path = source.join(&artifact.path);
        let metadata = fs::metadata(&path).map_err(|source| WindowsHostedError::Io {
            path: path.clone(),
            source,
        })?;
        if !metadata.is_file()
            || metadata.len() != artifact.bytes
            || digest_file(Domain::DistArtifact, &path)? != artifact.digest
        {
            return Err(WindowsHostedError::ProviderResponse(format!(
                "hosted artifact {} differs from its signed inventory",
                artifact.path
            )));
        }
    }
    let mut published = artifacts.iter().collect::<Vec<_>>();
    published.sort_by_key(|artifact| artifact.path.ends_with("/payload.json"));
    for artifact in published {
        let source = source.join(&artifact.path);
        let path = destination.join(&artifact.path);
        let parent = path.parent().expect("admitted artifact path has a parent");
        fs::create_dir_all(parent).map_err(|source| WindowsHostedError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let staged = path.with_extension(format!("hosted-{}", std::process::id()));
        fs::copy(&source, &staged).map_err(|source| WindowsHostedError::Io {
            path: staged.clone(),
            source,
        })?;
        fs::rename(&staged, &path).map_err(|source| WindowsHostedError::Io {
            path: path.clone(),
            source,
        })?;
    }
    Ok(())
}

fn delete_artifact(root: &Path, repository: &str, run_id: u64) -> Result<(), WindowsHostedError> {
    let listed = command(
        root,
        "gh",
        [
            OsString::from("api"),
            OsString::from(format!(
                "repos/{repository}/actions/runs/{run_id}/artifacts"
            )),
        ],
    )?;
    let listed = successful_text("artifact inventory", listed)?;
    let inventory: ArtifactInventory =
        serde_json::from_str(&listed).map_err(WindowsHostedError::Decode)?;
    if inventory.artifacts.len() != 1 {
        return Err(WindowsHostedError::ProviderResponse(format!(
            "run {run_id} published {} artifacts instead of one",
            inventory.artifacts.len()
        )));
    }
    successful(
        "artifact deletion",
        command(
            root,
            "gh",
            [
                OsString::from("api"),
                OsString::from("--method"),
                OsString::from("DELETE"),
                OsString::from(format!(
                    "repos/{repository}/actions/artifacts/{}",
                    inventory.artifacts[0].id
                )),
            ],
        )?,
    )?;
    Ok(())
}

fn snapshot(root: &Path, scratch: &Path, challenge: &str) -> Result<String, WindowsHostedError> {
    let index = scratch.join("index");
    let git = |arguments: Vec<OsString>| -> Result<Output, WindowsHostedError> {
        Command::new("git")
            .current_dir(root)
            .env("GIT_INDEX_FILE", &index)
            .args(arguments)
            .output()
            .map_err(|source| WindowsHostedError::Launch {
                program: "git".to_owned(),
                source,
            })
    };
    successful(
        "snapshot base",
        git(vec!["read-tree".into(), "HEAD".into()])?,
    )?;
    successful(
        "snapshot worktree",
        git(vec!["add".into(), "-A".into(), "--".into(), ".".into()])?,
    )?;
    let tree = successful_text("snapshot tree", git(vec!["write-tree".into()])?)?;
    let parent = successful_text(
        "snapshot parent",
        command(root, "git", ["rev-parse", "HEAD"].map(OsString::from))?,
    )?;
    let message = format!("spice hosted source {challenge}\n");
    let mut process = Command::new("git");
    process
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "Spice Hosted Runner")
        .env("GIT_AUTHOR_EMAIL", "hosted-runner@invalid")
        .env("GIT_COMMITTER_NAME", "Spice Hosted Runner")
        .env("GIT_COMMITTER_EMAIL", "hosted-runner@invalid")
        .args(["commit-tree", tree.trim(), "-p", parent.trim()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = process
        .spawn()
        .map_err(|source| WindowsHostedError::Launch {
            program: "git".to_owned(),
            source,
        })?;
    child
        .stdin
        .take()
        .ok_or_else(|| {
            WindowsHostedError::ProviderResponse("git commit-tree stdin is unavailable".to_owned())
        })?
        .write_all(message.as_bytes())
        .map_err(WindowsHostedError::Output)?;
    successful_text(
        "snapshot commit",
        child
            .wait_with_output()
            .map_err(|source| WindowsHostedError::Launch {
                program: "git".to_owned(),
                source,
            })?,
    )
    .map(|value| value.trim().to_owned())
}

fn source_digest(root: &Path, commit: &str) -> Result<String, WindowsHostedError> {
    let mut child = Command::new("git")
        .current_dir(root)
        .args(["archive", "--format=tar", commit])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| WindowsHostedError::Launch {
            program: "git".to_owned(),
            source,
        })?;
    let mut archive = child.stdout.take().ok_or_else(|| {
        WindowsHostedError::ProviderResponse("git archive stdout is unavailable".to_owned())
    })?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = archive
            .read(&mut buffer)
            .map_err(WindowsHostedError::Output)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let output = child
        .wait_with_output()
        .map_err(|source| WindowsHostedError::Launch {
            program: "git".to_owned(),
            source,
        })?;
    successful("source archive", output)?;
    Ok(format!("{:x}", digest.finalize()))
}

fn digest_file(domain: Domain, path: &Path) -> Result<Digest, WindowsHostedError> {
    let file = fs::File::open(path).map_err(|source| WindowsHostedError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Digest::compute_reader(domain, file, 512 * 1024 * 1024)
        .map(|(_, digest)| digest)
        .map_err(|source| WindowsHostedError::Io {
            path: path.to_path_buf(),
            source,
        })
}

fn push_source(root: &Path, commit: &str, source_ref: &str) -> Result<(), WindowsHostedError> {
    successful(
        "source transfer",
        command(
            root,
            "git",
            [
                OsString::from("push"),
                OsString::from(REMOTE),
                OsString::from(format!("{commit}:refs/tags/{source_ref}")),
            ],
        )?,
    )?;
    Ok(())
}

fn delete_source(root: &Path, source_ref: &str) -> Result<(), WindowsHostedError> {
    successful(
        "source retirement",
        command(
            root,
            "git",
            [
                OsString::from("push"),
                OsString::from(REMOTE),
                OsString::from(format!(":refs/tags/{source_ref}")),
            ],
        )?,
    )?;
    Ok(())
}

fn workspace_root() -> Result<PathBuf, WindowsHostedError> {
    let current = std::env::current_dir().map_err(|source| WindowsHostedError::Io {
        path: PathBuf::from("."),
        source,
    })?;
    let root = successful_text(
        "workspace discovery",
        command(
            &current,
            "git",
            ["rev-parse", "--show-toplevel"].map(OsString::from),
        )?,
    )?;
    Ok(PathBuf::from(root.trim()))
}

fn challenge() -> Result<String, WindowsHostedError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| WindowsHostedError::Random(error.to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn unicode(kind: &'static str, value: &OsStr) -> Result<String, WindowsHostedError> {
    value
        .to_str()
        .map(str::to_owned)
        .ok_or(WindowsHostedError::NonUnicode(kind))
}

fn command<I>(root: &Path, program: &str, arguments: I) -> Result<Output, WindowsHostedError>
where
    I: IntoIterator<Item = OsString>,
{
    Command::new(program)
        .current_dir(root)
        .args(arguments)
        .output()
        .map_err(|source| WindowsHostedError::Launch {
            program: program.to_owned(),
            source,
        })
}

fn command_with_input<I>(
    root: &Path,
    program: &str,
    arguments: I,
    input: Vec<u8>,
) -> Result<Output, WindowsHostedError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut child = Command::new(program)
        .current_dir(root)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| WindowsHostedError::Launch {
            program: program.to_owned(),
            source,
        })?;
    child
        .stdin
        .take()
        .ok_or_else(|| {
            WindowsHostedError::ProviderResponse(format!("{program} stdin is unavailable"))
        })?
        .write_all(&input)
        .map_err(WindowsHostedError::Output)?;
    child
        .wait_with_output()
        .map_err(|source| WindowsHostedError::Launch {
            program: program.to_owned(),
            source,
        })
}

fn successful(operation: &'static str, output: Output) -> Result<Output, WindowsHostedError> {
    if output.status.success() {
        Ok(output)
    } else {
        Err(WindowsHostedError::Command {
            operation,
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

fn successful_text(operation: &'static str, output: Output) -> Result<String, WindowsHostedError> {
    let output = successful(operation, output)?;
    String::from_utf8(output.stdout).map_err(|_| {
        WindowsHostedError::ProviderResponse(format!("{operation} returned non-UTF-8 output"))
    })
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, WindowsHostedError> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| WindowsHostedError::Clock(error.to_string()))?
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "spice-windows-hosted-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).map_err(|source| WindowsHostedError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _removed = fs::remove_dir_all(&self.0);
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    schema: u16,
    provider: String,
    repository: String,
    workflow: String,
    image: ImagePin,
    toolchain: ToolchainPins,
}

impl Policy {
    fn admit(&self) -> Result<(), WindowsHostedError> {
        if self.schema != 1
            || self.provider != GITHUB_PROVIDER
            || self.workflow != WORKFLOW
            || self.workflow != WORKFLOW_PATH
        {
            return Err(WindowsHostedError::PolicyIdentity);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct ArtifactInventory {
    artifacts: Vec<Artifact>,
}

#[derive(Deserialize)]
struct Artifact {
    id: u64,
}

/// Why no provider-authenticated execution result could be returned.
#[derive(Debug)]
pub enum WindowsHostedError {
    MissingProgram,
    NonUnicode(&'static str),
    Policy(toml::de::Error),
    PolicyIdentity,
    Admission(crate::AdmissionError),
    Encode(serde_json::Error),
    Decode(serde_json::Error),
    Random(String),
    Clock(String),
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Launch {
        program: String,
        source: io::Error,
    },
    Command {
        operation: &'static str,
        code: Option<i32>,
        stdout: String,
        stderr: String,
    },
    ProviderResponse(String),
    RunIdentity {
        dispatched: u64,
        signed: u64,
    },
    Output(io::Error),
    OperationAndCleanup {
        operation: Box<WindowsHostedError>,
        cleanup: Box<WindowsHostedError>,
    },
}

impl fmt::Display for WindowsHostedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingProgram => {
                formatter.write_str("windows-hosted requires PROGRAM [ARG...] after --")
            }
            Self::NonUnicode(kind) => write!(formatter, "hosted {kind} is not Unicode"),
            Self::Policy(source) => {
                write!(formatter, "hosted Windows policy does not decode: {source}")
            }
            Self::PolicyIdentity => formatter
                .write_str("hosted Windows policy names an unsupported provider or workflow"),
            Self::Admission(source) => source.fmt(formatter),
            Self::Encode(source) => write!(formatter, "hosted request does not encode: {source}"),
            Self::Decode(source) => write!(formatter, "hosted result does not decode: {source}"),
            Self::Random(detail) => {
                write!(formatter, "fresh hosted challenge is unavailable: {detail}")
            }
            Self::Clock(detail) => write!(
                formatter,
                "hosted scratch identity is unavailable: {detail}"
            ),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Launch { program, source } => {
                write!(formatter, "cannot launch {program}: {source}")
            }
            Self::Command {
                operation,
                code,
                stdout,
                stderr,
            } => write!(
                formatter,
                "{operation} failed with exit {code:?}: stdout={stdout:?} stderr={stderr:?}"
            ),
            Self::ProviderResponse(detail) => formatter.write_str(detail),
            Self::RunIdentity { dispatched, signed } => write!(
                formatter,
                "signed provider run {signed} does not match dispatched run {dispatched}"
            ),
            Self::Output(source) => write!(formatter, "hosted output failed: {source}"),
            Self::OperationAndCleanup { operation, cleanup } => write!(
                formatter,
                "hosted operation failed: {operation}; source retirement also failed: {cleanup}"
            ),
        }
    }
}

impl Error for WindowsHostedError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Policy(source) => Some(source),
            Self::Admission(source) => Some(source),
            Self::Encode(source) | Self::Decode(source) => Some(source),
            Self::Io { source, .. } | Self::Launch { source, .. } | Self::Output(source) => {
                Some(source)
            }
            Self::OperationAndCleanup { operation, .. } => Some(operation),
            _ => None,
        }
    }
}
