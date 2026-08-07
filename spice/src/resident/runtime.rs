#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimePaths {
    worktree: PathBuf,
    common: PathBuf,
    runtime: PathBuf,
    data: PathBuf,
    journal: PathBuf,
    resident: PathBuf,
    canvas: PathBuf,
    lane_canvas: PathBuf,
    inbox: PathBuf,
    degraded: PathBuf,
    tape: PathBuf,
    stage: PathBuf,
}

struct SurfaceConfig {
    canvas: canvas::Config,
    steering: steer::Config,
    taste: taste::Bag,
    consent: consent::Policy,
    speech: canvas::SpeechConfig,
    allocation: board::NextConfig,
    projects: board::ProjectPolicy,
}

struct SurfaceLaunch<'a> {
    binding: CanvasBinding,
    /// Whether the operator asked this start to also open the app.
    ///
    /// Discovery is announced either way, so this is not part of the binding:
    /// it decides who dials the published address, not which address gets
    /// published or what admits a caller to it.
    open: bool,
    progress: &'a mut dyn io::Write,
    endpoint: &'a mut dyn io::Write,
}

// Runtime policy and trust identity are projections of this one captured generation.
struct StartupSurface {
    config: SurfaceConfig,
    trust: crate::trust_command::Surface,
}

fn load_startup_surface(
    repository: &Repository,
) -> Result<StartupSurface, spice::config::LoadError> {
    load_startup_surface_with(repository, || {})
}

fn load_startup_surface_with<F>(
    repository: &Repository,
    after_capture: F,
) -> Result<StartupSurface, spice::config::LoadError>
where
    F: FnOnce(),
{
    let loaded = spice::config::capture_repository(repository, &[])?;
    after_capture();
    Ok(StartupSurface {
        config: SurfaceConfig {
            canvas: *loaded.resolved().root().canvas(),
            steering: loaded.resolved().root().steer().clone(),
            taste: loaded.resolved().root().taste().bag().clone(),
            consent: loaded.resolved().root().consent().policy().clone(),
            speech: loaded.resolved().root().say().clone(),
            allocation: *loaded.resolved().root().board().next(),
            projects: loaded.resolved().root().tasks().project_policy(),
        },
        trust: crate::trust_command::Surface::from_loaded(&loaded),
    })
}

fn require_current_surface(
    repository: &Repository,
    surface: &crate::trust_command::Surface,
) -> Result<(), GuardError> {
    match surface.is_current(repository) {
        Ok(true) => Ok(()),
        Ok(false) | Err(_) => Err(GuardError::refused("trust::surface-moved")),
    }
}

impl RuntimePaths {
    #[cfg(test)]
    fn discover(start: &Path) -> CommandResult<Self> {
        let repository = Repository::discover(start)
            .map_err(|error| environment_fault("resident::git-unavailable", &error.to_string()))?;
        Self::from_repository(&repository)
    }

    fn from_repository(repository: &Repository) -> CommandResult<Self> {
        let state = repository
            .admit_state()
            .map_err(|error| crate::state_failure(&error).outcome())?;
        Ok(Self {
            worktree: repository.worktree_root().to_path_buf(),
            common: repository.common_dir().to_path_buf(),
            runtime: state.runtime().to_path_buf(),
            data: state.data().to_path_buf(),
            journal: state.journal().to_path_buf(),
            resident: state.resident().to_path_buf(),
            canvas: state.canvas().to_path_buf(),
            lane_canvas: state.lane_canvas().to_path_buf(),
            inbox: state.inbox().to_path_buf(),
            degraded: state.degraded().to_path_buf(),
            tape: state.tape().to_path_buf(),
            stage: repository.worktree_root().join("dist/stage"),
        })
    }

    fn clear_stale_surfaces(&self) -> CommandResult<()> {
        remove_if_present(&self.canvas)
            .map_err(|error| environment_fault("canvas::stale-discovery", &error.to_string()))?;
        remove_tree_if_present(&self.lane_canvas).map_err(|error| {
            environment_fault("canvas::stale-lane-discovery", &error.to_string())
        })
    }
}

#[derive(Debug)]
struct ResidentLock {
    possession: Possession,
}

impl ResidentLock {
    fn acquire(paths: &RuntimePaths) -> CommandResult<Self> {
        Self::acquire_with(paths, || {})
    }

