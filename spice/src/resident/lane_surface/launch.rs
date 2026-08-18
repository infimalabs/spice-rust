//! Occupant process launch and its configuration refusals.

use std::ffi::OsString;

use sill::{
    Cancellation, Deadlines, Environment, Observation, RestartDisposition, Spec, WaitPolicy,
};

use super::*;

pub(in crate::resident) struct OccupantLaunch {
    pub(in crate::resident) spec: Spec,
    pub(in crate::resident) observation: Observation,
    pub(in crate::resident) session: SessionProof,
    pub(in crate::resident) git: String,
}

pub(in crate::resident) struct OccupantLaunchRequest<'a> {
    pub(in crate::resident) worktree: &'a str,
    pub(in crate::resident) vendor: &'a wire::Name,
    pub(in crate::resident) epoch: CanonicalUuid,
    pub(in crate::resident) lane: CanonicalUuid,
    pub(in crate::resident) occupant: CanonicalUuid,
    pub(in crate::resident) resume: vendor::Resume,
}

pub(in crate::resident) fn occupant_launch(
    paths: &RuntimePaths,
    request: OccupantLaunchRequest<'_>,
    reporter: Arc<dyn CaptureReporter>,
) -> CommandResult<OccupantLaunch> {
    let OccupantLaunchRequest {
        worktree,
        vendor: name,
        epoch,
        lane,
        occupant,
        resume,
    } = request;
    let repository = launch_repository(paths, worktree)?;
    // Run ahead of configuration load so the launch reads the advanced tree.
    // Keep this call at the live seam: `xtask/tests/git_boundaries.rs` proves
    // both that every launch reaches it and that its exact note is published.
    let git = spice::task_git::advance_at_launch(&repository).to_string();
    let resolved = spice::config::load_repository(&repository, &[])
        .map_err(|error| configuration_refusal(&error))?;
    let plan = launch_plan(paths, &repository, &resolved, name, lane, resume)?;
    let model = plan.launch().model.clone();
    let effort = plan.launch().effort.as_str().to_owned();
    let record = plan
        .record()
        .map_err(|error| store_fault("lane::launch-invalid", &error.to_string()))?;
    let process = launch_process(&repository, &resolved, plan.environment().clone(), lane)?;
    let spec = record.spec(&process);
    let stream = record.stream();
    let expected = record.resume().session_id().map(str::to_owned);
    let tape = tape::Tape::open(paths.tape.join(lane.to_string()).join(name.as_str()))
        .map_err(|error| store_fault("lane::capture-tape-unavailable", &error.to_string()))?;
    let (session_reply, session) = mpsc::sync_channel(4);
    let horizon = session_proof_horizon(paths);
    let state = Arc::new(Mutex::new(DeclarationState {
        reply: session_reply,
        seen: None,
        witness: DeclarationWitness::default(),
    }));
    let declaration_reporter: Arc<dyn CaptureReporter> = Arc::new(DeclarationReporter {
        inner: reporter,
        lane,
        occupant,
        worktree: repository.worktree_root().display().to_string(),
        vendor: name.clone(),
        model,
        effort,
        git: git.clone(),
        epoch,
        stream,
        expected,
        state: Arc::clone(&state),
    });
    let observation = lane::capture::observation(
        lane,
        occupant,
        record,
        tape,
        CAPTURE_LIMIT,
        RECORD_QUEUE,
        declaration_reporter,
    )
    .map_err(|error| store_fault("lane::capture-invalid", &error.to_string()))?;
    Ok(OccupantLaunch {
        spec,
        observation,
        session: SessionProof {
            session,
            state,
            horizon,
        },
        git,
    })
}

fn session_proof_horizon(paths: &RuntimePaths) -> Duration {
    // The marker exists only in selected integration fixtures. Ordinary
    // resident launches in this same featured binary retain the production
    // proof horizon, including under the canonical coordinator's load.
    #[cfg(feature = "test-short-horizons")]
    if paths.runtime.join("test-short-session-proof").is_file() {
        return Duration::from_secs(3);
    }
    #[cfg(not(feature = "test-short-horizons"))]
    let _ = paths;
    SESSION_PROOF
}

