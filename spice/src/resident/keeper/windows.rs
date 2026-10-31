//! Process-level entry points of the owner on Windows.

use super::super::*;

use std::fs::File;
use std::io::Write as _;
use std::time::{Duration, Instant as Monotonic};

use spice::repository::StatePaths;

use super::definition::{self, Definition};
use super::owner;
use super::ownership::{self, Mismatch, Owners};
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
    binary: wire::Digest,
}

impl Site {
    fn admit() -> CommandResult<Self> {
        Self::admit_checked(|_| Ok(()))
    }

    /// Admit the site for a lifecycle write, refusing before the runtime root
    /// is created when its state would not belong to the repository user.
    fn admit_owning() -> CommandResult<Self> {
        Self::admit_checked(admit_ownership)
    }

    fn admit_checked(check: fn(&StatePaths) -> CommandResult<()>) -> CommandResult<Self> {
        let current = std::env::current_dir()
            .map_err(|error| keeper_fault("resident::cwd-unavailable", &error))?;
        let repository = Repository::discover(&current)
            .map_err(|error| keeper_fault(spice::repository::ResolveError::REASON, &error))?;
        let state = repository
            .admit_resident_state()
            .map_err(|error| crate::state_failure(&error).outcome())?;
        check(&state)?;
        fs::create_dir_all(state.runtime())
            .map_err(|error| keeper_fault("resident::runtime-unavailable", &error))?;
        let executable = std::env::current_exe()
            .map_err(|error| keeper_fault("resident::executable-unavailable", &error))?;
        let binary = spice::version::running()
            .map_err(|fault| Outcome::Fault(fault.clone()))?
            .binary
            .clone();
        Ok(Self {
            current,
            repository,
            state,
            executable,
            binary,
        })
    }

    fn require_repository(&self, repository: &Repository) -> CommandResult<()> {
        if self.repository.common_dir() == repository.common_dir() {
            Ok(())
        } else {
            Err(environment_fault(
                "install::keeper-repository-moved",
                "the lifecycle owner command no longer addresses the admitted install repository",
            ))
        }
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
    reply(install_result(leaf, ambient))
}

fn install_result(leaf: &clap::ArgMatches, ambient: &Ambient) -> CommandResult<wire::AppOpen> {
    let site = Site::admit_owning()?;
    let listen = requested_listen(leaf)?;
    let access = requested_access(leaf, listen, None)?;
    let managed_default = INSTALL_FLAGS
        .iter()
        .all(|flag| leaf.value_source(flag) != Some(clap::parser::ValueSource::CommandLine));
    let mut record = KeeperRecord {
        task: definition::task_path(site.repository.common_dir()),
        executable: site.executable.clone(),
        binary: site.binary.clone(),
        working_directory: site.current.clone(),
        listen,
        allow_insecure_bind: leaf.get_flag("allow-insecure-bind"),
        auth_token: access.token,
        managed_default,
    };
    let previous = site.record()?;
    if let Some(previous) = &previous {
        record.inherit_port(previous);
    }
    let replace_owner = previous
        .as_ref()
        .is_some_and(|previous| previous.owner_changed_by(&record));
    site.write_record(&record)?;
    if previous
        .as_ref()
        .is_none_or(|previous| previous.reconfigured_by(&record))
    {
        // A first install replaces any ordinary detached resident. A changed
        // install disables and drains the old keeper's child before the task
        // is refreshed, so that keeper cannot keep launching its own previous
        // executable or serving the previous listen/access policy.
        drain_live_resident(&site, ambient)?;
    }
    if replace_owner {
        retire_task(&record.task)?;
    }
    apply(&site, record, leaf.get_flag("open"), ambient)
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
            if let Some(flag) = INSTALL_FLAGS.iter().find(|flag| {
                leaf.value_source(flag) == Some(clap::parser::ValueSource::CommandLine)
            }) {
                return installed_refusal(flag, &record.task);
            }
            reply(
                admit_ownership(&site.state)
                    .and_then(|()| apply(&site, record, leaf.get_flag("open"), ambient)),
            )
        }
        Err(outcome) => typed_outcome(outcome),
    }
}

/// Register or refresh the owner's task from this executable, make sure it
/// is enabled and running, and answer with the resident it keeps.
fn apply(
    site: &Site,
    mut record: KeeperRecord,
    open: bool,
    ambient: &Ambient,
) -> CommandResult<wire::AppOpen> {
    if record.binary != site.binary || record.executable != site.executable {
        record.binary.clone_from(&site.binary);
        record.executable.clone_from(&site.executable);
        site.write_record(&record)?;
        // A bare `up` after package replacement must retire the keeper that
        // is still running the previous appliance before Task Scheduler can
        // start the refreshed action.
        drain_live_resident(site, ambient)?;
        retire_task(&record.task)?;
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
    remove_owner(&site, record, ambient)
}

/// Refuse a Windows lifecycle write from this process when the repository's
/// lifecycle state would not belong to the repository user.
pub(crate) fn admit_owner(repository: &Repository) -> CommandResult<()> {
    let state = repository
        .admit_resident_state()
        .map_err(|error| crate::state_failure(&error).outcome())?;
    admit_ownership(&state)
}

fn admit_ownership(state: &StatePaths) -> CommandResult<()> {
    let token = sill::authority::token_owners()
        .map_err(|error| keeper_fault("resident::keeper-principal-unavailable", &error))?;
    let runtime = match sill::authority::owner_sid(state.runtime()) {
        Ok(owner) => Some(owner),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(keeper_fault("resident::runtime-unavailable", &error)),
    };
    let Some(mismatch) = ownership::mismatch(&Owners {
        user: &token.user,
        default_owner: &token.default_owner,
        runtime: runtime.as_deref(),
    }) else {
        return Ok(());
    };
    let (subject, observed, detail) = match mismatch {
        Mismatch::DefaultOwner(owner) => (
            "default-owner",
            owner,
            "this elevated shell would create lifecycle state another principal owns; \
             rerun from an unelevated shell as the repository user"
                .to_owned(),
        ),
        Mismatch::Runtime(owner) => (
            "runtime-owner",
            owner,
            format!(
                "{} belongs to another principal and is not taken over; have its owner \
                 remove it, then rerun from an unelevated shell as the repository user",
                state.runtime().display()
            ),
        ),
    };
    Err(Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            "install::lifecycle-owner-mismatch".to_owned(),
            Some(subject.to_owned()),
            Some(token.user.clone()),
            Some(observed.to_owned()),
        )
        .with_detail(detail),
    )))
}

