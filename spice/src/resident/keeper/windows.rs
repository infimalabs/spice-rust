//! Process-level entry points of the owner on Windows.

use super::super::*;

use std::io::Write as _;
use std::time::{Duration, Instant as Monotonic};

use spice::repository::StatePaths;

use super::definition::{self, Definition};
use super::owner;
use super::policy::{self, Attempt, Exit};
use super::record::{self, KeeperRecord};
use super::scheduler::windows::TaskService;

/// How long `install` and apply wait for the kept resident to publish.
const ARRIVAL_HORIZON: Duration = Duration::from_secs(60);
/// How often the wait re-reads discovery.
const ARRIVAL_POLL: Duration = Duration::from_millis(250);
/// Suffix of a rotated keeper log, mirroring the resident log.
const LOG_ROTATED_SUFFIX: &str = ".1";
/// Listen flags the installed owner fixes; a detached `up` naming one is
/// asking for a reinstall.
const INSTALL_FLAGS: [&str; 4] = ["host", "port", "allow-insecure-bind", "auth-token"];

/// The repository the command runs in and where its owner's state lives.
struct Site {
    current: PathBuf,
    repository: Repository,
    state: StatePaths,
    executable: PathBuf,
}

impl Site {
    fn admit() -> CommandResult<Self> {
        let current = std::env::current_dir()
            .map_err(|error| keeper_fault("resident::cwd-unavailable", &error))?;
        let repository = Repository::discover(&current)
            .map_err(|error| keeper_fault(spice::repository::ResolveError::REASON, &error))?;
        let state = repository
            .admit_resident_state()
            .map_err(|error| crate::state_failure(&error).outcome())?;
        fs::create_dir_all(state.runtime())
            .map_err(|error| keeper_fault("resident::runtime-unavailable", &error))?;
        let executable = std::env::current_exe()
            .map_err(|error| keeper_fault("resident::executable-unavailable", &error))?;
        Ok(Self {
            current,
            repository,
            state,
            executable,
        })
    }

    fn record(&self) -> CommandResult<Option<KeeperRecord>> {
        record::read(self.state.keeper())
            .map_err(|error| keeper_fault("resident::keeper-record-unavailable", &error))
    }

    fn write_record(&self, record: &KeeperRecord) -> CommandResult<()> {
        record::write(self.state.keeper(), record)
            .map_err(|error| keeper_fault("resident::keeper-record-unavailable", &error))
    }
}

/// `spice up --install`: record the listen address and access, register the
/// owner for the current user, and wait for the resident it starts.
pub(crate) fn install(leaf: &clap::ArgMatches, ambient: &Ambient) -> Outcome<wire::AppOpen> {
    let _ = ambient;
    reply(install_result(leaf))
}

fn install_result(leaf: &clap::ArgMatches) -> CommandResult<wire::AppOpen> {
    let site = Site::admit()?;
    let listen = requested_listen(leaf)?;
    let access = requested_access(leaf, listen, None)?;
    let record = KeeperRecord {
        task: definition::task_path(site.repository.common_dir()),
        executable: site.executable.clone(),
        working_directory: site.current.clone(),
        listen,
        allow_insecure_bind: leaf.get_flag("allow-insecure-bind"),
        auth_token: access.token,
    };
    site.write_record(&record)?;
    apply(&site, record, leaf.get_flag("open"))
}

/// Detached `spice up`: an installed owner is applied, otherwise the plain
/// detached launch runs.
pub(crate) fn up_detached(leaf: &clap::ArgMatches, ambient: &Ambient) -> Outcome<wire::AppOpen> {
    let site = match Site::admit() {
        Ok(site) => site,
        Err(outcome) => return typed_outcome(outcome),
    };
    match site.record() {
        Ok(None) => launch(leaf, None, leaf.get_flag("open"), ambient),
        Ok(Some(record)) => {
            if let Some(flag) = INSTALL_FLAGS
                .iter()
                .find(|flag| leaf.value_source(flag).is_some())
            {
                return installed_refusal(flag, &record.task);
            }
            reply(apply(&site, record, leaf.get_flag("open")))
        }
        Err(outcome) => typed_outcome(outcome),
    }
}

