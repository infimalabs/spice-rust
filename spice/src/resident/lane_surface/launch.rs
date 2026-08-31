//! Occupant process launch and its configuration refusals.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

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
    let (plan, rtk) = launch_plan(paths, &repository, &resolved, name, lane, resume)?;
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
        rtk,
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
        return Duration::from_secs(8);
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
) -> CommandResult<(spice::agent_launch::Plan, wire::body::RtkHealth)> {
    let skill = required_skill(repository)?;
    let wrapper = resolved
        .root()
        .wrappers()
        .spice()
        .map(|reference| spice::agent_wrapper::resolve(reference, repository))
        .transpose()
        .map_err(|error| store_fault(error.reason(), &error.to_string()))?;
    let executable = match &wrapper {
        Some(route) => route.executable().to_path_buf(),
        None => std::env::current_exe()
            .map_err(|error| store_fault("lane::executable-unresolved", &error.to_string()))?,
    };
    let wrapper_command = wrapper
        .as_ref()
        .map(spice::agent_wrapper::Resolved::command);
    let (environment, rtk) = launch_environment(
        paths,
        repository,
        &executable,
        wrapper_command,
        resolved.root().rtk().executable(),
        lane,
    )?;
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
    let prompt = vendor.neutral_launch_prompt(&skill);
    let plan = spice::agent_launch::compose(
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
    })?;
    Ok((plan, rtk))
}

/// Require the initialized Git-common skill before composing any vendor argv.
fn required_skill(repository: &Repository) -> CommandResult<PathBuf> {
    let skill = spice::install::installed_skill(repository);
    let metadata = fs::symlink_metadata(&skill).map_err(|error| {
        store_fault(
            "lane::skill-unavailable",
            &format!(
                "initialized skill {} is unavailable: {error}",
                skill.display()
            ),
        )
    })?;
    if !metadata.is_file() {
        return Err(store_fault(
            "lane::skill-unavailable",
            &format!(
                "initialized skill {} is not a regular file",
                skill.display()
            ),
        ));
    }
    let common = fs::canonicalize(repository.common_dir()).map_err(|error| {
        store_fault(
            "lane::skill-unavailable",
            &format!(
                "initialized skill authority {} is unavailable: {error}",
                repository.common_dir().display()
            ),
        )
    })?;
    let physical = fs::canonicalize(&skill).map_err(|error| {
        store_fault(
            "lane::skill-unavailable",
            &format!(
                "initialized skill {} is unavailable: {error}",
                skill.display()
            ),
        )
    })?;
    if !physical.starts_with(&common) {
        return Err(store_fault(
            "lane::skill-unavailable",
            &format!(
                "initialized skill {} resolves outside Git-common authority: {}",
                skill.display(),
                physical.display()
            ),
        ));
    }
    let bytes = fs::read(&skill).map_err(|error| {
        store_fault(
            "lane::skill-unavailable",
            &format!(
                "initialized skill {} is unavailable: {error}",
                skill.display()
            ),
        )
    })?;
    if bytes != include_bytes!("../../../agent/SKILL.md") {
        return Err(store_fault(
            "lane::skill-unavailable",
            &format!(
                "initialized skill {} does not match this appliance",
                skill.display()
            ),
        ));
    }
    Ok(skill)
}

