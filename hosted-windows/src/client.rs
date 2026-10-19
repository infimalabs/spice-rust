//! Provider client for disposable hosted Windows execution.

use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
const WATCH_POLL: Duration = Duration::from_millis(250);

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
        self.outputs[0].completion.exit_code()
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

    let cancellation = Cancellation::install()?;
    let operation = push_source(&root, &commit, &source_ref).and_then(|()| {
        if cancellation.requested() {
            Err(WindowsHostedError::Cancelled { run_id: None })
        } else {
            dispatch(
                &root,
                scratch.path(),
                &policy.repository,
                &source_ref,
                &commit,
                &request,
                destination.as_deref(),
                &cancellation,
            )
        }
    });
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
    cancellation: &Cancellation,
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

    watch_run(root, repository, run_id, cancellation)?;
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
        .args(["ls-tree", "-r", "-z", "--full-tree", commit])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| WindowsHostedError::Launch {
            program: "git".to_owned(),
            source,
        })?;
    let mut listing = child.stdout.take().ok_or_else(|| {
        WindowsHostedError::ProviderResponse("git tree stdout is unavailable".to_owned())
    })?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = listing
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
    successful("source tree", output)?;
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
    let deletion = command(
        root,
        "git",
        [
            OsString::from("push"),
            OsString::from(REMOTE),
            OsString::from(format!(":refs/tags/{source_ref}")),
        ],
    )?;
    if deletion.status.success() {
        return Ok(());
    }
    let observed = command(
        root,
        "git",
        [
            OsString::from("ls-remote"),
            OsString::from("--tags"),
            OsString::from(REMOTE),
            OsString::from(format!("refs/tags/{source_ref}")),
        ],
    )?;
    if observed.status.success() && observed.stdout.is_empty() {
        return Ok(());
    }
    successful("source retirement", deletion)?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WatchObservation {
    Running,
    Exited { success: bool, code: Option<i32> },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WatchAction {
    Wait,
    Complete,
    Cancel,
    Failed(Option<i32>),
}

fn watch_action(cancel_requested: bool, observation: WatchObservation) -> WatchAction {
    if cancel_requested {
        return WatchAction::Cancel;
    }
    match observation {
        WatchObservation::Running => WatchAction::Wait,
        WatchObservation::Exited { success: true, .. } => WatchAction::Complete,
        WatchObservation::Exited {
            success: false,
            code,
        } => WatchAction::Failed(code),
    }
}

fn watch_run(
    root: &Path,
    repository: &str,
    run_id: u64,
    cancellation: &Cancellation,
) -> Result<(), WindowsHostedError> {
    let mut child = provider_command(root, "gh")
        .args([
            OsString::from("run"),
            OsString::from("watch"),
            OsString::from(run_id.to_string()),
            OsString::from("--repo"),
            OsString::from(repository),
            OsString::from("--compact"),
            OsString::from("--exit-status"),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| WindowsHostedError::Launch {
            program: "gh".to_owned(),
            source,
        })?;
    loop {
        let observation = match child
            .try_wait()
            .map_err(|source| WindowsHostedError::Launch {
                program: "gh".to_owned(),
                source,
            })? {
            None => WatchObservation::Running,
            Some(status) => WatchObservation::Exited {
                success: status.success(),
                code: status.code(),
            },
        };
        // Read after observing the watcher, so a signal racing its exit still
        // cancels the run.
        match watch_action(cancellation.requested(), observation) {
            WatchAction::Wait => thread::sleep(WATCH_POLL),
            WatchAction::Complete => return Ok(()),
            WatchAction::Cancel => {
                stop_watch(&mut child)?;
                cancel_run(root, repository, run_id)?;
                return Err(WindowsHostedError::Cancelled {
                    run_id: Some(run_id),
                });
            }
            WatchAction::Failed(code) => {
                return Err(WindowsHostedError::Command {
                    operation: "workflow execution",
                    code,
                    stdout: String::new(),
                    stderr: String::new(),
                });
            }
        }
    }
}

fn stop_watch(child: &mut Child) -> Result<(), WindowsHostedError> {
    match child.kill() {
        Ok(()) => {}
        Err(source) if source.kind() == io::ErrorKind::InvalidInput => {}
        Err(source) => {
            return Err(WindowsHostedError::Launch {
                program: "gh".to_owned(),
                source,
            });
        }
    }
    child
        .wait()
        .map(|_| ())
        .map_err(|source| WindowsHostedError::Launch {
            program: "gh".to_owned(),
            source,
        })
}

fn cancel_run(root: &Path, repository: &str, run_id: u64) -> Result<(), WindowsHostedError> {
    let cancelled = command(
        root,
        "gh",
        [
            OsString::from("run"),
            OsString::from("cancel"),
            OsString::from(run_id.to_string()),
            OsString::from("--repo"),
            OsString::from(repository),
        ],
    )?;
    if cancelled.status.success() {
        Ok(())
    } else {
        let status = successful_text(
            "workflow state after cancellation",
            command(
                root,
                "gh",
                [
                    OsString::from("run"),
                    OsString::from("view"),
                    OsString::from(run_id.to_string()),
                    OsString::from("--repo"),
                    OsString::from(repository),
                    OsString::from("--json"),
                    OsString::from("status"),
                    OsString::from("--jq"),
                    OsString::from(".status"),
                ],
            )?,
        )?;
        if status.trim() == "completed" {
            Ok(())
        } else {
            successful("workflow cancellation", cancelled).map(|_| ())
        }
    }
}

struct Cancellation {
    #[cfg(unix)]
    requested: Arc<AtomicBool>,
    #[cfg(unix)]
    registrations: Vec<signal_hook::SigId>,
}

impl Cancellation {
    fn install() -> Result<Self, WindowsHostedError> {
        #[cfg(unix)]
        {
            let requested = Arc::new(AtomicBool::new(false));
            let interrupt = signal_hook::flag::register(
                signal_hook::consts::signal::SIGINT,
                Arc::clone(&requested),
            )
            .map_err(WindowsHostedError::Signal)?;
            let terminate = match signal_hook::flag::register(
                signal_hook::consts::signal::SIGTERM,
                Arc::clone(&requested),
            ) {
                Ok(registration) => registration,
                Err(source) => {
                    signal_hook::low_level::unregister(interrupt);
                    return Err(WindowsHostedError::Signal(source));
                }
            };
            Ok(Self {
                requested,
                registrations: vec![interrupt, terminate],
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {})
        }
    }

    fn requested(&self) -> bool {
        #[cfg(unix)]
        {
            self.requested.load(Ordering::Relaxed)
        }
        #[cfg(not(unix))]
        {
            false
        }
    }
}

impl Drop for Cancellation {
    fn drop(&mut self) {
        #[cfg(unix)]
        for registration in self.registrations.drain(..) {
            signal_hook::low_level::unregister(registration);
        }
    }
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
    provider_command(root, program)
        .args(arguments)
        .output()
        .map_err(|source| WindowsHostedError::Launch {
            program: program.to_owned(),
            source,
        })
}

/// Starts a provider subprocess in its own process group, so a terminal
/// interrupt reaches only this client, which cancels at its typed
/// checkpoints. Prompts are disabled because a background group cannot read
/// the terminal.
fn provider_command(root: &Path, program: &str) -> Command {
    let mut command = Command::new(program);
    command
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=5",
        )
        .env("GH_PROMPT_DISABLED", "1");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    command
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
    let mut child = provider_command(root, program)
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
    Signal(io::Error),
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
    Cancelled {
        run_id: Option<u64>,
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
            Self::Signal(source) => write!(
                formatter,
                "cannot install hosted cancellation handler: {source}"
            ),
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
            Self::Cancelled {
                run_id: Some(run_id),
            } => {
                write!(formatter, "hosted provider run {run_id} was cancelled")
            }
            Self::Cancelled { run_id: None } => {
                formatter.write_str("hosted execution was cancelled before provider dispatch")
            }
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
            Self::Io { source, .. }
            | Self::Launch { source, .. }
            | Self::Output(source)
            | Self::Signal(source) => Some(source),
            Self::OperationAndCleanup { operation, .. } => Some(operation),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{WatchAction, WatchObservation, watch_action};

    #[test]
    fn cancellation_preempts_every_provider_watch_observation() {
        for observation in [
            WatchObservation::Running,
            WatchObservation::Exited {
                success: true,
                code: Some(0),
            },
            WatchObservation::Exited {
                success: false,
                code: Some(1),
            },
        ] {
            assert_eq!(watch_action(true, observation), WatchAction::Cancel);
        }
    }

    #[test]
    fn provider_watch_outcomes_remain_typed_without_cancellation() {
        assert_eq!(
            watch_action(false, WatchObservation::Running),
            WatchAction::Wait
        );
        assert_eq!(
            watch_action(
                false,
                WatchObservation::Exited {
                    success: true,
                    code: Some(0),
                }
            ),
            WatchAction::Complete
        );
        assert_eq!(
            watch_action(
                false,
                WatchObservation::Exited {
                    success: false,
                    code: Some(23),
                }
            ),
            WatchAction::Failed(Some(23))
        );
    }
}