    fn acquire_with(paths: &RuntimePaths, after_contention: impl FnMut()) -> CommandResult<Self> {
        fs::create_dir_all(&paths.runtime).map_err(|error| {
            environment_fault("resident::runtime-unavailable", &error.to_string())
        })?;
        let name = LockName::new("resident").expect("resident is a non-empty lock name");
        let lock = LockFile::new(name, &paths.resident);
        match lock
            .try_acquire_with(after_contention)
            .map_err(|error| environment_fault("resident::lock-unavailable", &error.to_string()))?
        {
            Acquisition::Acquired(possession) => {
                possession
                    .rewrite_record(holder_record(std::process::id(), None).as_bytes())
                    .map_err(|error| {
                        environment_fault("resident::holder-unwritable", &error.to_string())
                    })?;
                Ok(Self { possession })
            }
            Acquisition::Contended => {
                // Holder rewrites never publish an empty intermediate state,
                // so a contender arriving during the pending-to-epoch change
                // can still name the holder instead of taking this fallback.
                let observed = fs::read_to_string(&paths.resident)
                    .ok()
                    .map(|holder| holder.trim().to_owned())
                    .filter(|holder| !holder.is_empty())
                    .unwrap_or_else(|| "holder metadata unavailable".to_owned());
                Err(Outcome::Refusal(Refusal {
                    reason: "resident::already-up".to_owned(),
                    witness: Some("resident-lock".to_owned()),
                    expected: Some("unheld".to_owned()),
                    observed: Some(observed),
                    remedy: "address the named holder or use spice down for its current epoch"
                        .to_owned(),
                }))
            }
        }
    }

    fn set_epoch(&self, epoch: CanonicalUuid) -> io::Result<()> {
        self.possession
            .rewrite_record(holder_record(std::process::id(), Some(epoch)).as_bytes())
    }
}

impl Drop for ResidentLock {
    fn drop(&mut self) {
        let _ = self.possession.file().set_len(0);
        let _ = self.possession.file().sync_data();
    }
}

#[derive(Debug)]
struct PublishedDiscovery {
    path: PathBuf,
    lane_root: PathBuf,
}

impl Drop for PublishedDiscovery {
    fn drop(&mut self) {
        let _ = remove_if_present(&self.path);
        let _ = remove_tree_if_present(&self.lane_root);
    }
}

/// The address the surface listens on when the operator names neither flag.
///
/// The port is fixed rather than ephemeral because the operator's browser holds
/// the URL: a port the kernel picks is a different address after every restart,
/// which invalidates every bookmark and open tab. Loopback stays the default
/// because reaching this surface from another machine is a decision only the
/// operator can make, and `--host` is how they make it.
///
/// This port is not the only system that picked 8765. The Python predecessor
/// serves on `*:8765` and `*:8766` and holds them for as long as it runs, so on
/// a machine running both, this default cannot bind. Nothing in either system
/// resolves that: fixing the port is the whole point above, so there is no
/// fallback to resolve it with. Lanes step around it by naming `--port` once per
/// invocation, by hand. That is the standing arrangement between two systems
/// that both keep running -- not a rough edge on the way to one replacing the
/// other -- and no check records that the workaround is required.
const DEFAULT_LISTEN: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8765);
/// Bounds on an operator-supplied bearer token for a non-loopback bind.
///
/// Stated rather than inlined so the refusal text and the check cannot come to
/// name two different lengths.
const AUTH_TOKEN_MIN_BYTES: usize = 16;
const AUTH_TOKEN_MAX_BYTES: usize = 256;

/// The address a caller that owns no operator intent should listen on.
///
/// A demo is not the operator's surface and must not take the port theirs
/// answers on, so it asks the kernel for one nobody is using.
pub(crate) const EPHEMERAL_LOOPBACK: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);

pub(crate) struct CanvasBinding {
    listen: SocketAddr,
    access: CanvasAccess,
}

struct CanvasAccess {
    policy: canvas::axum::Access,
    token: Option<String>,
}

impl CanvasBinding {
    pub(crate) fn local(listen: SocketAddr) -> Self {
        Self {
            listen,
            access: CanvasAccess {
                policy: canvas::axum::Access::local(),
                token: None,
            },
        }
    }
}

pub(crate) fn up(
    leaf: &clap::ArgMatches,
    progress: &mut dyn io::Write,
    endpoint: &mut dyn io::Write,
) -> Outcome<()> {
    let listen = match requested_listen(leaf) {
        Ok(listen) => listen,
        Err(outcome) => return outcome,
    };
    let access = match requested_access(leaf, listen) {
        Ok(access) => access,
        Err(outcome) => return outcome,
    };
    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => return environment_fault("resident::cwd-unavailable", &error.to_string()),
    };
    up_from_with(
        &current,
        CanvasBinding { listen, access },
        leaf.get_flag("open"),
        progress,
        endpoint,
    )
}

