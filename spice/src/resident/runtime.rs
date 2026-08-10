use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RuntimePaths {
    pub(super) worktree: PathBuf,
    pub(super) common: PathBuf,
    pub(super) runtime: PathBuf,
    pub(super) data: PathBuf,
    pub(super) journal: PathBuf,
    pub(super) folds: PathBuf,
    pub(super) resident: PathBuf,
    pub(super) canvas: PathBuf,
    pub(super) lane_canvas: PathBuf,
    #[cfg(test)]
    pub(super) inbox: PathBuf,
    pub(super) legacy_inbox: PathBuf,
    pub(super) degraded: PathBuf,
    pub(super) tape: PathBuf,
    pub(super) stage: PathBuf,
}

pub(super) struct SurfaceConfig {
    canvas: canvas::Config,
    pub(super) steering: steer::Config,
    pub(super) taste: taste::Bag,
    pub(super) judge_enabled: bool,
    pub(super) consent: consent::Policy,
    speech: canvas::SpeechConfig,
    pub(super) allocation: board::NextConfig,
    pub(super) projects: board::ProjectPolicy,
    pub(super) lane: lane::Config,
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
pub(super) struct StartupSurface {
    pub(super) config: SurfaceConfig,
    pub(super) trust: crate::trust_command::Surface,
}

pub(super) fn load_startup_surface(
    repository: &Repository,
) -> Result<StartupSurface, spice::config::LoadError> {
    load_startup_surface_with(repository, || {})
}

pub(super) fn load_startup_surface_with<F>(
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
            judge_enabled: loaded.resolved().root().judge().enabled().unwrap_or(false),
            consent: loaded.resolved().root().consent().policy().clone(),
            speech: loaded.resolved().root().say().clone(),
            allocation: *loaded.resolved().root().board().next(),
            projects: loaded.resolved().root().tasks().project_policy(),
            lane: loaded.resolved().root().lane().clone(),
        },
        trust: crate::trust_command::Surface::from_loaded(&loaded),
    })
}

pub(super) fn require_current_surface(
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
    pub(super) fn discover(start: &Path) -> CommandResult<Self> {
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
            folds: state.folds().to_path_buf(),
            resident: state.resident().to_path_buf(),
            canvas: state.canvas().to_path_buf(),
            lane_canvas: state.lane_canvas().to_path_buf(),
            #[cfg(test)]
            inbox: state.inbox().to_path_buf(),
            legacy_inbox: state.runtime().join("inbox"),
            degraded: state.degraded().to_path_buf(),
            tape: state.tape().to_path_buf(),
            stage: repository.worktree_root().join("dist/stage"),
        })
    }

    pub(super) fn inbox_for_worktree(&self, worktree: &str) -> CommandResult<PathBuf> {
        let repository = Repository::discover(worktree).map_err(|error| {
            environment_fault("steer::inbox-worktree-unresolved", &error.to_string())
        })?;
        if repository.common_dir() != self.common
            || repository.worktree_root() != Path::new(worktree)
        {
            return Err(store_fault(
                "steer::inbox-worktree-moved",
                "durable lane worktree no longer names one current repository worktree root",
            ));
        }
        self.inbox_for_repository(&repository)
    }

    pub(super) fn inbox_for_repository(&self, repository: &Repository) -> CommandResult<PathBuf> {
        if repository.common_dir() != self.common {
            return Err(store_fault(
                "steer::inbox-repository-moved",
                "lane repository no longer belongs to the resident Git-common authority",
            ));
        }
        repository
            .admit_state()
            .map(|state| state.inbox().to_path_buf())
            .map_err(|error| crate::state_failure(&error).outcome())
    }

    pub(super) fn clear_stale_surfaces(&self) -> CommandResult<()> {
        remove_if_present(&self.canvas)
            .map_err(|error| environment_fault("canvas::stale-discovery", &error.to_string()))?;
        remove_tree_if_present(&self.lane_canvas)
            .map_err(|error| environment_fault("canvas::stale-lane-discovery", &error.to_string()))
    }
}

#[derive(Debug)]
pub(super) struct ResidentLock {
    possession: Possession,
}

impl ResidentLock {
    pub(super) fn acquire(paths: &RuntimePaths) -> CommandResult<Self> {
        Self::acquire_with(paths, || {})
    }

