use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

#[cfg(not(target_arch = "wasm32"))]
use std::{
    fs,
    io::{self, Write},
    process::ExitStatus,
    sync::mpsc,
    time::Duration,
};

use listen::{RetroSelector, StreamOutcome, SurfaceError, Watch};
use spice::cli;
use spice::placeholder::PlaceholderReason;
use wire::{Fault, FaultClass, Outcome, Refusal};

#[cfg(not(target_arch = "wasm32"))]
mod board_command;
#[cfg(not(target_arch = "wasm32"))]
#[cfg(not(target_arch = "wasm32"))]
mod command_client;
#[cfg(not(target_arch = "wasm32"))]
mod config_command;
#[cfg(not(target_arch = "wasm32"))]
mod consent_command;
#[cfg(not(target_arch = "wasm32"))]
mod demo_command;
#[cfg(not(target_arch = "wasm32"))]
mod doctor;
#[cfg(not(target_arch = "wasm32"))]
mod gate_command;
#[cfg(not(target_arch = "wasm32"))]
mod install_command;
#[cfg(not(target_arch = "wasm32"))]
mod journal_command;
#[cfg(not(target_arch = "wasm32"))]
mod journal_lock;
#[cfg(not(target_arch = "wasm32"))]
mod lane_command;
#[cfg(not(target_arch = "wasm32"))]
mod lock_command;
mod proof_command;
#[cfg(not(target_arch = "wasm32"))]
mod release_command;
#[cfg(not(target_arch = "wasm32"))]
mod release_history;
mod render;
#[cfg(not(target_arch = "wasm32"))]
mod resident;
#[cfg(not(target_arch = "wasm32"))]
mod restore_command;
#[cfg(not(target_arch = "wasm32"))]
mod session_command;
#[cfg(not(target_arch = "wasm32"))]
mod steering_command;
#[cfg(not(target_arch = "wasm32"))]
mod task_artifact_command;
#[cfg(not(target_arch = "wasm32"))]
mod task_migration_command;
#[cfg(not(target_arch = "wasm32"))]
mod taste_command;
#[cfg(not(target_arch = "wasm32"))]
mod trust_command;

use render::Form;

const REPLY_EXIT: i32 = 0;
const FAULT_EXIT: i32 = 1;
const REFUSAL_EXIT: i32 = 2;
#[cfg(not(target_arch = "wasm32"))]
const WATCH_SCAN_INTERVAL: Duration = Duration::from_millis(100);

enum StateFailure {
    Refusal(Refusal),
    Fault(Fault),
}

impl StateFailure {
    #[cfg(not(target_arch = "wasm32"))]
    fn outcome<B>(self) -> Outcome<B> {
        match self {
            Self::Refusal(refusal) => Outcome::Refusal(refusal),
            Self::Fault(fault) => Outcome::Fault(fault),
        }
    }
}

fn state_failure(error: &spice::repository::StateError) -> StateFailure {
    match error {
        spice::repository::StateError::Filesystem { .. } => StateFailure::Fault(Fault {
            class: FaultClass::Environment,
            reason: "repository::state-path-unavailable".to_owned(),
            detail: error.to_string(),
        }),
        spice::repository::StateError::Linked { .. }
        | spice::repository::StateError::NotDirectory { .. }
        | spice::repository::StateError::NotRegularFile { .. }
        | spice::repository::StateError::RetiredState { .. } => StateFailure::Refusal(Refusal {
            reason: "repository::state-path-refused".to_owned(),
            witness: Some("repository-state-root".to_owned()),
            expected: Some(
                "real fixed components beneath the canonical Git-common .spice-rs directory"
                    .to_owned(),
            ),
            observed: Some(error.to_string()),
            remedy: format!(
                concat!(
                    "follow the named archive/remove action or repair the malformed state ",
                    "component at {}"
                ),
                error.path().display()
            ),
        }),
    }
}