/// The address `--host` and `--port` asked for, each defaulted on its own.
///
/// Both are parsed strictly: what the operator named is what gets bound, so a
/// host that is not an address of this machine has to fail here by name rather
/// than resolve to something they did not ask for.
fn requested_listen(leaf: &clap::ArgMatches) -> CommandResult<SocketAddr> {
    let host = match leaf.get_one::<String>("host") {
        Some(host) => host.parse::<IpAddr>().map_err(|_| {
            listen_refusal("resident::host-invalid", "one IP address to listen on", host)
        })?,
        None => DEFAULT_LISTEN.ip(),
    };
    let port = match leaf.get_one::<String>("port") {
        Some(port) => port.parse::<u16>().map_err(|_| {
            listen_refusal("resident::port-invalid", "one port from 0 to 65535", port)
        })?,
        None => DEFAULT_LISTEN.port(),
    };
    Ok(SocketAddr::new(host, port))
}

fn requested_access(
    leaf: &clap::ArgMatches,
    listen: SocketAddr,
) -> CommandResult<CanvasAccess> {
    let token = leaf.get_one::<String>("auth-token").cloned();
    if let Some(token) = token.as_deref()
        && !auth_token_is_valid(token)
    {
        return Err(access_refusal(
            "resident::auth-token-invalid",
            None,
            &format!(
                "use {AUTH_TOKEN_MIN_BYTES} to {AUTH_TOKEN_MAX_BYTES} URL-safe \
                 characters: letters, digits, hyphen, period, underscore, or tilde"
            ),
        ));
    }
    if !listen.ip().is_loopback() && !leaf.get_flag("allow-insecure-bind") {
        return Err(access_refusal(
            "resident::insecure-bind-consent-required",
            Some(&listen.to_string()),
            "add --allow-insecure-bind only when this network exposure is intended",
        ));
    }
    if !listen.ip().is_loopback() && token.is_none() {
        return Err(access_refusal(
            "resident::auth-token-required",
            Some(&listen.to_string()),
            "add --auth-token TOKEN and open the remote URL with #TOKEN",
        ));
    }
    let policy = token.as_ref().map_or_else(canvas::axum::Access::local, |token| {
        canvas::axum::Access::authenticated(token.clone())
    });
    Ok(CanvasAccess { policy, token })
}

fn auth_token_is_valid(token: &str) -> bool {
    (AUTH_TOKEN_MIN_BYTES..=AUTH_TOKEN_MAX_BYTES).contains(&token.len())
        && token.bytes().all(is_token_byte)
}

const fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

fn access_refusal(reason: &str, observed: Option<&str>, remedy: &str) -> Outcome<()> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: Some("requested-canvas-access".to_owned()),
        expected: Some("loopback, or an explicitly consented authenticated bind".to_owned()),
        observed: observed.map(str::to_owned),
        remedy: remedy.to_owned(),
    })
}

fn listen_refusal(reason: &str, expected: &str, observed: &str) -> Outcome<()> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: Some("requested-listen-address".to_owned()),
        expected: Some(expected.to_owned()),
        observed: Some(observed.to_owned()),
        remedy: format!(
            "name an address this machine can listen on, or omit the flag for {DEFAULT_LISTEN}"
        ),
    })
}

pub(crate) fn up_from(current: &Path, binding: CanvasBinding) -> Outcome<()> {
    up_from_with(
        current,
        binding,
        false,
        &mut io::sink(),
        &mut io::sink(),
    )
}