/// Register or refresh the owner's task from this executable, make sure it
/// is enabled and running, and answer with the resident it keeps.
fn apply(site: &Site, mut record: KeeperRecord, open: bool) -> CommandResult<wire::AppOpen> {
    if record.executable != site.executable {
        record.executable.clone_from(&site.executable);
        site.write_record(&record)?;
    }
    let user_sid = door::current_user_sid()
        .map_err(|error| keeper_fault("resident::keeper-principal-unavailable", &error))?;
    let xml = definition::render(&Definition {
        user_sid: &user_sid,
        executable: &record.executable,
        working_directory: &record.working_directory,
        repository: site.repository.common_dir(),
    });
    let mut scheduler = connect()?;
    owner::apply(&mut scheduler, &record.task, &xml)
        .map_err(|error| keeper_fault("resident::keeper-registration-failed", &error))?;
    let opened = await_discovery(site.state.serve())?;
    if record.pin_port(&opened.endpoint) {
        site.write_record(&record)?;
    }
    if open {
        crate::command_client::system_browser(&opened.endpoint)
            .map_err(|detail| environment_fault("serve::launch-failed", &detail))?;
    }
    Ok(opened)
}

/// `spice up --uninstall`: stop keeping, drain the resident, and remove the
/// registration with every artifact the owner wrote.
pub(crate) fn uninstall(ambient: &Ambient) -> Outcome<wire::KeeperRemoved> {
    reply(uninstall_result(ambient))
}

fn uninstall_result(ambient: &Ambient) -> CommandResult<wire::KeeperRemoved> {
    let site = Site::admit()?;
    let Some(record) = site.record()? else {
        return Err(Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                "resident::keeper-not-installed".to_owned(),
                Some("lifecycle-owner".to_owned()),
                Some("installed".to_owned()),
                Some("absent".to_owned()),
            )
            .with_detail("install one with `spice up --install`"),
        )));
    };
    let mut scheduler = connect()?;
    owner::hold(&mut scheduler, &record.task)
        .map_err(|error| keeper_fault("resident::keeper-registration-failed", &error))?;
    let drained = drain_live_resident(&site, ambient)?;
    owner::release(&mut scheduler, &record.task)
        .map_err(|error| keeper_fault("resident::keeper-registration-failed", &error))?;
    let mut rotated = site.state.keeper_log().as_os_str().to_owned();
    rotated.push(LOG_ROTATED_SUFFIX);
    owner::remove_artifacts(&[
        site.state.keeper(),
        site.state.keeper_log(),
        Path::new(&rotated),
        site.state.keeper_lock(),
    ])
    .map_err(|error| keeper_fault("resident::keeper-record-unavailable", &error))?;
    Ok(wire::KeeperRemoved {
        task: record.task,
        drained,
    })
}

/// Drain the resident if one is serving; answer whether one was.
fn drain_live_resident(site: &Site, ambient: &Ambient) -> CommandResult<bool> {
    if crate::command_client::open_app_result_with(site.state.serve(), || {}, |_| Ok(())).is_err() {
        return Ok(false);
    }
    let contract = crate::cli::contract(&["down"]).expect("down is a registered command");
    match down_from_operator(&site.current, contract, ambient) {
        Outcome::Reply(_) => Ok(true),
        Outcome::Refusal(refusal) => Err(Outcome::Refusal(refusal)),
        Outcome::Fault(fault) => Err(Outcome::Fault(fault)),
    }
}

/// `spice down`: forbid the owner to relaunch before the resident drains.
pub(crate) fn hold_stopped(repository: &Repository) -> CommandResult<()> {
    let state = repository
        .admit_resident_state()
        .map_err(|error| crate::state_failure(&error).outcome())?;
    let Some(record) = record::read(state.keeper())
        .map_err(|error| keeper_fault("resident::keeper-record-unavailable", &error))?
    else {
        return Ok(());
    };
    let mut scheduler = connect()?;
    owner::hold(&mut scheduler, &record.task)
        .map_err(|error| keeper_fault("resident::keeper-registration-failed", &error))
}

/// `spice up --keeper`: the task's action. Relaunch the resident until the
/// task is disabled or deleted, appending to the owner's log.
pub(crate) fn keep(ambient: &Ambient) -> Outcome<()> {
    match keep_result(ambient) {
        Ok(()) => Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body: (),
        }),
        Err(outcome) => outcome,
    }
}

