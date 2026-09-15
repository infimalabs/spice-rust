use super::*;

mod identity;
#[cfg(test)]
pub(crate) use identity::canvas_version;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RuntimePaths {
    pub(super) worktree: PathBuf,
    pub(super) common: PathBuf,
    pub(super) runtime: PathBuf,
    pub(super) data: PathBuf,
    pub(super) journal: PathBuf,
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

struct ResidentClientPaths {
    resident: PathBuf,
    canvas: PathBuf,
}

impl ResidentClientPaths {
    pub(super) fn from_repository(repository: &Repository) -> CommandResult<Self> {
        let canvas = repository
            .admit_resident_client()
            .map_err(|error| crate::state_failure(&error).outcome())?;
        let resident = canvas.with_file_name("resident");
        Ok(Self { resident, canvas })
    }
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
    app: canvas::AppSource,
    version: wire::Version,
    binding: CanvasBinding,
    /// Whether the operator asked this start to also open the app.
    ///
    /// Discovery is announced either way, so this is not part of the binding:
    /// it decides who dials the published address, not which address gets
    /// published or what admits a caller to it.
    open: bool,
    progress: &'a mut dyn io::Write,
    endpoint: &'a mut dyn io::Write,
    daemon: Option<ChildStatus>,
}

// Runtime policy and trust identity derive from this one captured generation.
pub(super) struct StartupSurface {
    pub(super) config: SurfaceConfig,
    pub(super) trust: crate::trust_command::Surface,
}

#[cfg(test)]
pub(super) fn load_startup_surface(
    repository: &Repository,
) -> Result<StartupSurface, spice::config::LoadError> {
    load_startup_surface_with(repository, || {})
}

fn load_startup_surface_scoped(
    repository: &Repository,
    configuration: Option<&spice::config::RepositoryCapture>,
) -> Result<StartupSurface, spice::config::LoadError> {
    let loaded = spice::config::capture_repository_scoped(repository, &[], configuration)?;
    Ok(startup_surface_from_loaded(&loaded))
}

#[cfg(test)]
pub(super) fn load_startup_surface_with<F>(
    repository: &Repository,
    after_capture: F,
) -> Result<StartupSurface, spice::config::LoadError>
where
    F: FnOnce(),
{
    let loaded = spice::config::capture_repository(repository, &[])?;
    after_capture();
    Ok(startup_surface_from_loaded(&loaded))
}

fn startup_surface_from_loaded(loaded: &spice::config::Loaded) -> StartupSurface {
    StartupSurface {
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
        trust: crate::trust_command::Surface::from_loaded(loaded),
    }
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

    pub(super) fn from_repository(repository: &Repository) -> CommandResult<Self> {
        let state = repository
            .admit_resident_state()
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
            .admit_inbox_delivery()
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
                Err(Outcome::Refusal(Refusal::bare(
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
    pub(super) token: Option<String>,
}

enum AppSelection {
    Installed(PathBuf),
    Staged,
}

impl AppSelection {
    fn resolve(self, repository: &Repository) -> CommandResult<canvas::AppSource> {
        match self {
            Self::Installed(executable) => {
                spice::install::installed_canvas_source(repository, &executable)
                    .map_err(installed_app_refusal)
            }
            Self::Staged => {
                canvas::AppSource::resolve(repository.worktree_root()).map_err(|error| {
                    environment_fault("canvas::app-source-invalid", &error.to_string())
                })
            }
        }
    }
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
    child: Option<ChildLaunch>,
    test_canvas_stage: bool,
    progress: &mut dyn io::Write,
    endpoint: &mut dyn io::Write,
    configuration: Option<&spice::config::RepositoryCapture>,
) -> Outcome<()> {
    let listen = match requested_listen(leaf) {
        Ok(listen) => listen,
        Err(outcome) => return outcome,
    };
    let handoff_token = child.as_ref().and_then(|child| child.token.clone());
    let access = match requested_access_with(leaf, listen, handoff_token) {
        Ok(access) => access,
        Err(outcome) => return outcome,
    };
    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => return environment_fault("resident::cwd-unavailable", &error.to_string()),
    };
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            return environment_fault("resident::executable-unavailable", &error.to_string());
        }
    };
    let app = if test_canvas_stage {
        AppSelection::Staged
    } else {
        AppSelection::Installed(executable)
    };
    up_from_with(
        &current,
        app,
        CanvasBinding { listen, access },
        leaf.get_flag("open"),
        progress,
        endpoint,
        (child.map(|child| child.status), configuration),
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
    requested_access_with(leaf, listen, None)
}

fn requested_access_with(
    leaf: &clap::ArgMatches,
    listen: SocketAddr,
    handoff_token: Option<String>,
) -> CommandResult<CanvasAccess> {
    let argument_token = leaf.get_one::<String>("auth-token").cloned();
    if argument_token.is_some() && handoff_token.is_some() {
        return Err(environment_fault(
            "resident::daemon-handoff-ambiguous",
            "the detached child received authentication in both argv and its private handoff",
        ));
    }
    let token = handoff_token.or(argument_token);
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
    let allow_insecure = leaf.get_flag("allow-insecure-bind");
    if !listen.ip().is_loopback() && !allow_insecure {
        return Err(access_refusal(
            "resident::insecure-bind-consent-required",
            Some(&listen.to_string()),
            "add --allow-insecure-bind only when this network exposure is intended",
        ));
    }
    let policy = token.as_ref().map_or_else(
        || {
            if !listen.ip().is_loopback() && allow_insecure {
                canvas::axum::Access::insecure()
            } else {
                canvas::axum::Access::local()
            }
        },
        |token| canvas::axum::Access::authenticated(token.clone()),
    );
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
            Some("loopback, or an explicitly consented non-loopback bind".to_owned()),
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

/// Run the explicit demo/test surface against its worktree-built app stage.
pub(crate) fn up_from(current: &Path, binding: CanvasBinding) -> Outcome<()> {
    up_from_with(
        current,
        AppSelection::Staged,
        binding,
        false,
        &mut io::sink(),
        &mut io::sink(),
        (None, None),
    )
}

fn up_from_with(
    current: &Path,
    app: AppSelection,
    binding: CanvasBinding,
    open: bool,
    progress: &mut dyn io::Write,
    endpoint: &mut dyn io::Write,
    invocation: (
        Option<ChildStatus>,
        Option<&spice::config::RepositoryCapture>,
    ),
) -> Outcome<()> {
    let (daemon, configuration) = invocation;
    let repository = match Repository::discover(current) {
        Ok(repository) => repository,
        Err(error) => return environment_fault("resident::git-unavailable", &error.to_string()),
    };
    let app = match app.resolve(&repository) {
        Ok(app) => app,
        Err(outcome) => return outcome,
    };
    let paths = match RuntimePaths::from_repository(&repository) {
        Ok(paths) => paths,
        Err(outcome) => return outcome,
    };
    if let Err(error) = fs::create_dir_all(&paths.runtime) {
        return environment_fault("resident::runtime-unavailable", &error.to_string());
    }
    let startup_surface = match load_startup_surface_scoped(&repository, configuration) {
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
    let version = match identity::canvas_version_from_journal(&app, &journal) {
        Ok(version) => version,
        Err(outcome) => return outcome,
    };
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
                app,
                version,
                binding,
                open,
                progress,
                endpoint,
                daemon,
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

fn installed_app_refusal(error: spice::install::InstallError) -> Outcome<()> {
    let observed = error.to_string();
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            "canvas::installed-app-refused".to_owned(),
            Some("installed-canvas-generation".to_owned()),
            Some(
                "one complete current receipt-owned Canvas generation matching this Spice binary"
                    .to_owned(),
            ),
            Some(observed.clone()),
        )
        .with_detail(observed),
    ))
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
    down_from_with(current, contract, crate::command_client::execute)
}

/// Stop the resident serving `current` through the operator connection even
/// while a lane identity is ambient, for callers whose root never seats
/// lanes: the demo drives a scratch root that publishes only an operator
/// canvas, so honoring the ambient seat would dial a discovery path the
/// scratch root cannot publish and strand the resident it launched.
pub(crate) fn down_from_operator(current: &Path, contract: &crate::cli::Contract) -> Outcome<()> {
    down_from_with(current, contract, crate::command_client::execute_operator)
}

fn down_from_with(
    current: &Path,
    contract: &crate::cli::Contract,
    execute: fn(
        &Path,
        &crate::cli::Contract,
        crate::command_client::SelectedCommand<'_>,
    ) -> Outcome<CommandAnswer>,
) -> Outcome<()> {
    let repository = match Repository::discover(current) {
        Ok(repository) => repository,
        Err(error) => return environment_fault("resident::git-unavailable", &error.to_string()),
    };
    let paths = match ResidentClientPaths::from_repository(&repository) {
        Ok(paths) => paths,
        Err(outcome) => return outcome,
    };
    let outcome = execute(
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

fn wait_for_release(paths: &ResidentClientPaths) -> CommandResult<()> {
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
fn acquire_released_resident(paths: &ResidentClientPaths) -> CommandResult<Possession> {
    let release_bound = crate::command_client::answer_bound(CommandKind::UpChange, &paths.canvas);

    let name = LockName::new("resident").expect("resident is a non-empty lock name");
    let lock = LockFile::new(name, &paths.resident);
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("spice-release-wait".to_owned())
        .spawn(move || {
            let _ = sender.send(lock.acquire());
        })
        .map_err(|error| environment_fault("resident::release-failed", &error.to_string()))?;

    match receiver.recv_timeout(release_bound) {
        Ok(Ok(possession)) => Ok(possession),
        Ok(Err(error)) => Err(environment_fault(
            "resident::release-failed",
            &error.to_string(),
        )),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(environment_fault(
            "canvas::deadline-elapsed",
            &format!("resident did not release within the {release_bound:?} client bound"),
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
                            "task-allocation lifetime"
                        ),
                    ));
                }
                let policy = wire::intent::LineageLife::from_wire(&row.life).ok_or_else(|| {
                    store_fault(
                        "lineage::subscription-invalid",
                        "a legacy lineage lane has no closed task-allocation lifetime",
                    )
                })?;
                Ok(wire::LineageSubscription { lineage_id, policy })
            })
            .collect();
    }

    let mut indexed = std::collections::BTreeMap::new();
    for subscription in stored {
        if indexed
            .insert(subscription.lineage_id, subscription)
            .is_some()
        {
            return Err(store_fault(
                "lineage::subscription-ambiguous",
                "one lineage has more than one durable task-allocation lifetime",
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
            "the durable task-allocation lifetime table does not match the resident lineages",
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
        let worktree = repository
            .worktree_root()
            .to_str()
            .ok_or_else(|| {
                store_fault(
                    "trust::worktree-unrepresentable",
                    "resident worktree root is not UTF-8",
                )
            })?
            .to_owned();
        let previous = latest_surface_digest(bus)?;
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
                    worktree: Some(worktree.clone()),
                    previous,
                },
                cause,
            )
            .map_err(|error| store_fault("trust::fact-invalid", &error.to_string()))?;
        pending.trust_head(pended, move |_| TrustHead {
            digest,
            state: TrustState::Pended,
            worktree: Some(worktree),
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
    let startup = lane_surface::append_startup_claims(bus, &lanes, epoch, &mut write, seated)?;
    write.residency_head(startup, move |_| ResidencyHead {
        epoch,
        state: ResidentState::Seated,
        topology_revision,
        lanes,
        subscriptions,
        renewals: Vec::new(),
    });
    let committed = commit(bus, write)?;
    Ok(StartupCommit {
        first: first.unwrap_or_else(|| committed.first()),
        last: committed.last(),
    })
}

pub(super) fn latest_surface_digest(bus: &Bus) -> CommandResult<Option<wire::Digest>> {
    let snapshot = bus
        .task_snapshot()
        .map_err(|error| store_fault("trust::journal-unreadable", &error.to_string()))?;
    Ok(snapshot
        .facts()
        .latest_trust_digest(crate::trust_command::SURFACE_NAME)
        .cloned())
}

mod deferral;
mod surface;
#[cfg(test)]
pub(super) use surface::announce_canvas;
use surface::run_surface;