fn up_from_with(
    current: &Path,
    binding: CanvasBinding,
    open: bool,
    progress: &mut dyn io::Write,
    endpoint: &mut dyn io::Write,
) -> Outcome<()> {
    let repository = match Repository::discover(current) {
        Ok(repository) => repository,
        Err(error) => return environment_fault("resident::git-unavailable", &error.to_string()),
    };
    let paths = match RuntimePaths::from_repository(&repository) {
        Ok(paths) => paths,
        Err(outcome) => return outcome,
    };
    if let Err(error) = fs::create_dir_all(&paths.runtime) {
        return environment_fault("resident::runtime-unavailable", &error.to_string());
    }
    let startup_surface = match load_startup_surface(&repository) {
        Ok(surface) => surface,
        Err(error) => return configuration_refusal(&error),
    };
    if let Err(error) = fs::create_dir_all(&paths.data) {
        return environment_fault("resident::data-unavailable", &error.to_string());
    }
    let resident_lock = match ResidentLock::acquire(&paths) {
        Ok(lock) => lock,
        Err(outcome) => return outcome,
    };
    if let Err(outcome) = paths.clear_stale_surfaces() {
        return outcome;
    }

    let journal = match journal::Journal::open(&paths.journal) {
        Ok(journal) => journal,
        Err(error) => return journal_open_failure(error),
    };
    let migration = journal.migration();
    let epoch = new_uuid();
    if let Err(error) = resident_lock.set_epoch(epoch) {
        return environment_fault("resident::holder-unwritable", &error.to_string());
    }
    let bus = match Bus::start(journal) {
        Ok(bus) => bus,
        Err(error) => return store_fault("bus::start-failed", &error.to_string()),
    };
    let startup = match commit_startup(
        &bus,
        &repository,
        &startup_surface.trust,
        epoch,
        migration,
    ) {
        Ok(startup) => startup,
        Err(outcome) => return outcome,
    };

    let mut seats = Seats::new();
    let surface = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(run_surface(
            &bus,
            &repository,
            &paths,
            epoch,
            &startup_surface.config,
            &mut seats,
            SurfaceLaunch {
                binding,
                open,
                progress,
                endpoint,
            },
        )),
        Err(error) => Err(environment_fault(
            "resident::runtime-failed",
            &error.to_string(),
        )),
    };
    let answer = settle_surface(&bus, &paths, epoch, startup, &mut seats, surface);
    let shutdown = bus.shutdown();
    drop(resident_lock);
    if let Err(error) = shutdown {
        return store_fault("bus::shutdown-failed", &error.to_string());
    }
    answer
}

fn journal_open_failure(error: journal::OpenError) -> Outcome<()> {
    match error {
        journal::OpenError::Migration(migration) => Outcome::Refusal(Refusal {
            reason: "journal::schema-refused".to_owned(),
            witness: None,
            expected: Some(format!("Rust journal schema {}", journal::SCHEMA_VERSION,)),
            observed: Some(migration.to_string()),
            remedy: concat!(
                "finish or replace the unfinalized or incompatible Rust authority ",
                "before starting the resident"
            )
            .to_owned(),
        }),
        error => store_fault("journal::open-failed", &error.to_string()),
    }
}

fn configuration_refusal(error: &spice::config::LoadError) -> Outcome<()> {
    Outcome::Refusal(Refusal {
        reason: "resident::configuration-refused".to_owned(),
        witness: Some("effective-configuration".to_owned()),
        expected: Some("one admitted effective repository configuration".to_owned()),
        observed: Some(error.diagnostic()),
        remedy: "repair tracked or local configuration before starting the resident".to_owned(),
    })
}

fn settle_surface(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    startup: StartupCommit,
    seats: &mut Seats,
    surface: CommandResult<()>,
) -> Outcome<()> {
    match surface {
        Ok(()) => committed_reply(startup.first, startup.last),
        Err(original) => match drive_shutdown(bus, paths, epoch, seats) {
            Outcome::Reply(_) => original,
            compensation => store_fault(
                "resident::compensation-failed",
                &format!("startup failed as {original:?}; compensation failed as {compensation:?}"),
            ),
        },
    }
}

pub(crate) fn down(contract: &crate::cli::Contract) -> Outcome<()> {
    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => return environment_fault("resident::cwd-unavailable", &error.to_string()),
    };
    down_from(&current, contract)
}

pub(crate) fn down_from(current: &Path, contract: &crate::cli::Contract) -> Outcome<()> {
    let repository = match Repository::discover(current) {
        Ok(repository) => repository,
        Err(error) => return environment_fault("resident::git-unavailable", &error.to_string()),
    };
    let paths = match RuntimePaths::from_repository(&repository) {
        Ok(paths) => paths,
        Err(outcome) => return outcome,
    };
    let outcome = crate::command_client::execute(
        &paths.canvas,
        contract,
        crate::command_client::SelectedCommand::Down,
    );
    let outcome = decode_dark_outcome(outcome);
    if matches!(outcome, Outcome::Reply(_))
        && let Err(release) = wait_for_release(&paths)
    {
        return release;
    }
    outcome
}

fn wait_for_release(paths: &RuntimePaths) -> CommandResult<()> {
    let possession = acquire_released_resident(paths)?;
    if paths.canvas.exists() {
        return Err(environment_fault(
            "canvas::release-incomplete",
            "resident lock was released while discovery remained published",
        ));
    }
    drop(possession);
    Ok(())
}