/// Admit only the durable worktree that still names this repository's root.
fn launch_repository(paths: &RuntimePaths, worktree: &str) -> CommandResult<Repository> {
    let repository = Repository::discover(worktree)
        .map_err(|error| store_fault("lane::worktree-unresolved", &error.to_string()))?;
    if repository.common_dir() != paths.common || repository.worktree_root() != Path::new(worktree)
    {
        return Err(store_fault(
            "lane::worktree-moved",
            "durable lane worktree no longer names one current repository worktree root",
        ));
    }
    Ok(repository)
}

/// Compose the vendor launch plan this lane's configuration admits.
fn launch_plan(
    paths: &RuntimePaths,
    repository: &Repository,
    resolved: &spice::config::Resolved,
    name: &wire::Name,
    lane: CanonicalUuid,
    resume: vendor::Resume,
) -> CommandResult<spice::agent_launch::Plan> {
    let executable = std::env::current_exe()
        .map_err(|error| store_fault("lane::executable-unresolved", &error.to_string()))?;
    let environment = launch_environment(paths, repository, &executable, lane)?;
    let Some(vendor) = spice::extension::vendor(name.as_str()) else {
        return Err(Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                "lane::vendor-unsupported".to_owned(),
                Some("lane.vendor".to_owned()),
                Some(
                    spice::extension::registry()
                        .expect("compiled extension declarations are validated by the test gate")
                        .vendors()
                        .into_iter()
                        .map(|vendor| vendor.name())
                        .collect::<Vec<_>>()
                        .join(" or "),
                ),
                Some(name.as_str().to_owned()),
            ),
        )));
    };
    let prompt = vendor.neutral_launch_prompt(&spice::install::installed_skill(repository));
    spice::agent_launch::compose(
        resolved.root(),
        spice::agent_launch::Inputs {
            worktree: repository.worktree_root().to_path_buf(),
            vendor: name.clone(),
            prompt,
            executable: executable.display().to_string(),
            environment,
            lane,
            resume,
        },
    )
    .map_err(|error| match error {
        spice::agent_launch::ComposeError::UnsupportedVendor(other) => {
            Outcome::Refusal(Refusal::bare(wire::RefusalDiagnostic::new(
                "lane::vendor-unsupported".to_owned(),
                Some("lane.vendor".to_owned()),
                Some(
                    spice::extension::registry()
                        .expect("compiled extension declarations are validated by the test gate")
                        .vendors()
                        .into_iter()
                        .map(|vendor| vendor.name())
                        .collect::<Vec<_>>()
                        .join(" or "),
                ),
                Some(other),
            )))
        }
        other => store_fault("lane::launch-invalid", &other.to_string()),
    })
}