fn main() {
    let matches = cli::command().get_matches();
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(command) = cli::run_command(&matches) {
            run(command);
        }
    }

    let contract = cli::selected_contract(&matches)
        .expect("the generated parser returns only registered command leaves");
    let form = if cli::json_selected(&matches) {
        Form::Json
    } else {
        Form::Text
    };
    let leaf = cli::selected_matches(&matches);
    match contract.path {
        ["watch"] => run_watch(leaf, form),
        ["retro"] => run_retro(leaf, form),
        ["proof"] => deliver(&proof_command::seal(), form),
        ["proof", "replay"] => deliver(&proof_command::replay(), form),
        _ => {}
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        dispatch_native_queries(contract, leaf, form);
        dispatch_native_controls(contract, leaf, form);
    }
    #[cfg(target_arch = "wasm32")]
    deliver_target_refusal(contract.path, form);
    #[cfg(not(target_arch = "wasm32"))]
    let outcome = match contract.path {
        ["up"] => resident_up(leaf, form),
        ["down"] => resident::down(contract),
        _ => not_landed(contract),
    };
    #[cfg(target_arch = "wasm32")]
    let outcome = not_landed(contract);
    deliver(&outcome, form);
}

#[cfg(not(target_arch = "wasm32"))]
fn resident_up(leaf: &clap::ArgMatches, form: Form) -> Outcome<()> {
    // A foreground resident has useful startup state before it has a terminal
    // outcome. Human mode keeps diagnostics on stderr and publishes the bound
    // endpoint on stdout, where a shell can read it while the resident remains
    // alive. JSON mode keeps both streams reserved for its one eventual machine
    // document.
    match form {
        Form::Text => resident::up(leaf, &mut io::stderr().lock(), &mut io::stdout().lock()),
        Form::Json => resident::up(leaf, &mut io::sink(), &mut io::sink()),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn dispatch_native_queries(contract: &cli::Contract, leaf: &clap::ArgMatches, form: Form) {
    dispatch_native_gate_queries(contract, leaf, form);
    dispatch_native_policy_queries(contract, leaf, form);
    dispatch_native_repository_queries(contract, leaf, form);
}

#[cfg(not(target_arch = "wasm32"))]
fn dispatch_native_gate_queries(contract: &cli::Contract, leaf: &clap::ArgMatches, form: Form) {
    match contract.path {
        ["doctor"] => deliver(&doctor_outcome(), form),
        ["demo"] => deliver(&demo_command::run(leaf), form),
        ["gate"] => deliver(&gate_command::run(leaf), form),
        ["gate", "camp"] => deliver(&gate_command::camp(), form),
        ["gate", "kill"] => deliver(&gate_command::kill(), form),
        ["gate", "pre-commit"] => deliver(&gate_command::pre_commit(), form),
        ["gate", "receipt"] => deliver(&gate_command::receipt(), form),
        ["install"] => match install_command::run(leaf) {
            install_command::Answer::Plan(outcome) => deliver(&outcome, form),
            install_command::Answer::Receipt(outcome) => deliver(&outcome, form),
        },
        ["config"] => deliver_config(&config_command::run(leaf), form),
        ["config", "migrate-tasks"] => {
            deliver_task_migration(&task_migration_command::run(leaf), form)
        }
        _ => {}
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn dispatch_native_policy_queries(contract: &cli::Contract, leaf: &clap::ArgMatches, form: Form) {
    match contract.path {
        ["consent"] => deliver(&consent_command::read(contract), form),
        ["consent", "allow"] => deliver(
            &consent_command::decide(leaf, contract, wire::Answer::Granted),
            form,
        ),
        ["consent", "deny"] => deliver(
            &consent_command::decide(leaf, contract, wire::Answer::Denied),
            form,
        ),
        ["consent", "policy"] => deliver(&consent_command::policy(contract), form),
        ["lock"] => deliver(&lock_command::report(), form),
        ["lock", "run"] => match lock_command::run(leaf) {
            lock_command::RunResult::Child(status) => terminate_child(status),
            lock_command::RunResult::Rejected { outcome, code } => {
                deliver_with_code(&outcome, form, code)
            }
        },
        ["task", "artifact", "add"]
        | ["task", "artifact", "list"]
        | ["task", "artifact", "show"]
        | ["task", "artifact", "prune"] => {
            deliver(&task_artifact_command::run(leaf, contract), form)
        }
        ["backup"] => deliver(&journal_command::backup(leaf), form),
        ["restore"] => deliver(&restore_command::restore(leaf), form),
        _ => {}
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn dispatch_native_repository_queries(
    contract: &cli::Contract,
    leaf: &clap::ArgMatches,
    form: Form,
) {
    match contract.path {
        ["journal"] => deliver(&journal_command::report(contract), form),
        ["journal", "replay"] => deliver(&journal_command::replay(leaf, contract), form),
        ["journal", "reindex"] => deliver(&journal_command::reindex(), form),
        ["journal", "verify"] => deliver(&journal_command::verify(contract), form),
        ["fold"] => deliver(&journal_command::folds(contract), form),
        ["task", "effort"] => deliver(&journal_command::task_effort(leaf, contract), form),
        ["thread"] => deliver(&journal_command::thread(leaf, contract), form),
        ["version"] => deliver(&version_outcome(), form),
        ["release", "notes"] => deliver_release_notes(&release_command::notes(leaf), form),
        _ => {}
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn dispatch_native_controls(contract: &cli::Contract, leaf: &clap::ArgMatches, form: Form) {
    match contract.path {
        ["restore", "verify"] => deliver(&restore_command::verify(leaf), form),
        ["session"]
        | ["session", "briefing"]
        | ["session", "commands"]
        | ["session", "commits"]
        | ["session", "compactions"]
        | ["session", "messages"]
        | ["session", "phases"]
        | ["session", "slices"]
        | ["session", "summary"]
        | ["session", "sweep"]
        | ["session", "thread"]
        | ["session", "timeline"]
        | ["session", "tokens"]
        | ["session", "turns"] => deliver(
            &session_command::run(leaf, contract.path, &session_environment()),
            form,
        ),
        ["lane"]
        | ["lane", "add"]
        | ["lane", "drain"]
        | ["lane", "wake"]
        | ["lane", "import"]
        | ["recall"] => deliver(&lane_command::run(leaf, contract), form),
        ["release", "publish"] => match release_command::run(leaf, contract) {
            release_command::RunResult::Plan(outcome) => deliver(&outcome, form),
            release_command::RunResult::Published(outcome) => deliver(&outcome, form),
        },
        ["board"]
        | ["board", "graph"]
        | ["board", "handout"]
        | ["task", "list"]
        | ["task", "status"]
        | ["task", "next"]
        | ["task", "reclaim"]
        | ["task", "add"]
        | ["task", "capture"]
        | ["task", "done"]
        | ["task", "review"]
        | ["task", "note"]
        | ["task", "defer"]
        | ["task", "wake"]
        | ["task", "free"]
        | ["task", "oops"]
        | ["task", "depends"]
        | ["task", "modify"]
        | ["task", "delete"]
        | ["doc"]
        | ["doc", "ledger"] => deliver(&board_command::run(leaf, contract), form),
        ["steer"] | ["steer", "cancel"] | ["steer", "recover"] | ["ack"] | ["inbox"] => deliver(
            &steering_command::run(leaf, contract, bootstrap_environment(steer::hook::LANE)),
            form,
        ),
        ["hook"] => {
            let outcome =
                steering_command::run(leaf, contract, bootstrap_environment(steer::hook::LANE));
            let harness = leaf
                .get_one::<String>("HARNESS")
                .expect("clap requires a hook harness");
            deliver_inbox_hook(&outcome, harness, form)
        }
        ["trust"] => deliver(&trust_command::report(contract), form),
        ["trust", "grant"] => deliver(&trust_command::rule(leaf, contract, true), form),
        ["trust", "revoke"] => deliver(&trust_command::rule(leaf, contract, false), form),
        ["taste"] => deliver(&taste_command::report(contract), form),
        ["taste", "add"] => deliver(&taste_command::add(leaf, contract), form),
        ["taste", "drop"] => deliver(&taste_command::drop(leaf, contract), form),
        _ => {}
    }
}

#[cfg(target_arch = "wasm32")]
fn deliver_target_refusal(path: &[&str], form: Form) {
    let Some(fault) = target_unsupported(path) else {
        return;
    };
    deliver_fault(fault, form);
}

#[cfg(any(test, target_arch = "wasm32"))]
fn target_unsupported(path: &[&str]) -> Option<Fault> {
    let (reason, detail) = match path {
        ["demo"] => (
            "demo::target-unsupported",
            "filesystem seeding and platform browser launch are unavailable on wasm32",
        ),
        ["gate", ..] => (
            "gate::target-unsupported",
            "immutable Git tree selection is unavailable on wasm32",
        ),
        ["release", "notes"] => (
            "release::target-unsupported",
            "immutable Git history inspection for release notes is unavailable on wasm32",
        ),
        ["install"] => (
            "install::target-unsupported",
            "filesystem installation is unavailable on wasm32",
        ),
        ["lock"] | ["lock", "run"] => (
            "lock::target-unsupported",
            "filesystem-backed process locks are unavailable on wasm32",
        ),
        ["restore", "verify"] => (
            "restore::target-unsupported",
            "reading and verifying a restore archive is unavailable on wasm32",
        ),
        ["config", "migrate-tasks"] => (
            "task-migration::target-unsupported",
            "reading a predecessor task export is unavailable on wasm32",
        ),
        ["session", ..] => (
            "session::target-unsupported",
            "filesystem transcript forensics are unavailable on wasm32",
        ),
        _ => return None,
    };
    Some(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

fn not_landed(contract: &cli::Contract) -> Outcome<()> {
    not_landed_from(contract.path, cli::landing(contract.path))
}

fn not_landed_from(path: &[&str], landing: cli::Landing) -> Outcome<()> {
    match landing {
        cli::Landing::Landed => Outcome::Fault(Fault {
            class: FaultClass::Protocol,
            reason: PlaceholderReason::DispatchGap.to_reason(),
            detail: format!(
                concat!(
                    "registered landed command reached the terminal ",
                    "release branch: {}"
                ),
                path.join(" ")
            ),
        }),
        cli::Landing::GapOwned(owner) => Outcome::Refusal(Refusal {
            reason: PlaceholderReason::NotLanded.to_reason(),
            witness: Some("landing-owner".to_owned()),
            expected: Some(owner.to_owned()),
            observed: Some(path.join(" ")),
            remedy: format!("complete the owning task {owner}"),
        }),
        cli::Landing::Unaccounted => Outcome::Fault(Fault {
            class: FaultClass::Protocol,
            reason: PlaceholderReason::LandingUnaccounted.to_reason(),
            detail: format!(
                "registered command has no landing disposition: {}",
                path.join(" ")
            ),
        }),
    }
}

/// Acquire Doctor's ambient inputs at the binary rim, retaining presence but
/// never the values of Spice-owned variables.
#[cfg(not(target_arch = "wasm32"))]
fn doctor_outcome() -> Outcome<wire::DoctorReport<spice::config::Root>> {
    let working = match std::env::current_dir() {
        Ok(path) => path,
        Err(error) => {
            return Outcome::Fault(Fault {
                class: FaultClass::Environment,
                reason: "doctor::working-directory-unavailable".to_owned(),
                detail: error.to_string(),
            });
        }
    };
    let environment: Vec<_> = wire::SpiceVariable::ALL
        .iter()
        .map(|variable| {
            (
                *variable,
                bootstrap_environment(variable.as_str()).is_some(),
            )
        })
        .collect();
    let search_path = bootstrap_environment("PATH");

    doctor::inspect(&working, &environment, search_path.as_deref())
}

fn run_retro(matches: &clap::ArgMatches, form: Form) -> ! {
    let selector = match matches.get_one::<String>("LANE|LOCUS|PATH") {
        Some(value) => match RetroSelector::parse(value) {
            Ok(selector) => selector,
            Err(error) => {
                let refusal: Outcome<()> = Outcome::Refusal(Refusal {
                    reason: "retro::selector".to_owned(),
                    witness: None,
                    expected: Some("LANE, M:O:L, or PATH".to_owned()),
                    observed: Some(value.clone()),
                    remedy: error.to_string(),
                });
                deliver(&refusal, form);
            }
        },
        None => RetroSelector::Path(PathBuf::from(".")),
    };
    let dialects = extension_dialects();
    match listen::retro(".", selector, &dialects) {
        Ok(outcome) => deliver_stream_once(&outcome, form),
        Err(error) => deliver_fault(surface_fault(error), form),
    }
}

/// Builds the daemonless identity report from the executable and repository
/// this process is actually running against.
#[cfg(not(target_arch = "wasm32"))]
fn version_outcome() -> Outcome<wire::Version> {
    let start = match std::env::current_dir() {
        Ok(path) => path,
        Err(error) => {
            return Outcome::Fault(Fault {
                class: FaultClass::Environment,
                reason: "version::working-directory-unavailable".to_owned(),
                detail: error.to_string(),
            });
        }
    };
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            return Outcome::Fault(Fault {
                class: FaultClass::Environment,
                reason: "version::binary-unresolved".to_owned(),
                detail: error.to_string(),
            });
        }
    };
    let binary = match fs::read(&executable) {
        Ok(bytes) => wire::Digest::compute(wire::Domain::DistArtifact, &bytes),
        Err(error) => {
            return Outcome::Fault(Fault {
                class: FaultClass::Environment,
                reason: "version::binary-unreadable".to_owned(),
                detail: format!("{}: {error}", executable.display()),
            });
        }
    };
    let repository = match spice::repository::Repository::discover(&start) {
        Ok(repository) => repository,
        Err(error) => {
            return Outcome::Fault(Fault {
                class: FaultClass::Environment,
                reason: "version::repository-unresolved".to_owned(),
                detail: error.to_string(),
            });
        }
    };
    let state = match repository.admit_state() {
        Ok(state) => state,
        Err(error) => return state_failure(&error).outcome(),
    };
    let app = match canvas::AppSource::resolve(repository.worktree_root())
        .and_then(|source| source.digest())
    {
        Ok(digest) => digest,
        Err(error) => {
            return Outcome::Fault(Fault {
                class: FaultClass::Environment,
                reason: "version::app-unavailable".to_owned(),
                detail: error.to_string(),
            });
        }
    };
    let reader = match journal::JournalReader::open(state.journal()) {
        Ok(reader) => reader,
        Err(error) => return journal_version_failure(error),
    };
    let journal = match reader.schema_version() {
        Ok(version) => version,
        Err(error) => return journal_version_failure(error),
    };

    Outcome::Reply(wire::Reply {
        position: wire::ReplyPosition::Direct {},
        body: wire::Version {
            binary,
            wire: wire::WireVersion::CURRENT,
            journal,
            app,
        },
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn journal_version_failure(error: journal::OpenError) -> Outcome<wire::Version> {
    match error {
        journal::OpenError::Migration(migration) => Outcome::Refusal(Refusal {
            reason: "journal::schema-refused".to_owned(),
            witness: None,
            expected: Some(format!(
                "finalized journal schema {}",
                journal::SCHEMA_VERSION
            )),
            observed: Some(migration.to_string()),
            remedy: concat!(
                "finish or replace the unfinalized or incompatible authority ",
                "before asking for version"
            )
            .to_owned(),
        }),
        error => Outcome::Fault(Fault {
            class: FaultClass::Store,
            reason: "version::journal-unreadable".to_owned(),
            detail: error.to_string(),
        }),
    }
}

/// Emits any pending direction and hands the caller's command through.
#[cfg(not(target_arch = "wasm32"))]
fn run(command: Vec<OsString>) -> ! {
    let (program, arguments) = command
        .split_first()
        .expect("clap requires the run command operand");

    if let Some(rendering) = bootstrap_environment(steer::hook::INBOX) {
        let mut stderr = io::stderr().lock();
        let _ = steer::run::emit(Path::new(&rendering), &mut stderr);
    }

    match steer::run::hand(program, arguments) {
        Ok(status) => terminate_child(status),
        Err(error) => {
            let mut stderr = io::stderr().lock();
            let _ = writeln!(stderr, "spice run: {error}");
            let code = match error.kind() {
                io::ErrorKind::NotFound => 127,
                io::ErrorKind::PermissionDenied => 126,
                _ => FAULT_EXIT,
            };
            terminate_code(code);
        }
    }
}

fn run_watch(matches: &clap::ArgMatches, form: Form) -> ! {
    let root = matches
        .get_one::<String>("ROOT")
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    let mut watch = Watch::new(&root, extension_dialects());

    if !is_real_directory(&root) {
        match poll_and_emit(&mut watch, form) {
            Ok(()) => terminate_stream(),
            Err(fault) => deliver_fault(fault, form),
        }
    }

    follow(&root, &mut watch, form)
}

fn extension_dialects() -> Vec<(&'static str, listen::Dialect)> {
    spice::extension::registry()
        .expect("compiled extension declarations are validated by the test gate")
        .vendors()
        .into_iter()
        .map(|vendor| (vendor.name(), vendor.dialect()))
        .collect()
}

fn is_real_directory(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir())
}

#[cfg(not(target_arch = "wasm32"))]
fn follow(root: &Path, watch: &mut Watch, form: Form) -> ! {
    use notify::{RecursiveMode, Watcher};

    let (sender, receiver) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = match notify::PollWatcher::new(
        move |event| {
            let _ = sender.send(event);
        },
        notify::Config::default().with_poll_interval(WATCH_SCAN_INTERVAL),
    ) {
        Ok(watcher) => watcher,
        Err(error) => deliver_fault(watch_fault("create filesystem watcher", error), form),
    };
    if let Err(error) = watcher.watch(root, RecursiveMode::Recursive) {
        deliver_fault(watch_fault("arm filesystem watcher", error), form);
    }
    if let Err(fault) = poll_and_emit(watch, form) {
        deliver_fault(fault, form);
    }

    loop {
        match receiver.recv() {
            Ok(Ok(_)) => {
                if let Err(fault) = poll_and_emit(watch, form) {
                    deliver_fault(fault, form);
                }
            }
            Ok(Err(error)) => {
                deliver_fault(watch_fault("receive filesystem notification", error), form);
            }
            Err(error) => {
                deliver_fault(
                    Fault {
                        class: FaultClass::Environment,
                        reason: "watch::notification".to_owned(),
                        detail: format!("filesystem notification channel disconnected: {error}"),
                    },
                    form,
                );
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn follow(_root: &Path, _watch: &mut Watch, form: Form) -> ! {
    deliver_fault(
        Fault {
            class: FaultClass::Environment,
            reason: "watch::unavailable".to_owned(),
            detail: "filesystem notifications are unavailable on wasm32".to_owned(),
        },
        form,
    )
}

fn poll_and_emit(watch: &mut Watch, form: Form) -> Result<(), Fault> {
    let outcome = watch.poll().map_err(surface_fault)?;
    if !outcome.is_empty() {
        render::render_stream(&outcome, form)
            .map_err(render_fault)?
            .speak();
    }
    Ok(())
}

fn deliver_stream_once(outcome: &StreamOutcome, form: Form) -> ! {
    match render::render_stream(outcome, form) {
        Ok(rendering) => {
            rendering.speak();
            terminate_stream();
        }
        Err(error) => deliver_fault(render_fault(error), form),
    }
}

fn surface_fault(error: SurfaceError) -> Fault {
    Fault {
        class: FaultClass::Environment,
        reason: "listen::surface".to_owned(),
        detail: error.to_string(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn watch_fault(operation: &str, error: notify::Error) -> Fault {
    Fault {
        class: FaultClass::Environment,
        reason: "watch::notification".to_owned(),
        detail: format!("failed to {operation}: {error}"),
    }
}

fn render_fault(error: String) -> Fault {
    Fault {
        class: FaultClass::Protocol,
        reason: "render::unencodable".to_owned(),
        detail: error,
    }
}

fn deliver_fault(fault: Fault, form: Form) -> ! {
    let outcome: Outcome<()> = Outcome::Fault(fault);
    deliver(&outcome, form)
}

fn terminate_stream() -> ! {
    terminate_code(REPLY_EXIT);
}

#[cfg(not(target_arch = "wasm32"))]
fn terminate_child(status: ExitStatus) -> ! {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        if let Some(signal) = status.signal() {
            terminate_code(128 + signal);
        }
    }

    terminate_code(status.code().unwrap_or(FAULT_EXIT));
}

/// Renders one outcome in the selected form and exits on it.
///
/// An outcome that cannot be encoded is a defect in this binary's own DTOs
/// rather than anything the caller did, so it becomes a fault with its own
/// exit code. That fault is rendered as text, which is total, so this reaches
/// an exit in one step and never asks the encoder a second time.
fn deliver<B: serde::Serialize>(outcome: &Outcome<B>, form: Form) -> ! {
    match render::render(outcome, form) {
        Ok(rendering) => {
            rendering.speak();
            terminate(outcome);
        }
        Err(error) => {
            let fault: Outcome<()> = Outcome::Fault(render_fault(error));
            render::render(&fault, Form::Text)
                .expect("the text rendering of a fault encodes nothing")
                .speak();
            terminate(&fault);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn deliver_inbox_hook(
    outcome: &Outcome<wire::intent::CommandAnswer>,
    harness: &str,
    form: Form,
) -> ! {
    match outcome {
        Outcome::Reply(reply) => match steering_command::hook_document(&reply.body, harness) {
            Ok(Some(document)) => {
                let mut stdout = io::stdout().lock();
                let _ = stdout.write_all(&document);
                let _ = stdout.flush();
                terminate_code(REPLY_EXIT);
            }
            Ok(None) => terminate_code(REPLY_EXIT),
            Err(error) => deliver_fault(render_fault(error), Form::Text),
        },
        Outcome::Refusal(_) | Outcome::Fault(_) => deliver(outcome, form),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn deliver_with_code<B: serde::Serialize>(outcome: &Outcome<B>, form: Form, code: i32) -> ! {
    match render::render(outcome, form) {
        Ok(rendering) => {
            rendering.speak();
            terminate_code(code);
        }
        Err(error) => {
            let fault: Outcome<()> = Outcome::Fault(render_fault(error));
            render::render(&fault, Form::Text)
                .expect("the text rendering of a fault encodes nothing")
                .speak();
            terminate(&fault);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn deliver_release_notes(outcome: &Outcome<wire::ReleaseNotes>, form: Form) -> ! {
    if form == Form::Text
        && let Outcome::Reply(reply) = outcome
    {
        let mut document = reply.body.document.as_bytes().to_vec();
        if !document.ends_with(b"\n") {
            document.push(b'\n');
        }
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(&document);
        let _ = stdout.flush();
        terminate_code(REPLY_EXIT);
    }
    deliver(outcome, form)
}

#[cfg(not(target_arch = "wasm32"))]
fn deliver_config(outcome: &Outcome<config_command::ConfigReport>, form: Form) -> ! {
    if form == Form::Text
        && let Outcome::Reply(reply) = outcome
    {
        match reply.body.text() {
            Ok(document) => {
                let mut stdout = io::stdout().lock();
                let _ = stdout.write_all(&document);
                let _ = stdout.flush();
                terminate_code(REPLY_EXIT);
            }
            Err(error) => deliver_fault(render_fault(error.to_string()), Form::Text),
        }
    }
    deliver(outcome, form)
}

#[cfg(not(target_arch = "wasm32"))]
fn deliver_task_migration(outcome: &Outcome<spice::config::TaskMigrationReport>, form: Form) -> ! {
    if form == Form::Text
        && let Outcome::Reply(reply) = outcome
    {
        match reply.body.text() {
            Ok(document) => {
                let mut stdout = io::stdout().lock();
                let _ = stdout.write_all(&document);
                let _ = stdout.flush();
                terminate_code(REPLY_EXIT);
            }
            Err(error) => deliver_fault(render_fault(error.to_string()), Form::Text),
        }
    }
    deliver(outcome, form)
}

/// Acquire one ambient bootstrap value before typed configuration crosses
/// into the library graph.
#[expect(
    clippy::disallowed_methods,
    reason = "the binary rim owns ambient bootstrap acquisition"
)]
pub fn bootstrap_environment(name: impl AsRef<OsStr>) -> Option<OsString> {
    std::env::var_os(name)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn session_environment() -> session_command::Environment {
    let vendors = spice::extension::registry()
        .expect("compiled extension declarations are validated by the test gate")
        .vendors()
        .into_iter()
        .map(|vendor| {
            let layout = vendor.transcript_layout();
            session_command::VendorEnvironment::new(
                vendor,
                bootstrap_environment(vendor.thread_id_environment()),
                bootstrap_environment(layout.home_environment),
            )
        })
        .collect();
    session_command::Environment::new(
        vendors,
        bootstrap_environment("SPICE_AGENT_VENDOR"), // env-policy: allow
        bootstrap_environment("HOME"),
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn working_directory() -> Result<PathBuf, Outcome<()>> {
    std::env::current_dir().map_err(|error| {
        Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "environment::working-directory-unavailable".to_owned(),
            detail: error.to_string(),
        })
    })
}

/// Terminate the process after the command result has returned to the binary
/// rim.
pub fn terminate<B>(outcome: &Outcome<B>) -> ! {
    terminate_code(exit_status(outcome));
}

#[expect(
    clippy::disallowed_methods,
    reason = "the binary rim alone terminates the process"
)]
fn terminate_code(code: i32) -> ! {
    std::process::exit(code);
}

const fn exit_status<B>(outcome: &Outcome<B>) -> i32 {
    match outcome {
        Outcome::Reply(_) => REPLY_EXIT,
        Outcome::Refusal(_) => REFUSAL_EXIT,
        Outcome::Fault(_) => FAULT_EXIT,
    }
}

#[cfg(test)]
mod tests {
    use spice::cli;
    use spice::placeholder::PlaceholderReason;
    use wire::{Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition};

    use super::render::{self, Stream};
    use super::{
        FAULT_EXIT, REFUSAL_EXIT, REPLY_EXIT, exit_status, not_landed_from, target_unsupported,
    };

    fn reply() -> Outcome<()> {
        Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body: (),
        })
    }

    fn refusal() -> Outcome<()> {
        Outcome::Refusal(Refusal {
            reason: "command::refused".to_owned(),
            witness: None,
            expected: None,
            observed: None,
            remedy: "change the request".to_owned(),
        })
    }

    fn fault() -> Outcome<()> {
        Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "environment::unavailable".to_owned(),
            detail: "required service is unavailable".to_owned(),
        })
    }

    #[test]
    fn typed_outcomes_map_to_the_public_exit_contract() {
        assert_eq!(exit_status(&reply()), REPLY_EXIT);
        assert_eq!(exit_status(&refusal()), REFUSAL_EXIT);
        assert_eq!(exit_status(&fault()), FAULT_EXIT);
        assert_eq!((REPLY_EXIT, REFUSAL_EXIT, FAULT_EXIT), (0, 2, 1));
    }

    #[test]
    fn daemonless_local_os_contracts_have_named_wasm_refusals() {
        for (path, reason) in [
            (&["demo"][..], "demo::target-unsupported"),
            (&["install"][..], "install::target-unsupported"),
            (&["lock"][..], "lock::target-unsupported"),
            (&["lock", "run"][..], "lock::target-unsupported"),
            (&["restore", "verify"][..], "restore::target-unsupported"),
            (&["release", "notes"][..], "release::target-unsupported"),
            (
                &["config", "migrate-tasks"][..],
                "task-migration::target-unsupported",
            ),
        ] {
            let fault = target_unsupported(path)
                .unwrap_or_else(|| panic!("{} needs a wasm refusal", path.join(" ")));
            assert_eq!(fault.class, FaultClass::Environment);
            assert_eq!(fault.reason, reason);
            assert!(!fault.detail.is_empty());
        }
        assert!(target_unsupported(&["version"]).is_none());
    }

    #[test]
    fn landing_terminal_distinguishes_owned_and_unaccounted_paths() {
        let Outcome::Refusal(owned) = not_landed_from(
            &["fixture", "owned"],
            cli::Landing::GapOwned("FIXTURE-owner01"),
        ) else {
            panic!("an owned gap must remain one visible refusal");
        };
        assert_eq!(owned.reason, PlaceholderReason::NotLanded.as_str());
        // The witness serializes beside the reason, so it is a string an
        // operator matches on rather than private naming.
        assert_eq!(owned.witness.as_deref(), Some("landing-owner"));
        assert_eq!(owned.expected.as_deref(), Some("FIXTURE-owner01"));
        assert_eq!(owned.observed.as_deref(), Some("fixture owned"));

        let Outcome::Fault(unaccounted) =
            not_landed_from(&["fixture", "unknown"], cli::Landing::Unaccounted)
        else {
            panic!("an unaccounted path must fault");
        };
        assert_eq!(
            unaccounted.reason,
            PlaceholderReason::LandingUnaccounted.as_str()
        );
    }

    /// The stream an outcome speaks on and the exit it carries read the same.
    ///
    /// These are two independent classifications of one enum, written in two
    /// modules, and nothing but this makes them agree. A caller redirecting by
    /// stream and a caller branching on the code are asking the same question,
    /// so an outcome that answered on standard output while exiting nonzero
    /// would strand whichever of them guessed wrong — and a variant added to
    /// one match without the other is exactly how that happens.
    #[test]
    fn the_stream_an_outcome_speaks_on_agrees_with_the_exit_it_carries() {
        for outcome in [reply(), refusal(), fault()] {
            assert_eq!(
                render::stream(&outcome) == Stream::Out,
                exit_status(&outcome) == REPLY_EXIT,
                "{outcome:?} must read the same either way",
            );
        }
    }
}