/// Wait one client bound for the resident to surrender its runtime lock.
fn acquire_released_resident(paths: &RuntimePaths) -> CommandResult<Possession> {
    const RELEASE_BOUND: std::time::Duration = crate::cli::answer_bound(CommandKind::UpChange);

    let name = LockName::new("resident").expect("resident is a non-empty lock name");
    let lock = LockFile::new(name, &paths.resident);
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("spice-release-wait".to_owned())
        .spawn(move || {
            let _ = sender.send(lock.acquire());
        })
        .map_err(|error| environment_fault("resident::release-failed", &error.to_string()))?;

    match receiver.recv_timeout(RELEASE_BOUND) {
        Ok(Ok(possession)) => Ok(possession),
        Ok(Err(error)) => Err(environment_fault(
            "resident::release-failed",
            &error.to_string(),
        )),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(environment_fault(
            "canvas::deadline-elapsed",
            &format!(
                "resident did not release within the {RELEASE_BOUND:?} client bound"
            ),
        )),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(environment_fault(
            "resident::release-failed",
            "resident release waiter ended without reporting lock acquisition",
        )),
    }
}

#[derive(Clone, Copy, Debug)]
struct StartupCommit {
    first: u64,
    last: u64,
}

/// Preserve current typed subscriptions or migrate one legacy all-lane head.
///
/// Legacy heads are recognizable because their subscription table is wholly
/// absent and therefore decodes empty. Once any typed row exists, the table is
/// current and must match the lane lineages exactly; partial or duplicate
/// authority is refused instead of repaired through a second path.
pub(super) fn startup_subscriptions(
    lanes: &[wire::LaneResidency],
    stored: Vec<wire::LineageSubscription>,
) -> CommandResult<Vec<wire::LineageSubscription>> {
    let lineage_ids = lanes
        .iter()
        .map(|row| row.lineage_id)
        .collect::<std::collections::BTreeSet<_>>();
    if stored.is_empty() {
        return lineage_ids
            .into_iter()
            .map(|lineage_id| {
                let row = lanes
                    .iter()
                    .find(|row| row.lineage_id == lineage_id)
                    .expect("a collected lineage has one lane row");
                if lanes
                    .iter()
                    .filter(|candidate| candidate.lineage_id == lineage_id)
                    .any(|candidate| candidate.life != row.life)
                {
                    return Err(store_fault(
                        "lineage::subscription-ambiguous",
                        concat!(
                            "a legacy lineage has mixed lane lives and no authoritative ",
                            "task-subscription policy"
                        ),
                    ));
                }
                let policy = wire::intent::LineageLife::from_wire(&row.life).ok_or_else(|| {
                    store_fault(
                        "lineage::subscription-invalid",
                        "a legacy lineage lane has no closed task-subscription policy",
                    )
                })?;
                Ok(wire::LineageSubscription {
                    lineage_id,
                    policy,
                    manual_projects: Vec::new(),
                    auto_projects: Vec::new(),
                })
            })
            .collect();
    }

    let mut indexed = std::collections::BTreeMap::new();
    for mut subscription in stored {
        subscription.auto_projects.clear();
        if indexed
            .insert(subscription.lineage_id, subscription)
            .is_some()
        {
            return Err(store_fault(
                "lineage::subscription-ambiguous",
                "one lineage has more than one durable task-subscription policy",
            ));
        }
    }
    if indexed.keys().copied().collect::<std::collections::BTreeSet<_>>() != lineage_ids {
        return Err(store_fault(
            "lineage::subscription-misaligned",
            "the durable task-subscription table does not match the resident lineages",
        ));
    }
    Ok(indexed.into_values().collect())
}

fn commit_startup(
    bus: &Bus,
    repository: &Repository,
    surface: &crate::trust_command::Surface,
    epoch: CanonicalUuid,
    migration: Option<journal::Migration>,
) -> CommandResult<StartupCommit> {
    commit_startup_with(bus, repository, surface, epoch, migration, || {})
}

