//! Occupant process launch and its configuration refusals.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use sill::{
    Cancellation, Deadlines, Environment, Observation, RestartDisposition, Spec, WaitPolicy,
};

use super::*;

pub(in crate::resident) struct OccupantLaunch {
    pub(in crate::resident) spec: Spec,
    pub(in crate::resident) observation: Observation,
    pub(in crate::resident) session: SessionProof,
    pub(in crate::resident) git: String,
    /// Request bytes a command-driven vendor reads first, or nothing for a
    /// vendor driven entirely by its arguments.
    pub(in crate::resident) opening: Option<Vec<u8>>,
}

pub(in crate::resident) struct OccupantLaunchRequest<'a> {
    pub(in crate::resident) worktree: &'a str,
    pub(in crate::resident) vendor: &'a wire::Name,
    pub(in crate::resident) epoch: CanonicalUuid,
    pub(in crate::resident) lane: CanonicalUuid,
    pub(in crate::resident) occupant: CanonicalUuid,
    pub(in crate::resident) resume: vendor::Resume,
    pub(in crate::resident) fast_mode: bool,
    /// Model spelling this seat selected, or the vendor's configured model.
    pub(in crate::resident) model: Option<&'a str>,
    /// Effort spelling this seat selected, or the vendor's configured effort.
    pub(in crate::resident) effort: Option<&'a str>,
}

/// What one launch selects beyond the repository it runs in.
struct Selection<'a> {
    vendor: &'a wire::Name,
    lane: CanonicalUuid,
    pinned: Pinned,
    fast_mode: bool,
    model: Option<&'a str>,
    effort: Option<vendor::Effort>,
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
        fast_mode,
        model,
        effort,
    } = request;
    let effort = effort
        .map(str::parse)
        .transpose()
        .map_err(|error: vendor::EffortError| {
            store_fault("lane::effort-invalid", &error.to_string())
        })?;
    let repository = launch_repository(paths, worktree)?;
    // Run ahead of configuration load so the launch reads the advanced tree.
    // Keep this call at the live seam: `xtask/tests/git_boundaries.rs` proves
    // both that every launch reaches it and that its exact note is published.
    let git = spice::task_git::advance_at_launch(&repository).to_string();
    let resolved = spice::config::load_repository(&repository, &[])
        .map_err(|error| configuration_refusal(&error))?;
    let pinned = if name.as_str() == vendor::OMP.name() {
        pin_omp(
            paths,
            &repository,
            resolved.root().vendor().omp(),
            model,
            resume,
            lane,
            occupant,
        )?
    } else {
        Pinned {
            resume,
            config_overlay: None,
        }
    };
    let selection = Selection {
        vendor: name,
        lane,
        pinned,
        fast_mode,
        model,
        effort,
    };
    let (plan, rtk) = launch_plan(&repository, &resolved, selection)?;
    let model = plan.launch().model.clone();
    let effort = plan.launch().effort.as_str().to_owned();
    let record = plan
        .record()
        .map_err(|error| store_fault("lane::launch-invalid", &error.to_string()))?;
    let environment = Environment::new(
        plan.environment()
            .entries()
            .iter()
            .chain(record.environment())
            .cloned(),
    )
    .map_err(|error| store_fault("lane::environment-invalid", &error.to_string()))?;
    let process = launch_process(&repository, &resolved, environment, lane)?;
    let spec = record.spec(&process);
    let stream = record.stream();
    let expected = record.resume().session_id().map(str::to_owned);
    let opening = record.opening().map(<[u8]>::to_vec);
    let tape = tape::Tape::open(paths.tape.join(lane.to_string()).join(name.as_str()))
        .map_err(|error| store_fault("lane::capture-tape-unavailable", &error.to_string()))?;
    let (session_reply, session) = mpsc::sync_channel(4);
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
            horizon: session_proof_horizon(paths),
        },
        git,
        opening,
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
    repository: &Repository,
    resolved: &spice::config::Resolved,
    selection: Selection<'_>,
) -> CommandResult<(spice::agent_launch::Plan, wire::body::RtkHealth)> {
    let Selection {
        vendor: name,
        lane,
        pinned,
        fast_mode,
        model,
        effort,
    } = selection;
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
        repository,
        &executable,
        wrapper_command,
        resolved.root().rtk().executable(),
        lane,
    )?;
    let Some(vendor) = spice::extension::vendor(name.as_str()) else {
        return Err(vendor_unsupported(name.as_str().to_owned()));
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
            resume: pinned.resume,
            config_overlay: pinned.config_overlay,
            fast_mode,
            model: model.map(str::to_owned),
            effort,
        },
    )
    .map_err(|error| match error {
        spice::agent_launch::ComposeError::UnsupportedVendor(other) => vendor_unsupported(other),
        other => store_fault("lane::launch-invalid", &other.to_string()),
    })?;
    let rtk = rtk_hook_health(plan.vendor().dialect(), rtk);
    Ok((plan, rtk))
}