    pub(super) fn acquire_with(
        paths: &RuntimePaths,
        after_contention: impl FnMut(),
    ) -> CommandResult<Self> {
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
                Err(Outcome::Refusal(Refusal::repairable(
                    wire::SpiceCommand::new("down", std::iter::empty::<&str>())
                        .expect("the static resident shutdown command is valid"),
                    wire::RefusalDiagnostic::new(
                        "resident::already-up".to_owned(),
                        Some("resident-lock".to_owned()),
                        Some("unheld".to_owned()),
                        Some(observed),
                    ),
                )))
            }
        }
    }

    pub(super) fn set_epoch(&self, epoch: CanonicalUuid) -> io::Result<()> {
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
/// This successor uses a port separate from the Python predecessor so both
/// residents can run without requiring the operator to name `--port` on every
/// invocation.
pub(super) const DEFAULT_LISTEN: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8767);
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

pub(super) struct CanvasAccess {
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
pub(super) fn requested_listen(leaf: &clap::ArgMatches) -> CommandResult<SocketAddr> {
    let host = match leaf.get_one::<String>("host") {
        Some(host) => host.parse::<IpAddr>().map_err(|_| {
            listen_refusal(
                "resident::host-invalid",
                "one IP address to listen on",
                host,
            )
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

pub(super) fn requested_access(
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
    let policy = token
        .as_ref()
        .map_or_else(canvas::axum::Access::local, |token| {
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

fn access_refusal(reason: &str, observed: Option<&str>, detail: &str) -> Outcome<()> {
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            reason.to_owned(),
            Some("requested-canvas-access".to_owned()),
            Some("loopback, or an explicitly consented authenticated bind".to_owned()),
            observed.map(str::to_owned),
        )
        .with_detail(detail),
    ))
}

fn listen_refusal(reason: &str, expected: &str, observed: &str) -> Outcome<()> {
    Outcome::Refusal(Refusal::bare(wire::RefusalDiagnostic::new(
        reason.to_owned(),
        Some("requested-listen-address".to_owned()),
        Some(expected.to_owned()),
        Some(observed.to_owned()),
    )))
}

pub(crate) fn up_from(current: &Path, binding: CanvasBinding) -> Outcome<()> {
    up_from_with(current, binding, false, &mut io::sink(), &mut io::sink())
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

    let mut journal = match journal::Journal::open(&paths.journal) {
        Ok(journal) => journal,
        Err(error) => return journal_open_failure(error),
    };
    let migration = journal.migration();
    if let Err(outcome) = steering::admit_index(&mut journal) {
        return outcome;
    }
    let epoch = new_uuid();
    if let Err(error) = resident_lock.set_epoch(epoch) {
        return environment_fault("resident::holder-unwritable", &error.to_string());
    }
    let bus = match Bus::start(journal) {
        Ok(bus) => bus,
        Err(error) => return store_fault("bus::start-failed", &error.to_string()),
    };
    let startup = match commit_startup(&bus, &repository, &startup_surface.trust, epoch, migration)
    {
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
        journal::OpenError::Migration(migration) => Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                "journal::schema-refused".to_owned(),
                None,
                Some(format!("Rust journal schema {}", journal::SCHEMA_VERSION,)),
                Some(migration.to_string()),
            )
            .with_detail(concat!(
                "finish or replace the unfinalized or incompatible Rust authority ",
                "before starting the resident"
            )),
        )),
        error => store_fault("journal::open-failed", &error.to_string()),
    }
}

fn configuration_refusal(error: &spice::config::LoadError) -> Outcome<()> {
    Outcome::Refusal(Refusal::bare(wire::RefusalDiagnostic::new(
        "resident::configuration-refused".to_owned(),
        Some("effective-configuration".to_owned()),
        Some("one admitted effective repository configuration".to_owned()),
        Some(error.diagnostic()),
    )))
}

pub(super) fn settle_surface(
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
            &format!("resident did not release within the {RELEASE_BOUND:?} client bound"),
        )),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(environment_fault(
            "resident::release-failed",
            "resident release waiter ended without reporting lock acquisition",
        )),
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct StartupCommit {
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
    if indexed
        .keys()
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
        != lineage_ids
    {
        return Err(store_fault(
            "lineage::subscription-misaligned",
            "the durable task-subscription table does not match the resident lineages",
        ));
    }
    Ok(indexed.into_values().collect())
}

pub(super) fn commit_startup(
    bus: &Bus,
    repository: &Repository,
    surface: &crate::trust_command::Surface,
    epoch: CanonicalUuid,
    migration: Option<journal::Migration>,
) -> CommandResult<StartupCommit> {
    commit_startup_with(bus, repository, surface, epoch, migration, || {})
}

pub(super) fn commit_startup_with<F>(
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
    let (lanes, subscriptions, topology_revision) = bus
        .residency_head()
        .map_err(|error| store_fault("resident::head-unreadable", &error.to_string()))?
        .map_or_else(
            || Ok((Vec::new(), Vec::new(), 0)),
            |head| {
                let head = head.into_body();
                let subscriptions = startup_subscriptions(&head.lanes, head.subscriptions)?;
                Ok((head.lanes, subscriptions, head.topology_revision))
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
        topology_revision,
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

struct StartedSurface<'a> {
    capabilities: canvas::axum::Capabilities,
    discovery: canvas::Discovery,
    control: canvas::axum::Control,
    published: PublishedDiscovery,
    shutdown: oneshot::Sender<()>,
    server_task: tokio::task::JoinHandle<Result<(), io::Error>>,
    message_publisher: canvas::stream::MessagePublisher,
    progress: &'a mut dyn io::Write,
}

async fn start_surface<'a>(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    residency: &ResidencyHead,
    config: &SurfaceConfig,
    launch: SurfaceLaunch<'a>,
) -> CommandResult<StartedSurface<'a>> {
    let operator = new_uuid();
    let capabilities = surface_capabilities(operator, residency)?;
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
    steering::publish_lane_discoveries(paths, &discovery, residency)?;
    steering::publish_inboxes(bus, paths, residency)?;
    discovery
        .publish(&paths.canvas)
        .map_err(|error| environment_fault("canvas::publish-failed", &error.to_string()))?;
    let (shutdown, stopped) = oneshot::channel();
    let server_task = tokio::spawn(server.up_until(async move {
        let _ = stopped.await;
    }));
    announce_started_surface(
        paths,
        &app_description,
        token.as_deref(),
        open,
        endpoint,
        progress,
    );
    Ok(StartedSurface {
        capabilities,
        discovery,
        control,
        published,
        shutdown,
        server_task,
        message_publisher,
        progress,
    })
}

fn announce_started_surface(
    paths: &RuntimePaths,
    app_description: &str,
    token: Option<&str>,
    open: bool,
    endpoint: &mut dyn io::Write,
    progress: &mut dyn io::Write,
) {
    let _ = writeln!(progress, "canvas app source: {app_description}");
    announce_canvas(
        &paths.canvas,
        token,
        open,
        || {},
        crate::command_client::system_browser,
        endpoint,
        progress,
    );
    let _ = endpoint.flush();
    let _ = progress.flush();
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
    let residency = current_residency(bus, epoch)?;
    let timer_commits = bus.reader().subscribe();
    let timers = timers::Runtime::start(config.projects.clone(), config.steering.clone())?;
    let lifecycle = lifecycle::Runtime::new(&config.lane);
    let mut taste_adjudication = if config.judge_enabled {
        Some(taste_surface::start_adjudication()?)
    } else {
        None
    };
    let started = start_surface(bus, paths, epoch, residency.body(), config, launch).await?;
    let StartedSurface {
        capabilities,
        discovery,
        control,
        published,
        shutdown,
        server_task,
        message_publisher,
        progress,
    } = started;
    let capture::Channels {
        reporter,
        records,
        failures,
        exits,
    } = capture::channels();
    lane_surface::restore_seats(bus, paths, epoch, seats, &reporter)?;
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
    let (taste_submitter, taste_completions) = taste_adjudication
        .as_mut()
        .map(taste_surface::AdjudicationRuntime::parts)
        .map_or((None, None), |(submitter, completions)| {
            (Some(submitter), Some(completions))
        });
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
        taste_submitter,
        taste_completions,
        records,
        failures,
        exits,
        progress,
        timers,
        timer_commits,
        lifecycle,
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
pub(super) fn announce_canvas(
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
    auth_token.map_or_else(
        || endpoint.to_owned(),
        |token| format!("{endpoint}#{token}"),
    )
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
    mut taste_submitter: Option<&mut taste_surface::AdjudicationSubmitter>,
    mut taste_completions: Option<&mut taste_surface::AdjudicationCompletions>,
    mut records: tokio::sync::mpsc::Receiver<lane::capture::CaptureRecord>,
    mut failures: tokio::sync::mpsc::UnboundedReceiver<lane::capture::CaptureFailure>,
    mut exits: tokio::sync::mpsc::UnboundedReceiver<lane::capture::CaptureExit>,
    diagnostic: &mut dyn io::Write,
    mut timers: timers::Runtime,
    mut timer_commits: bus::BroadcastSubscriber,
    mut lifecycle: lifecycle::Runtime,
) -> CommandResult<()> {
    reconcile_wakes(
        bus,
        command_surface,
        seats,
        diagnostic,
        &mut timers,
        &mut lifecycle,
    )?;
    let mut tool_previews = CallPreviews::default();
    loop {
        tokio::select! {
            request = control.recv() => {
                if dispatch_control(
                    command_surface, seats, &mut lifecycle, &mut control, request,
                ).await? {
                    return stop_surface(published, shutdown, server_task).await;
                }
            }
            record = records.recv() => accept_record(
                bus, epoch, seats, command_surface, &message_publisher,
                taste_submitter.as_deref_mut(), record, &mut tool_previews, diagnostic,
            )?,
            completed = receive_judgment(&mut taste_completions) => {
                let completed = completed?;
                taste_submitter
                    .as_deref_mut()
                    .expect("a completion receiver always has its submitter")
                    .finished(&completed);
                taste_surface::complete_judgment(
                    bus,
                    command_surface.paths,
                    epoch,
                    &command_surface.config.taste,
                    &command_surface.config.steering,
                    completed,
                )?;
            }
            failure = failures.recv() => accept_failure(
                bus, epoch, seats, command_surface, &mut lifecycle, failure,
            )?,
            exit = exits.recv() => accept_exit(
                bus, epoch, seats, command_surface, &mut lifecycle, exit,
            )?,
            timer = timers.recv() => {
                let timer = timer.ok_or_else(|| environment_fault(
                    "bus::timer-events-closed",
                    "the production timer event stream closed while the resident was live",
                ))?;
                let fired = timers.consume(bus, timer)?;
                accept_timer_fired(bus, command_surface, &mut lifecycle, fired)?;
                reconcile_wakes(
                    bus, command_surface, seats, diagnostic, &mut timers, &mut lifecycle,
                )?;
            }
            committed = timer_commits.recv() => accept_commit(
                bus, command_surface, seats, diagnostic, &mut timers,
                &mut lifecycle, committed,
            )?,
            ending = &mut server_task => return Err(server_outcome(ending)),
        }
    }
}

async fn receive_judgment(
    completions: &mut Option<&mut taste_surface::AdjudicationCompletions>,
) -> CommandResult<taste_surface::CompletedJudgment> {
    match completions.as_deref_mut() {
        Some(completions) => completions.recv().await,
        None => std::future::pending().await,
    }
}

fn accept_timer_fired(
    bus: &Bus,
    command_surface: &command_surface::Surface<'_>,
    lifecycle: &mut lifecycle::Runtime,
    fired: timers::Fired,
) -> CommandResult<()> {
    match fired {
        timers::Fired::LaneResume(lane) => command_surface.expire_lifecycle_hold(lifecycle, lane),
        timers::Fired::Steering => {
            let residency = bus
                .residency_head()
                .map_err(|error| store_fault("steer::residency-unreadable", &error.to_string()))?;
            if let Some(residency) = residency {
                steering::publish_inboxes(bus, command_surface.paths, residency.body())?;
            }
            Ok(())
        }
        timers::Fired::Stale | timers::Fired::ClaimLapse | timers::Fired::TaskWake(_) => Ok(()),
    }
}

async fn stop_surface(
    published: PublishedDiscovery,
    shutdown: oneshot::Sender<()>,
    server_task: tokio::task::JoinHandle<Result<(), io::Error>>,
) -> CommandResult<()> {
    drop(published);
    let _ = shutdown.send(());
    match server_task.await {
        Ok(Ok(())) => Ok(()),
        ending => Err(server_outcome(ending)),
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one capture record crosses all resident-owned projection surfaces"
)]
fn accept_record(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command_surface: &command_surface::Surface<'_>,
    message_publisher: &canvas::stream::MessagePublisher,
    taste_judge: Option<&mut taste_surface::AdjudicationSubmitter>,
    record: Option<lane::capture::CaptureRecord>,
    tool_previews: &mut CallPreviews,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    let record = record.ok_or_else(|| {
        environment_fault(
            "lane::capture-records-closed",
            "lane capture record receiver closed",
        )
    })?;
    let surfaces = CaptureSurfaces::new(
        command_surface.paths,
        &command_surface.config.taste,
        &command_surface.config.steering,
        message_publisher,
    );
    if let Some(taste_judge) = taste_judge {
        message_surface::accept_capture_record_deferred(
            bus,
            epoch,
            seats,
            &surfaces,
            taste_judge,
            &record,
            message_surface::CaptureViews::new(tool_previews, diagnostic),
        )
    } else {
        accept_capture_record(
            bus,
            epoch,
            seats,
            &surfaces,
            None,
            &record,
            message_surface::CaptureViews::new(tool_previews, diagnostic),
        )
    }
}

fn accept_failure(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command_surface: &command_surface::Surface<'_>,
    lifecycle: &mut lifecycle::Runtime,
    failure: Option<lane::capture::CaptureFailure>,
) -> CommandResult<()> {
    let failure = failure.ok_or_else(|| {
        environment_fault(
            "lane::capture-failures-closed",
            "lane capture failure receiver closed",
        )
    })?;
    command_surface.observe_lifecycle_death(lifecycle, seats, failure.lane, failure.occupant)?;
    lane_surface::capture_failed(bus, epoch, seats, &failure)
}

fn accept_exit(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command_surface: &command_surface::Surface<'_>,
    lifecycle: &mut lifecycle::Runtime,
    exit: Option<lane::capture::CaptureExit>,
) -> CommandResult<()> {
    let exit = exit.ok_or_else(|| {
        environment_fault(
            "lane::capture-exits-closed",
            "lane process exit receiver closed",
        )
    })?;
    command_surface.observe_lifecycle_death(lifecycle, seats, exit.lane, exit.occupant)?;
    lane_surface::occupant_exited(bus, epoch, seats, exit)
}

fn accept_commit(
    bus: &Bus,
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    diagnostic: &mut dyn io::Write,
    timers: &mut timers::Runtime,
    lifecycle: &mut lifecycle::Runtime,
    committed: Result<JournalCommit, tokio::sync::broadcast::error::RecvError>,
) -> CommandResult<()> {
    match committed {
        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
            reconcile_wakes(bus, command_surface, seats, diagnostic, timers, lifecycle)
        }
        Err(tokio::sync::broadcast::error::RecvError::Closed) => Err(environment_fault(
            "bus::lifecycle-commits-closed",
            "the durable commit stream closed while the resident was live",
        )),
    }
}

fn reconcile_wakes(
    bus: &Bus,
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    diagnostic: &mut dyn io::Write,
    timers: &mut timers::Runtime,
    lifecycle: &mut lifecycle::Runtime,
) -> CommandResult<()> {
    timers.synchronize(bus)?;
    let wake = command_surface.reconcile_lifecycle(lifecycle, seats, diagnostic)?;
    timers.synchronize_wake(wake)
}
async fn dispatch_control(
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    lifecycle: &mut lifecycle::Runtime,
    control: &mut canvas::axum::Control,
    request: Option<canvas::axum::ControlRequest>,
) -> CommandResult<bool> {
    let request = request.ok_or_else(|| {
        environment_fault("canvas::control-closed", "canvas control receiver closed")
    })?;
    command_surface.prepare_explicit_lifecycle(lifecycle, request.command())?;
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

#[cfg(test)]
#[path = "tests/judge_liveness.rs"]
mod judge_liveness;