/// Capture the semantic lifecycle policy for a digest-bound package plan.
pub(crate) fn install_standing(
    repository: &Repository,
) -> CommandResult<Option<spice::install::LifecycleOwnerStanding>> {
    let state = repository
        .admit_resident_state()
        .map_err(|error| crate::state_failure(&error).outcome())?;
    let standing = match record::read(state.keeper())
        .map_err(|error| keeper_fault("resident::keeper-record-unavailable", &error))?
    {
        Some(record) => record
            .standing()
            .map_err(|error| keeper_fault("resident::keeper-record-unavailable", &error))?,
        None => spice::install::LifecycleOwnerStanding::Default,
    };
    Ok(Some(standing))
}

/// Whether durable owner policy already exists for this repository.
pub(crate) fn owner_installed(repository: &Repository) -> CommandResult<bool> {
    let state = repository
        .admit_resident_state()
        .map_err(|error| crate::state_failure(&error).outcome())?;
    record::read(state.keeper())
        .map(|record| record.is_some())
        .map_err(|error| keeper_fault("resident::keeper-record-unavailable", &error))
}

/// Remove the retained private lifecycle artifacts after package reversal.
///
/// The task was already released by [`quiesce_for_install`], so this step does
/// not need the executable the reversal just removed.
pub(crate) fn finish_unapply(repository: &Repository) -> CommandResult<()> {
    let state = repository
        .admit_resident_state()
        .map_err(|error| crate::state_failure(&error).outcome())?;
    let mut rotated = state.keeper_log().as_os_str().to_owned();
    rotated.push(LOG_ROTATED_SUFFIX);
    owner::remove_artifacts(&[
        state.keeper(),
        state.keeper_log(),
        Path::new(&rotated),
        state.keeper_lock(),
    ])
    .map_err(|error| keeper_fault("resident::keeper-record-unavailable", &error))
}

fn remove_owner(
    site: &Site,
    record: KeeperRecord,
    ambient: &Ambient,
) -> CommandResult<wire::KeeperRemoved> {
    let mut scheduler = connect()?;
    owner::hold(&mut scheduler, &record.task)
        .map_err(|error| keeper_fault("resident::keeper-registration-failed", &error))?;
    let drained = drain_live_resident(site, ambient)?;
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

/// Quiesce and unregister an owner while retaining its private policy record
/// so the newly installed appliance can register and start it again.
pub(crate) fn quiesce_for_install(
    repository: &Repository,
    expected: &spice::install::LifecycleOwnerStanding,
    ambient: &Ambient,
) -> CommandResult<bool> {
    let site = Site::admit()?;
    site.require_repository(repository)?;
    let record = site.record()?;
    let observed = match &record {
        Some(record) => record
            .standing()
            .map_err(|error| keeper_fault("resident::keeper-record-unavailable", &error))?,
        None => spice::install::LifecycleOwnerStanding::Default,
    };
    if &observed != expected {
        return Err(Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                "install::keeper-standing-changed".to_owned(),
                Some("lifecycle-owner".to_owned()),
                Some(format!("{expected:?}")),
                Some(format!("{observed:?}")),
            )
            .with_detail("preview the package install again before applying it"),
        )));
    }
    let Some(record) = record else {
        return Ok(false);
    };
    let mut scheduler = connect()?;
    owner::hold(&mut scheduler, &record.task)
        .map_err(|error| keeper_fault("resident::keeper-registration-failed", &error))?;
    drain_live_resident(&site, ambient)?;
    owner::release(&mut scheduler, &record.task)
        .map_err(|error| keeper_fault("resident::keeper-registration-failed", &error))?;
    Ok(true)
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
    let kept = keep_logged(&site, ambient, &mut log);
    if let Err(outcome) = &kept {
        let _ = writeln!(log, "keeper failed: {}", outcome_text(outcome));
    }
    kept
}

fn keep_logged(site: &Site, ambient: &Ambient, log: &mut File) -> CommandResult<()> {
    let Some(record) = site.record()? else {
        let _ = writeln!(log, "no lifecycle owner is recorded; keeper exits");
        return Ok(());
    };
    fs::create_dir_all(site.state.locks())
        .map_err(|error| keeper_fault("resident::keeper-lock-unavailable", &error))?;
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
    let mut attempt = || attempt_launch(site, ambient);
    let mut pause = |delay: Duration| std::thread::sleep(delay);
    let end = policy::keep(&mut scheduler, &record.task, &mut attempt, &mut pause, log)
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

fn retire_task(task: &str) -> CommandResult<()> {
    let mut scheduler = connect()?;
    owner::hold(&mut scheduler, task)
        .and_then(|()| owner::release(&mut scheduler, task))
        .map_err(|error| keeper_fault("resident::keeper-registration-failed", &error))
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