/// The refusal naming every vendor with a landed renderer beside the one asked
/// for.
fn vendor_unsupported(asked: String) -> Outcome<()> {
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
        Some(asked),
    )))
}

fn rtk_hook_health(dialect: listen::Dialect, health: spice::rtk::Health) -> spice::rtk::Health {
    if dialect != listen::Dialect::Codex {
        return health;
    }
    codex_rtk_hook_health(
        health,
        crate::steering_command::validate_codex_rtk_hook_contract(),
    )
}

fn codex_rtk_hook_health(
    health: spice::rtk::Health,
    contract: Result<(), String>,
) -> spice::rtk::Health {
    match contract {
        Ok(()) => health.with_hook(spice::rtk::HookState::Installed),
        Err(error) => spice::rtk::Health::new(
            spice::rtk::HealthState::ProtocolInvalid,
            health.version().map(str::to_owned),
            format!(
                "{}; Codex PreToolUse RTK response is not consumable: {error}",
                health.detail()
            ),
        ),
    }
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
    if bytes != ::std::include_bytes!("../../../agent/SKILL.md") {
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
/// for the wrapper, hook, and MCP commands. Only an absent wrapper falls back
/// to leading `PATH` with that already-validated executable's directory.
fn launch_environment(
    repository: &Repository,
    executable: &Path,
    wrapper_command: Option<&Path>,
    rtk_executable: &str,
    lane: CanonicalUuid,
) -> CommandResult<(Environment, wire::body::RtkHealth)> {
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
    values.extend(cargo_environment(repository, lane)?);
    values.extend([
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
    Ok((environment, health))
}

fn cargo_environment(
    repository: &Repository,
    lane: CanonicalUuid,
) -> CommandResult<Vec<(OsString, OsString)>> {
    let lock = repository.worktree_root().join("Cargo.lock");
    match fs::symlink_metadata(&lock) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => {
            return Err(store_fault(
                "lane::cargo-isolation-unavailable",
                &format!("{} is not a regular file", lock.display()),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(store_fault(
                "lane::cargo-isolation-unavailable",
                &format!("inspect {}: {error}", lock.display()),
            ));
        }
    }
    isolated_cargo_environment(repository, lane)
}

fn isolated_cargo_environment(
    repository: &Repository,
    lane: CanonicalUuid,
) -> CommandResult<Vec<(OsString, OsString)>> {
    let host = crate::cargo_command::host(repository)
        .map_err(|error| store_fault("lane::cargo-isolation-unavailable", &error.to_string()))?;
    // The lane-specific home is the resident reader pin: one occupied seat
    // admits one process group, and a replacement occupant cannot prepare the
    // same lane home until that seat is vacant. Manual Cargo children can
    // overlap a home advance, so their preparer additionally holds a kernel
    // pin for the exact child lifetime.
    let home = spice::cargo_isolation::prepare_lane(repository, lane, &host)
        .map_err(|error| store_fault("lane::cargo-isolation-unavailable", &error.to_string()))?;
    Ok(vec![
        (
            OsString::from(spice::cargo_isolation::CARGO_HOME_VARIABLE),
            home.home().as_os_str().to_owned(),
        ),
        (
            OsString::from(spice::cargo_isolation::BOOTSTRAP_HOME_VARIABLE),
            host.bootstrap_home().as_os_str().to_owned(),
        ),
        (
            OsString::from(spice::cargo_isolation::NATIVE_CARGO_VARIABLE),
            host.cargo().as_os_str().to_owned(),
        ),
        (
            OsString::from(spice::cargo_isolation::NATIVE_RUSTC_VARIABLE),
            host.rustc().as_os_str().to_owned(),
        ),
        (
            OsString::from(spice::cargo_isolation::NATIVE_RUSTDOC_VARIABLE),
            host.rustdoc().as_os_str().to_owned(),
        ),
        (
            OsString::from(spice::cargo_isolation::RUSTC_VARIABLE),
            host.rustc().as_os_str().to_owned(),
        ),
        (
            OsString::from(spice::cargo_isolation::RUSTDOC_VARIABLE),
            host.rustdoc().as_os_str().to_owned(),
        ),
    ])
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
    fn configured_rtk_remains_available_after_an_inactive_launch_probe() {
        let health = spice::rtk::Health::new(
            spice::rtk::HealthState::Missing,
            None,
            "fixture executable unavailable during launch",
        );
        assert!(!health.active());
        let environment = fixture();
        assert!(
            environment
                .entries()
                .iter()
                .any(|(name, _)| { name == spice::rtk::EXECUTABLE_VARIABLE })
        );
        assert!(
            environment
                .entries()
                .iter()
                .any(|(name, _)| name == spice::rtk::HISTORY_VARIABLE)
        );
    }

    #[test]
    fn codex_hook_installation_is_reported_separately_from_executable_health() {
        let launched_only = spice::rtk::Health::new(
            spice::rtk::HealthState::Active,
            Some("0.45.0".to_owned()),
            "fixture executable launched",
        );
        assert_eq!(launched_only.hook(), None);
        let installed = rtk_hook_health(listen::Dialect::Codex, launched_only);
        assert_eq!(installed.state(), spice::rtk::HealthState::Active);
        assert_eq!(installed.hook(), Some(spice::rtk::HookState::Installed));
        assert_eq!(
            serde_json::to_value(installed).expect("health encodes")["hook"],
            "installed"
        );
    }

    #[test]
    fn invalid_codex_response_contract_cannot_report_an_installed_hook() {
        let launched_only = spice::rtk::Health::new(
            spice::rtk::HealthState::Active,
            Some("0.45.0".to_owned()),
            "fixture executable launched",
        );
        let rejected = codex_rtk_hook_health(
            launched_only,
            Err("updatedInput lacks permissionDecision:allow".to_owned()),
        );

        assert_eq!(rejected.state(), spice::rtk::HealthState::ProtocolInvalid);
        assert_eq!(rejected.version(), Some("0.45.0"));
        assert_eq!(rejected.hook(), None);
        assert!(
            rejected
                .detail()
                .contains("updatedInput lacks permissionDecision:allow")
        );
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
    } else if let Some(directory) = executable_directory {
        entries.push(directory.to_path_buf());
    }
    if let Some(ambient) = ambient {
        entries.extend(
            std::env::split_paths(ambient)
                .filter(|entry| executable_directory.is_none_or(|directory| entry != directory)),
        );
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
    fn absent_wrapper_leads_with_the_validated_appliance_directory() {
        let ambient = std::env::join_paths([Path::new("/fixture/bin"), Path::new("/usr/bin")])
            .expect("fixture search path");

        let composed = compose_shell_path(
            None,
            Some(Path::new("/fixture/appliance")),
            Some(ambient.as_os_str()),
        )
        .expect("ambient search path composes")
        .expect("ambient search path remains present");
        assert_eq!(
            std::env::split_paths(&composed).collect::<Vec<_>>(),
            [
                Path::new("/fixture/appliance").to_path_buf(),
                Path::new("/fixture/bin").to_path_buf(),
                Path::new("/usr/bin").to_path_buf(),
            ]
        );
    }
}

/// What the lane fixed on disk before launch for a vendor that reads it back.
struct Pinned {
    resume: vendor::Resume,
    config_overlay: Option<PathBuf>,
}

/// Pin what an oh-my-pi occupant reads from disk before it launches: the
/// session it resumes and the overlay carrying its compaction threshold and
/// language-server disposition. The caller selects omp before entering.
fn pin_omp(
    paths: &RuntimePaths,
    repository: &Repository,
    omp: &vendor::config::OmpConfig,
    model: Option<&str>,
    resume: vendor::Resume,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> CommandResult<Pinned> {
    let resume = match resume.session_id() {
        Some(_) => resume,
        None => pin_session(repository)?,
    };
    let threshold = omp.auto_compact_threshold_tokens_for(model.unwrap_or_else(|| omp.model()));
    let config_overlay = pin_config_overlay(paths, threshold, omp.lsp(), lane, occupant)?;
    Ok(Pinned {
        resume,
        config_overlay: Some(config_overlay),
    })
}

/// Write this occupant's configuration overlay beside its supervisor record.
///
/// omp compacts at its own share of the model's window unless told a fixed
/// limit, and it takes that limit only from a settings file, so every launch
/// writes one stating the threshold the tracked policy derives for the model
/// this seat runs. The same file states whether this seat runs omp's
/// language-server integration, which omp otherwise decides for itself.
fn pin_config_overlay(
    paths: &RuntimePaths,
    threshold: u64,
    lsp: bool,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> CommandResult<PathBuf> {
    let path =
        crate::resident::supervisor::supervisor_directory(paths, lane, occupant).join("omp.yml");
    fs::create_dir_all(path.parent().expect("an overlay has a directory"))
        .and_then(|()| fs::write(&path, vendor::omp::config_overlay(threshold, lsp)))
        .map_err(|error| {
            store_fault(
                "lane::config-overlay-unavailable",
                &format!(
                    "oh-my-pi configuration overlay {} cannot be written: {error}",
                    path.display()
                ),
            )
        })?;
    Ok(path)
}

/// Pin a fresh oh-my-pi session before launch so its identity is known.
///
/// omp names a session only once it starts and exports nothing to its tools,
/// so the lane mints the identity, writes the session file omp resumes, and
/// launches by that identity.
fn pin_session(repository: &Repository) -> CommandResult<vendor::Resume> {
    let home = crate::bootstrap_environment("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| {
            store_fault(
                "lane::session-home-unavailable",
                "the supervised environment has no HOME to keep oh-my-pi sessions beneath",
            )
        })?;
    let temp =
        crate::bootstrap_environment("TMPDIR").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    let layout = vendor::OMP.transcript_layout();
    let sessions = home
        .join(layout.default_home)
        .join(layout.sessions_directory);
    let identity = CanonicalUuid::new(uuid::Uuid::now_v7()).to_string();
    let (path, contents) = vendor::omp::session_file(
        &sessions,
        &canonical(&home),
        &canonical(&temp),
        &canonical(repository.worktree_root()),
        &identity,
        &session_stamp(jiff::Timestamp::now()),
    );
    fs::create_dir_all(path.parent().expect("a session file has a directory"))
        .and_then(|()| fs::write(&path, contents))
        .map_err(|error| {
            store_fault(
                "lane::session-file-unavailable",
                &format!(
                    "oh-my-pi session {} cannot be written: {error}",
                    path.display()
                ),
            )
        })?;
    vendor::SessionId::new(identity)
        .map(vendor::Resume::Session)
        .map_err(|error| store_fault("lane::session-invalid", &error.to_string()))
}

/// The path as omp sees it: resolved through symlinks when that is possible.
fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The instant in omp's millisecond ISO-8601 spelling.
fn session_stamp(now: jiff::Timestamp) -> String {
    now.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// Bind occupant lifetime, wait policy, and capture bounds for this launch.
fn launch_process(
    repository: &Repository,
    resolved: &spice::config::Resolved,
    environment: Environment,
    lane: CanonicalUuid,
) -> CommandResult<vendor::Process> {
    let deadlines = Deadlines::new(resolved.root().lane().runtime().get(), OUTPUT_DRAIN, REAP)
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