fn keep_result(ambient: &Ambient) -> CommandResult<()> {
    let site = Site::admit()?;
    let mut log = rotate_log(site.state.keeper_log())
        .and_then(|()| open_log(site.state.keeper_log()))
        .map_err(|error| keeper_fault("resident::log-unavailable", &error))?;
    let Some(record) = site.record()? else {
        let _ = writeln!(log, "no lifecycle owner is recorded; keeper exits");
        return Ok(());
    };
    let name = LockName::new("keeper").expect("keeper is a non-empty lock name");
    let lock = LockFile::new(name, site.state.keeper_lock());
    let possession = match lock.try_acquire() {
        Ok(Acquisition::Acquired(possession)) => possession,
        Ok(Acquisition::Contended) => {
            let _ = writeln!(log, "another keeper holds this repository; this one exits");
            return Ok(());
        }
        Err(error) => return Err(keeper_fault("resident::keeper-lock-unavailable", &error)),
    };
    let mut scheduler = connect()?;
    let mut attempt = || attempt_launch(&site, ambient);
    let mut pause = |delay: Duration| std::thread::sleep(delay);
    let end = policy::keep(
        &mut scheduler,
        &record.task,
        &mut attempt,
        &mut pause,
        &mut log,
    )
    .map_err(|error| keeper_fault("resident::keeper-failed", &error))?;
    let _ = writeln!(log, "keeper ended: {end:?}");
    drop(possession);
    Ok(())
}

/// Start one resident from the current record; the record is re-read so a
/// reinstall takes effect at the next relaunch.
fn attempt_launch(site: &Site, ambient: &Ambient) -> Attempt {
    let record = match site.record() {
        Ok(Some(record)) => record,
        Ok(None) => return Attempt::Failed("the lifecycle owner record is gone".to_owned()),
        Err(outcome) => return Attempt::Failed(outcome_text(&outcome)),
    };
    let matches = match crate::cli::command().try_get_matches_from(record.launch_argv()) {
        Ok(matches) => matches,
        Err(error) => return Attempt::Failed(error.to_string()),
    };
    let leaf = crate::cli::selected_matches(&matches);
    match launch_held(leaf, record.auth_token.clone(), false, ambient) {
        (Outcome::Reply(reply), Some(mut child)) => Attempt::Ready {
            endpoint: reply.body.endpoint,
            wait: Box::new(move || {
                let started = Monotonic::now();
                let detail = match child.wait() {
                    Ok(status) => status.to_string(),
                    Err(error) => format!("wait failed: {error}"),
                };
                Exit {
                    detail,
                    ran_for: started.elapsed(),
                }
            }),
        },
        (Outcome::Reply(_), None) => {
            Attempt::Failed("the launcher reported readiness without a child".to_owned())
        }
        (outcome, _) => Attempt::Failed(outcome_text(&outcome)),
    }
}

/// Wait for a live resident to publish discovery and admit its endpoint.
fn await_discovery(serve: &Path) -> CommandResult<wire::AppOpen> {
    let deadline = Monotonic::now() + ARRIVAL_HORIZON;
    loop {
        match crate::command_client::open_app_result_with(serve, || {}, |_| Ok(())) {
            Ok(opened) => return Ok(opened),
            Err(fault) if Monotonic::now() >= deadline => {
                return Err(environment_fault(
                    "resident::keeper-arrival-timeout",
                    &format!(
                        "the lifecycle owner published no live resident within {}s: {}",
                        ARRIVAL_HORIZON.as_secs(),
                        fault.detail
                    ),
                ));
            }
            Err(_) => std::thread::sleep(ARRIVAL_POLL),
        }
    }
}

fn connect() -> CommandResult<TaskService> {
    TaskService::connect()
        .map_err(|error| keeper_fault("resident::keeper-scheduler-unavailable", &error))
}

fn installed_refusal<B>(flag: &str, task: &str) -> Outcome<B> {
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            "resident::keeper-installed".to_owned(),
            Some("lifecycle-owner".to_owned()),
            Some("no listen or access flags".to_owned()),
            Some(format!("--{flag}")),
        )
        .with_detail(format!(
            "{task} fixes the listen address and access; run `spice up --install` \
             with the new flags to change them, or `spice up --uninstall` first"
        )),
    ))
}

fn reply<B>(result: CommandResult<B>) -> Outcome<B> {
    match result {
        Ok(body) => Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body,
        }),
        Err(outcome) => typed_outcome(outcome),
    }
}

fn keeper_fault(reason: &str, error: &dyn std::fmt::Display) -> Outcome<()> {
    environment_fault(reason, &error.to_string())
}

fn outcome_text<B: serde::Serialize>(outcome: &Outcome<B>) -> String {
    serde_json::to_string(outcome).unwrap_or_else(|error| error.to_string())
}