/// Carry only the ambient variables a supervised occupant is admitted to see.
///
/// A configured shell-wrapper directory leads `PATH` so every bare `spice` in
/// the installed skill reaches the route selected by tracked policy. The Rust
/// appliance itself is carried separately as an exact typed environment path
/// for the wrapper, hook, and MCP commands; its directory never enters `PATH`.
/// Without tracked wrapper policy, the ambient search path is preserved.
fn launch_environment(
    paths: &RuntimePaths,
    repository: &Repository,
    executable: &Path,
    wrapper_command: Option<&Path>,
    rtk_executable: &str,
    lane: CanonicalUuid,
) -> CommandResult<(Environment, wire::body::RtkHealth)> {
    let inbox = paths.inbox_for_repository(repository)?;
    let cargo_host = crate::cargo_command::host(repository)
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
    let state = repository
        .admit_state()
        .map_err(|error| crate::state_failure(&error).outcome())?;
    let rtk_history = state.rtk_history(lane);
    fs::create_dir_all(
        rtk_history
            .parent()
            .expect("the RTK history contract always has a parent"),
    )
    .map_err(|error| store_fault("lane::rtk-history-unavailable", &error.to_string()))?;
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
            OsString::from(spice::cargo_isolation::NATIVE_RUSTC_VARIABLE),
            cargo_host.rustc().as_os_str().to_owned(),
        ),
        (
            OsString::from(spice::cargo_isolation::NATIVE_RUSTDOC_VARIABLE),
            cargo_host.rustdoc().as_os_str().to_owned(),
        ),
        (
            OsString::from(spice::cargo_isolation::RUSTC_VARIABLE),
            cargo_host.rustc().as_os_str().to_owned(),
        ),
        (
            OsString::from(spice::cargo_isolation::RUSTDOC_VARIABLE),
            cargo_host.rustdoc().as_os_str().to_owned(),
        ),
        (
            OsString::from("SHELL"),
            spice::install::installed_shell(repository).into_os_string(),
        ),
        (
            OsString::from(steer::hook::BASH_ENV),
            spice::install::installed_bash_env(repository).into_os_string(),
        ),
        (
            OsString::from(steer::hook::ZDOTDIR),
            spice::install::installed_zdotdir(repository).into_os_string(),
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
        (
            OsString::from(steer::hook::EXECUTABLE),
            executable.as_os_str().to_owned(),
        ),
        (
            OsString::from(spice::rtk::EXECUTABLE_VARIABLE),
            OsString::from(rtk_executable),
        ),
        (
            OsString::from(spice::rtk::HISTORY_VARIABLE),
            rtk_history.into_os_string(),
        ),
    ]);
    if let Some(path) = shell_path(wrapper_command, executable)? {
        values.push(("PATH".into(), path));
    }
    let environment = Environment::new(values)
        .map_err(|error| store_fault("lane::environment-invalid", &error.to_string()))?;
    let environment = spice::git_shadow::environment(repository, environment)
        .map_err(|error| store_fault("lane::git-shadow-unavailable", &error.to_string()))?;
    let probe_environment = rtk_probe_environment(&environment);
    let health = spice::rtk::probe_health_in_environment(
        std::ffi::OsStr::new(rtk_executable),
        repository.worktree_root(),
        &probe_environment,
    );
    let environment = rtk_environment(environment, &health);
    Ok((environment, health))
}

/// Health probes are not agent commands and must never enter lane gain history.
fn rtk_probe_environment(environment: &Environment) -> Environment {
    Environment::new(
        environment
            .entries()
            .iter()
            .filter(|(name, _)| name != spice::rtk::HISTORY_VARIABLE)
            .cloned(),
    )
    .expect("filtering one valid environment entry preserves validity")
}

fn rtk_environment(environment: Environment, health: &spice::rtk::Health) -> Environment {
    if health.active() {
        return environment;
    }
    Environment::new(
        environment
            .entries()
            .iter()
            .filter(|(name, _)| {
                name != spice::rtk::EXECUTABLE_VARIABLE && name != spice::rtk::HISTORY_VARIABLE
            })
            .cloned(),
    )
    .expect("filtering valid environment entries preserves their validity")
}

#[cfg(test)]
mod rtk_environment_tests {
    use super::*;

    fn fixture() -> Environment {
        Environment::new([
            ("PATH", "/fixture/bin"),
            (spice::rtk::EXECUTABLE_VARIABLE, "fixture-rtk"),
            (spice::rtk::HISTORY_VARIABLE, "/fixture/history.db"),
        ])
        .expect("fixture environment is valid")
    }