fn commit_startup_with<F>(
    bus: &Bus,
    repository: &Repository,
    surface: &crate::trust_command::Surface,
    epoch: CanonicalUuid,
    migration: Option<journal::Migration>,
    before_seating: F,
) -> CommandResult<StartupCommit>
where
    F: FnOnce(),
{
    let (lanes, subscriptions) = bus
        .residency_head()
        .map_err(|error| store_fault("resident::head-unreadable", &error.to_string()))?
        .map_or_else(
            || Ok((Vec::new(), Vec::new())),
            |head| {
                let head = head.into_body();
                let subscriptions =
                    startup_subscriptions(&head.lanes, head.subscriptions)?;
                Ok((head.lanes, subscriptions))
            },
        )?;
    let mut first = None;
    let mut cause = FactCause::Root;
    if let Some(migration) = migration {
        let mut write = write(Actor::Spice, epoch, Producer::ResidentRecognition)?;
        write
            .fact(
                wire::body::JournalMigrated {
                    from: migration.from(),
                    to: migration.to(),
                    epoch,
                },
                cause,
            )
            .map_err(|error| store_fault("journal::write-invalid", &error.to_string()))?;
        let committed = commit(bus, write)?;
        first = Some(committed.first());
        cause = FactCause::Existing(seq(committed.last()));
    }

    let trust = bus
        .trust_head(surface.digest())
        .map_err(|error| store_fault("trust::head-unreadable", &error.to_string()))?;
    if trust.is_none() {
        let snapshot = bus
            .fact_snapshot()
            .map_err(|error| store_fault("trust::journal-unreadable", &error.to_string()))?;
        let previous = latest_surface_digest(&snapshot)?;
        let mut pending = write(Actor::Spice, epoch, Producer::CommandDoor)?;
        let guarded_repository = repository.clone();
        let guarded_surface = surface.clone();
        pending.guard(JournalGuard::new(move |read| {
            require_current_surface(&guarded_repository, &guarded_surface)?;
            if read.trust_head(guarded_surface.digest())?.is_some() {
                return Err(GuardError::refused("trust::digest-already-pended"));
            }
            Ok(())
        }));
        let digest = surface.digest().clone();
        let pended = pending
            .fact(
                wire::body::TrustPended {
                    surface: crate::trust_command::SURFACE_NAME.to_owned(),
                    digest: digest.clone(),
                    previous,
                },
                cause,
            )
            .map_err(|error| store_fault("trust::fact-invalid", &error.to_string()))?;
        pending.trust_head(pended, move |_| TrustHead {
            digest,
            state: TrustState::Pended,
        });
        let committed = commit(bus, pending)?;
        first.get_or_insert_with(|| committed.first());
        cause = FactCause::Existing(seq(committed.last()));
    }

    before_seating();
    let mut write = write(Actor::Spice, epoch, Producer::CommandDoor)?;
    let guarded_repository = repository.clone();
    let guarded_surface = surface.clone();
    write.guard(JournalGuard::new(move |_| {
        require_current_surface(&guarded_repository, &guarded_surface)
    }));
    let seated = write
        .fact(wire::body::UpSeated { epoch }, cause)
        .map_err(|error| store_fault("journal::write-invalid", &error.to_string()))?;
    write.residency_head(seated, move |_| ResidencyHead {
        epoch,
        state: ResidentState::Seated,
        lanes,
        subscriptions,
    });
    let committed = commit(bus, write)?;
    Ok(StartupCommit {
        first: first.unwrap_or_else(|| committed.first()),
        last: committed.last(),
    })
}

fn latest_surface_digest(snapshot: &journal::FactSnapshot) -> CommandResult<Option<wire::Digest>> {
    for envelope in snapshot
        .facts()
        .iter()
        .rev()
        .filter(|envelope| envelope.fact.kind() == wire::FactKind::TrustPended)
    {
        let bytes = wire::encode(&envelope.fact)
            .map_err(|error| store_fault("trust::fact-invalid", &error.to_string()))?;
        let fact = wire::decode::<wire::Fact<wire::body::TrustPended>>(&bytes)
            .map_err(|error| store_fault("trust::fact-invalid", &error.to_string()))?;
        if fact.body.surface == crate::trust_command::SURFACE_NAME {
            return Ok(Some(fact.body.digest));
        }
    }
    Ok(None)
}

