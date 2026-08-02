//! Occupant process launch and its configuration refusals.

use super::*;

pub(super) struct OccupantLaunch {
    pub(super) spec: Spec,
    pub(super) observation: Observation,
    pub(super) session: mpsc::Receiver<Result<wire::LaneSession, SessionProofError>>,
}

pub(super) struct OccupantLaunchRequest<'a> {
    pub(super) worktree: &'a str,
    pub(super) vendor: &'a wire::Name,
    pub(super) epoch: CanonicalUuid,
    pub(super) lane: CanonicalUuid,
    pub(super) occupant: CanonicalUuid,
    pub(super) resume: vendor::Resume,
}

pub(super) fn occupant_launch(
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
    let resolved = spice::config::load_repository(&repository, &[])
        .map_err(|error| configuration_refusal(&error))?;
    let plan = launch_plan(&repository, &resolved, name, lane, resume)?;
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
    let declaration_reporter: Arc<dyn CaptureReporter> = Arc::new(DeclarationReporter {
        inner: reporter,
        lane,
        occupant,
        worktree: repository.worktree_root().display().to_string(),
        vendor: name.clone(),
        epoch,
        stream,
        expected,
        state: Mutex::new(DeclarationState {
            reply: session_reply,
            seen: None,
        }),
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
        session,
    })
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
    name: &wire::Name,
    lane: CanonicalUuid,
    resume: vendor::Resume,
) -> CommandResult<spice::agent_launch::Plan> {
    let executable = std::env::current_exe()
        .map_err(|error| store_fault("lane::executable-unresolved", &error.to_string()))?
        .display()
        .to_string();
    let environment = launch_environment(name)?;
    spice::agent_launch::compose(
        resolved.root(),
        spice::agent_launch::Inputs {
            worktree: repository.worktree_root().to_path_buf(),
            vendor: name.clone(),
            prompt: "[$spice](.agents/skills/spice/SKILL.md)".to_owned(),
            executable,
            environment,
            lane,
            resume,
        },
    )
    .map_err(|error| match error {
        spice::agent_launch::ComposeError::UnsupportedVendor(other) => Outcome::Refusal(Refusal {
            reason: "lane::vendor-unsupported".to_owned(),
            witness: Some("lane.vendor".to_owned()),
            expected: Some("claude or codex".to_owned()),
            observed: Some(other),
            remedy: "select a vendor with a landed Rust launch dialect".to_owned(),
        }),
        other => store_fault("lane::launch-invalid", &other.to_string()),
    })
}

/// Carry only the ambient variables a supervised occupant is admitted to see.
fn launch_environment(name: &wire::Name) -> CommandResult<Environment> {
    let mut values = ["HOME", "PATH", "SHELL", "TMPDIR", "USER"]
        .into_iter()
        .filter_map(|key| crate::bootstrap_environment(key).map(|value| (key.into(), value)))
        .collect::<Vec<(OsString, OsString)>>();
    if name.as_str() == "claude"
        && let Some(value) =
            crate::bootstrap_environment(spice::agent_launch::CLAUDE_AUTO_COMPACT_WINDOW)
    {
        values.push((
            spice::agent_launch::CLAUDE_AUTO_COMPACT_WINDOW.into(),
            value,
        )); // env-policy: allow
    }
    Environment::new(values)
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
    Outcome::Refusal(Refusal {
        reason: "lane::configuration-refused".to_owned(),
        witness: Some("effective-configuration".to_owned()),
        expected: Some(
            "tracked and local inputs admitted by the configuration registry".to_owned(),
        ),
        observed: Some(error.diagnostic()),
        remedy: "correct the named configuration inputs before launching the lane".to_owned(),
    })
}