    #[test]
    fn only_an_active_health_verdict_exports_rtk_to_the_lane() {
        let active_health = spice::rtk::Health::new(
            spice::rtk::HealthState::Active,
            Some("0.45.0".to_owned()),
            "fixture active",
        );
        let active = rtk_environment(fixture(), &active_health);
        assert!(
            active
                .entries()
                .iter()
                .any(|(name, _)| { name == spice::rtk::EXECUTABLE_VARIABLE })
        );

        let native_health = spice::rtk::Health::new(
            spice::rtk::HealthState::Obsolete,
            Some("0.44.9".to_owned()),
            "fixture native",
        );
        let native = rtk_environment(fixture(), &native_health);
        assert_eq!(native.entries(), [("PATH".into(), "/fixture/bin".into())]);
    }

    #[test]
    fn health_probe_cannot_write_the_lane_gain_database() {
        let probe = rtk_probe_environment(&fixture());
        assert!(
            probe
                .entries()
                .iter()
                .any(|(name, _)| name == spice::rtk::EXECUTABLE_VARIABLE)
        );
        assert!(
            probe
                .entries()
                .iter()
                .all(|(name, _)| name != spice::rtk::HISTORY_VARIABLE)
        );
    }
}

/// Ambient search path, optionally led by one configured wrapper directory.
fn shell_path(wrapper: Option<&Path>, executable: &Path) -> CommandResult<Option<OsString>> {
    let ambient = crate::bootstrap_environment("PATH");
    compose_shell_path(wrapper, executable.parent(), ambient.as_deref())
}

fn compose_shell_path(
    wrapper: Option<&Path>,
    executable_directory: Option<&Path>,
    ambient: Option<&std::ffi::OsStr>,
) -> CommandResult<Option<OsString>> {
    let mut entries = Vec::new();
    if let Some(command) = wrapper {
        let Some(directory) = command.parent() else {
            return Err(store_fault(
                "lane::wrapper-command-unavailable",
                "configured wrapper executable has no parent directory",
            ));
        };
        entries.push(directory.to_path_buf());
    }
    if let Some(ambient) = ambient {
        entries.extend(std::env::split_paths(ambient).filter(|entry| {
            wrapper.is_none() || executable_directory.is_none_or(|directory| entry != directory)
        }));
    }
    if entries.is_empty() {
        return Ok(None);
    }
    std::env::join_paths(entries)
        .map(Some)
        .map_err(|error| store_fault("lane::environment-invalid", &error.to_string()))
}

#[cfg(test)]
mod shell_path_tests {
    use super::*;

    #[test]
    fn wrapper_path_excludes_the_separately_carried_executable_directory() {
        let wrapper = Path::new("/fixture/wrappers/spice");
        let executable = Path::new("/fixture/bin/spice");
        let ambient = std::env::join_paths([
            Path::new("/fixture/bin"),
            Path::new("/usr/bin"),
            Path::new("/fixture/bin"),
        ])
        .expect("fixture search path");

        let composed = compose_shell_path(
            Some(wrapper),
            executable.parent(),
            Some(ambient.as_os_str()),
        )
        .expect("wrapper search path composes")
        .expect("wrapper supplies a search path");
        assert_eq!(
            std::env::split_paths(&composed).collect::<Vec<_>>(),
            [
                Path::new("/fixture/wrappers").to_path_buf(),
                Path::new("/usr/bin").to_path_buf(),
            ]
        );
    }

    #[test]
    fn absent_wrapper_preserves_the_ambient_search_path() {
        let ambient = std::env::join_paths([Path::new("/fixture/bin"), Path::new("/usr/bin")])
            .expect("fixture search path");

        let composed = compose_shell_path(
            None,
            Some(Path::new("/fixture/bin")),
            Some(ambient.as_os_str()),
        )
        .expect("ambient search path composes")
        .expect("ambient search path remains present");
        assert_eq!(composed, ambient);
    }
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
        Some(
            "tracked, worktree, and local inputs admitted by the configuration registry".to_owned(),
        ),
        Some(error.diagnostic()),
    )))
}

#[cfg(test)]
pub(in crate::resident) mod test_support;
#[cfg(test)]
mod tests;