async fn run_surface(
    bus: &Bus,
    repository: &Repository,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    config: &SurfaceConfig,
    seats: &mut Seats,
    launch: SurfaceLaunch<'_>,
) -> CommandResult<()> {
    let operator = new_uuid();
    let residency = current_residency(bus, epoch)?;
    let capabilities = surface_capabilities(operator, residency.body())?;
    let timer_commits = bus.reader().subscribe();
    let timers = timers::Runtime::start(config.projects.clone())?;
    // Settled before the listener exists, so a app stage that cannot supply
    // the whole app stops the resident with its own name rather than turning
    // into a page the operator has to diagnose from the browser.
    let app = canvas::AppSource::resolve(&paths.worktree)
        .map_err(|error| environment_fault("canvas::app-source-invalid", &error.to_string()))?;
    let app_description = app.to_string();
    let SurfaceLaunch {
        binding,
        open,
        progress,
        endpoint,
    } = launch;
    let CanvasBinding { listen, access } = binding;
    let CanvasAccess { policy, token } = access;
    let (server, control) = canvas::axum::Server::bind_streaming(
        canvas::axum::Binding::new(listen, policy),
        epoch,
        capabilities.clone(),
        app,
        config.speech.clone(),
        bus.reader(),
        config.canvas.rows(),
    )
    .await
    // Naming the address is the whole diagnosis now that one can be asked for:
    // the common failure is a port another process already holds, and "address
    // already in use" alone does not say which address or how to pick another.
    .map_err(|error| {
        environment_fault(
            "canvas::bind-failed",
            &format!("listening on {listen} failed: {error}"),
        )
    })?;
    let discovery = server
        .discovery(operator)
        .map_err(|error| environment_fault("canvas::discovery-invalid", &error.to_string()))?;
    let message_publisher = server.message_publisher().ok_or_else(|| {
        environment_fault(
            "canvas::message-surface-unavailable",
            "the streaming canvas server has no transcript activity publisher",
        )
    })?;
    let published = PublishedDiscovery {
        path: paths.canvas.clone(),
        lane_root: paths.lane_canvas.clone(),
    };
    steering::publish_lane_discoveries(paths, &discovery, residency.body())?;
    steering::publish_inboxes(bus, paths, residency.body())?;
    discovery
        .publish(&paths.canvas)
        .map_err(|error| environment_fault("canvas::publish-failed", &error.to_string()))?;
    let (shutdown, stopped) = oneshot::channel();
    let server_task = tokio::spawn(server.up_until(async move {
        let _ = stopped.await;
    }));
    let _ = writeln!(progress, "canvas app source: {app_description}");
    announce_canvas(
        &paths.canvas,
        token.as_deref(),
        open,
        || {},
        crate::command_client::system_browser,
        endpoint,
        progress,
    );
    let _ = endpoint.flush();
    let _ = progress.flush();
    let capture::Channels {
        reporter,
        records,
        failures,
    } = capture::channels();
    lane_surface::restore_seats(bus, paths, epoch, seats, &reporter)?;
    // Tapes kept growing while this resident was down. Accounting what they
    // gained before the surface is driven means the first live record continues
    // a head that already stands past everything the outage missed.
    ingest_transcripts(bus, paths, None, progress)?;
    let command_surface = command_surface::Surface::new(
        bus,
        repository,
        paths,
        epoch,
        lane_surface::Runtime::new(&capabilities, &discovery, reporter.clone()),
        lane_recovery::Runtime::new(reporter),
        config,
    );
    drive_surface(
        bus,
        epoch,
        seats,
        &command_surface,
        control,
        published,
        shutdown,
        server_task,
        message_publisher,
        records,
        failures,
        progress,
        timers.drive(bus, timer_commits),
    )
    .await
}

/// Announce the app this start serves, and dial it when asked to.
///
/// The endpoint is read back out of the discovery file the resident has just
/// published rather than formatted from the address it bound, so the line an
/// operator is told to open has already passed the ownership, loopback-origin,
/// and generation admission that a client applies to the same file. That is
/// the whole reason there is no separate opener command: one reader admits the
/// endpoint, and `up` is simply its first caller.
///
/// Authenticated access decorates that admitted endpoint only after admission.
/// The token stays in the URL fragment for announcement and browser launch, so
/// it neither changes the discovery origin nor crosses in the browser's HTTP
/// origin.
///
/// A resident whose own discovery fails that admission says so and keeps
/// serving. The surface is up either way, and refusing to run because the
/// operator cannot be handed a URL would trade a working appliance for a
/// cosmetic one.
///
/// Only an endpoint reaches the endpoint stream; a failure to produce one is a
/// diagnostic and goes where the other diagnostics went. A shell that reads the
/// endpoint stream to dial it cannot tell a URL from a sentence explaining why
/// there is not one, so it sees an empty capture instead.
fn announce_canvas(
    discovery_path: &Path,
    auth_token: Option<&str>,
    open: bool,
    before_current_check: impl FnOnce(),
    browser: impl FnOnce(&str) -> Result<(), String>,
    announcement: &mut dyn io::Write,
    diagnostic: &mut dyn io::Write,
) {
    let result = crate::command_client::open_canvas_result_with(
        discovery_path,
        before_current_check,
        |endpoint| {
            if open {
                browser(&canvas_opening(endpoint, auth_token))
            } else {
                Ok(())
            }
        },
    );
    let _ = match result {
        Ok(opened) => writeln!(
            announcement,
            "canvas endpoint: {}",
            canvas_opening(&opened.endpoint, auth_token)
        ),
        Err(fault) => writeln!(diagnostic, "canvas unavailable: {}", fault.reason),
    };
}