/// Carry only the ambient variables a supervised occupant is admitted to see.
///
/// The executable's directory leads `PATH` so every bare `spice` in the
/// installed skill resolves to the same appliance that composed the exact hook
/// and MCP commands. This is a routing boundary, not a convenience: the Python
/// predecessor also installs a program named `spice`, and inheriting its
/// earlier ambient entry would send agent-shell inbox and acknowledgment
/// commands to the wrong runtime.
fn launch_environment(
    paths: &RuntimePaths,
    repository: &Repository,
    executable: &Path,
    lane: CanonicalUuid,
) -> CommandResult<Environment> {
    let inbox = paths.inbox_for_repository(repository)?;
    let cargo_host = crate::cargo_command::host()
        .map_err(|error| store_fault("lane::cargo-isolation-unavailable", &error.to_string()))?;
    // The lane-specific home is the resident reader pin: one occupied seat
    // admits one process group, and a replacement occupant cannot prepare the
    // same lane home until that seat is vacant. Manual Cargo children can
    // overlap a home advance, so their preparer additionally holds a kernel
    // pin for the exact child lifetime.
    let cargo_home = spice::cargo_isolation::prepare_lane(repository, lane, &cargo_host)
        .map_err(|error| store_fault("lane::cargo-isolation-unavailable", &error.to_string()))?;
    let native_shell = crate::bootstrap_environment("SHELL").ok_or_else(|| {
        store_fault(
            "lane::native-shell-unavailable",
            "the supervised environment has no native SHELL to restore behind interception",
        )
    })?;
    let mut values = ["HOME", "TMPDIR", "USER"]
        .into_iter()
        .filter_map(|key| crate::bootstrap_environment(key).map(|value| (key.into(), value)))
        .collect::<Vec<(OsString, OsString)>>();
    values.extend([
        (
            OsString::from(spice::cargo_isolation::CARGO_HOME_VARIABLE),
            cargo_home.home().as_os_str().to_owned(),
        ),
        (
            OsString::from(spice::cargo_isolation::BOOTSTRAP_HOME_VARIABLE),
            cargo_host.bootstrap_home().as_os_str().to_owned(),
        ),
        (
            OsString::from(spice::cargo_isolation::NATIVE_CARGO_VARIABLE),
            cargo_host.cargo().as_os_str().to_owned(),
        ),
        (
            OsString::from("SHELL"),
            spice::install::installed_shell(repository).into_os_string(),
        ),
        (
            OsString::from(steer::hook::HOOKS),
            spice::install::installed_shell_hooks(repository).into_os_string(),
        ),
        (OsString::from(steer::hook::NATIVE_SHELL), native_shell),
        (
            OsString::from(steer::hook::INBOX),
            steer::inbox::path(&inbox, lane).into_os_string(),
        ),
    ]);
    values.push(("PATH".into(), appliance_path(executable)?));
    let environment = Environment::new(values)
        .map_err(|error| store_fault("lane::environment-invalid", &error.to_string()))?;
    spice::git_shadow::environment(repository, environment)
        .map_err(|error| store_fault("lane::git-shadow-unavailable", &error.to_string()))
}

/// Search path whose first `spice` is the appliance supervising this lane.
fn appliance_path(executable: &Path) -> CommandResult<OsString> {
    let Some(directory) = executable.parent() else {
        return Err(store_fault(
            "lane::executable-unresolved",
            "running executable has no parent directory",
        ));
    };
    let mut entries = vec![directory.to_path_buf()];
    if let Some(ambient) = crate::bootstrap_environment("PATH") {
        entries.extend(std::env::split_paths(&ambient));
    }
    std::env::join_paths(entries)
        .map_err(|error| store_fault("lane::environment-invalid", &error.to_string()))
}

/// Bind the lease, wait policy, and capture bounds this lane launches under.
fn launch_process(
    repository: &Repository,
    resolved: &spice::config::Resolved,
    environment: Environment,
    lane: CanonicalUuid,
) -> CommandResult<vendor::Process> {
    let deadlines = Deadlines::new(resolved.root().lane().lease().get(), OUTPUT_DRAIN, REAP)
        .map_err(|error| store_fault("lane::deadlines-invalid", &error.to_string()))?;
    let wait = WaitPolicy::new(
        format!("lane {lane}"),
        Cancellation::Never,
        RestartDisposition::CallerDecides,
    )
    .map_err(|error| store_fault("lane::wait-invalid", &error.to_string()))?;
    vendor::Process::new(
        repository.worktree_root(),
        environment,
        deadlines,
        wait,
        CAPTURE_LIMIT,
        TERMINATION_GRACE,
    )
    .map_err(|error| store_fault("lane::process-invalid", &error.to_string()))
}

pub(super) fn configuration_refusal(error: &spice::config::LoadError) -> Outcome<()> {
    Outcome::Refusal(Refusal::bare(wire::RefusalDiagnostic::new(
        "lane::configuration-refused".to_owned(),
        Some("effective-configuration".to_owned()),
        Some("tracked and local inputs admitted by the configuration registry".to_owned()),
        Some(error.diagnostic()),
    )))
}