fn canvas_opening(endpoint: &str, auth_token: Option<&str>) -> String {
    auth_token.map_or_else(|| endpoint.to_owned(), |token| format!("{endpoint}#{token}"))
}

fn surface_capabilities(
    operator: CanonicalUuid,
    residency: &ResidencyHead,
) -> CommandResult<canvas::axum::Capabilities> {
    let capabilities = canvas::axum::Capabilities::default();
    capabilities
        .insert(operator, Actor::Operator)
        .map_err(|error| environment_fault("canvas::capability-invalid", &error.to_string()))?;
    for lane in &residency.lanes {
        for capability in &lane.capabilities {
            capabilities
                .insert(*capability, Actor::Lane(lane.lane))
                .map_err(|error| {
                    environment_fault("canvas::capability-invalid", &error.to_string())
                })?;
        }
    }
    Ok(capabilities)
}

#[allow(
    clippy::too_many_arguments,
    reason = "the resident loop owns one receiver for each closed event source"
)]
async fn drive_surface(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command_surface: &command_surface::Surface<'_>,
    mut control: canvas::axum::Control,
    published: PublishedDiscovery,
    shutdown: oneshot::Sender<()>,
    mut server_task: tokio::task::JoinHandle<Result<(), io::Error>>,
    message_publisher: canvas::stream::MessagePublisher,
    mut records: tokio::sync::mpsc::Receiver<lane::capture::CaptureRecord>,
    mut failures: tokio::sync::mpsc::UnboundedReceiver<lane::capture::CaptureFailure>,
    diagnostic: &mut dyn io::Write,
    timer: impl std::future::Future<Output = CommandResult<()>>,
) -> CommandResult<()> {
    tokio::pin!(timer);
    // One resident owns one correlation table: a tool result names its tool only
    // by the call it answers, and calls outlive the record that announced them.
    let mut tool_previews = CallPreviews::default();
    loop {
        tokio::select! {
            request = control.recv() => {
                if dispatch_control(command_surface, seats, &mut control, request).await? {
                    drop(published);
                    let _ = shutdown.send(());
                    return match server_task.await {
                        Ok(Ok(())) => Ok(()),
                        ending => Err(server_outcome(ending)),
                    };
                }
            }
            record = records.recv() => {
                let record = record.ok_or_else(|| environment_fault(
                    "lane::capture-records-closed",
                    "lane capture record receiver closed",
                ))?;
                accept_capture_record(
                    bus,
                    epoch,
                    seats,
                    &CaptureSurfaces::new(
                        command_surface.paths,
                        &command_surface.config.taste,
                        &command_surface.config.steering,
                        &message_publisher,
                    ),
                    &record,
                    &mut tool_previews,
                    diagnostic,
                )?;
            }
            failure = failures.recv() => {
                let failure = failure.ok_or_else(|| environment_fault(
                    "lane::capture-failures-closed",
                    "lane capture failure receiver closed",
                ))?;
                lane_surface::capture_failed(bus, epoch, seats, &failure)?;
            }
            timed = &mut timer => return timed,
            ending = &mut server_task => {
                return Err(server_outcome(ending));
            }
        }
    }
}

async fn dispatch_control(
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    control: &mut canvas::axum::Control,
    request: Option<canvas::axum::ControlRequest>,
) -> CommandResult<bool> {
    let request = request.ok_or_else(|| {
        environment_fault("canvas::control-closed", "canvas control receiver closed")
    })?;
    let dark = is_dark_command(request.command());
    let outcome = command_surface.dispatch(seats, request.command());
    let reached_dark = dark && matches!(outcome, Outcome::Reply(_));
    let response = request.respond(outcome);
    if reached_dark {
        if let Ok(response) = response {
            response.wait().await;
        }
        control.close();
    }
    Ok(reached_dark)
}

fn is_dark_command(command: &wire::AdmittedCommand) -> bool {
    matches!(
        command.body(),
        Intent::UpChange(wire::intent::UpChange {
            state: ResidentState::Dark
        })
    )
}
