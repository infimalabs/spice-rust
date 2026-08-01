#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimePaths {
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
    steering: steer::Config,
    consent: consent::Policy,
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
            runtime: state.runtime().to_path_buf(),
            data: state.data().to_path_buf(),
            journal: state.journal().to_path_buf(),
            resident: state.resident().to_path_buf(),
            canvas: state.canvas().to_path_buf(),
            lane_canvas: state.runtime().join("canvas-lanes"),
            inbox: state.runtime().join("inbox"),
            degraded: state.degraded().to_path_buf(),
            tape: state.tape().to_path_buf(),
            stage: repository.worktree_root().join("dist/stage"),
        })
    }
}

#[derive(Debug)]
struct ResidentLock {
    possession: Possession,
}

impl ResidentLock {
    fn acquire(paths: &RuntimePaths) -> CommandResult<Self> {
        fs::create_dir_all(&paths.runtime).map_err(|error| {
            environment_fault("resident::runtime-unavailable", &error.to_string())
        })?;
        let name = LockName::new("resident").expect("resident is a non-empty lock name");
        let lock = LockFile::new(name, &paths.resident);
        match lock
            .try_acquire()
            .map_err(|error| environment_fault("resident::lock-unavailable", &error.to_string()))?
        {
            Acquisition::Acquired(possession) => {
                let resident = Self { possession };
                resident
                    .write_holder(&format!("pid={};epoch=pending\n", std::process::id()))
                    .map_err(|error| {
                        environment_fault("resident::holder-unwritable", &error.to_string())
                    })?;
                Ok(resident)
            }
            Acquisition::Contended => {
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
        self.write_holder(&format!("pid={};epoch={epoch}\n", std::process::id()))
    }

    fn write_holder(&self, holder: &str) -> io::Result<()> {
        let file = self.possession.file();
        file.set_len(0)?;
        let mut writer = file;
        writer.seek(SeekFrom::Start(0))?;
        writer.write_all(holder.as_bytes())?;
        writer.sync_data()
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

pub(crate) fn up() -> Outcome<()> {
    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => return environment_fault("resident::cwd-unavailable", &error.to_string()),
    };
    up_from(&current)
}

fn up_from(current: &Path) -> Outcome<()> {
    let repository = match Repository::discover(current) {
        Ok(repository) => repository,
        Err(error) => return environment_fault("resident::git-unavailable", &error.to_string()),
    };
    let configuration = match spice::config::load_coexisting_repository(&repository, &[]) {
        Ok(configuration) => configuration,
        Err(error) => return configuration_refusal(&error),
    };
    let surface_config = SurfaceConfig {
        steering: configuration.root().steer().clone(),
        consent: configuration.root().consent().policy().clone(),
    };
    let paths = match RuntimePaths::from_repository(&repository) {
        Ok(paths) => paths,
        Err(outcome) => return outcome,
    };
    let trust_surface = match crate::trust_command::Surface::load(&repository) {
        Ok(surface) => surface,
        Err(error) => return crate::trust_command::load_failure(&error),
    };
    if let Err(error) = fs::create_dir_all(&paths.data) {
        return environment_fault("resident::data-unavailable", &error.to_string());
    }
    let resident_lock = match ResidentLock::acquire(&paths) {
        Ok(lock) => lock,
        Err(outcome) => return outcome,
    };
    if let Err(error) = remove_if_present(&paths.canvas) {
        return environment_fault("canvas::stale-discovery", &error.to_string());
    }
    if let Err(error) = remove_tree_if_present(&paths.lane_canvas) {
        return environment_fault("canvas::stale-lane-discovery", &error.to_string());
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
    let startup = match commit_startup(&bus, &repository, &trust_surface, epoch, migration) {
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
            &surface_config,
            &mut seats,
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
    let mut diagnostics = error
        .acquisition()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if let Some(resolution) = error.resolution() {
        diagnostics.extend(resolution.issues().iter().map(ToString::to_string));
    }
    if let Some(repository) = error.repository() {
        diagnostics.push(repository.to_string());
    }
    if diagnostics.is_empty() {
        diagnostics.push(error.to_string());
    }
    Outcome::Refusal(Refusal {
        reason: "resident::configuration-refused".to_owned(),
        witness: Some("effective-configuration".to_owned()),
        expected: Some("one admitted effective repository configuration".to_owned()),
        observed: Some(diagnostics.join(" | ")),
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

fn down_from(current: &Path, contract: &crate::cli::Contract) -> Outcome<()> {
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
    let name = LockName::new("resident").expect("resident is a non-empty lock name");
    let lock = LockFile::new(name, &paths.resident);
    let possession = lock
        .acquire()
        .map_err(|error| environment_fault("resident::release-failed", &error.to_string()))?;
    if paths.canvas.exists() {
        return Err(environment_fault(
            "canvas::release-incomplete",
            "resident lock was released while discovery remained published",
        ));
    }
    drop(possession);
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct StartupCommit {
    first: u64,
    last: u64,
}

fn commit_startup(
    bus: &Bus,
    repository: &Repository,
    surface: &crate::trust_command::Surface,
    epoch: CanonicalUuid,
    migration: Option<journal::Migration>,
) -> CommandResult<StartupCommit> {
    let lanes = bus
        .residency_head()
        .map_err(|error| store_fault("resident::head-unreadable", &error.to_string()))?
        .map(|head| head.into_body().lanes)
        .unwrap_or_default();
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
            let current = crate::trust_command::Surface::load(&guarded_repository)
                .map_err(|_| GuardError::refused("trust::surface-moved"))?;
            if current != guarded_surface {
                return Err(GuardError::refused("trust::surface-moved"));
            }
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

    let mut write = write(Actor::Spice, epoch, Producer::CommandDoor)?;
    let seated = write
        .fact(wire::body::UpSeated { epoch }, cause)
        .map_err(|error| store_fault("journal::write-invalid", &error.to_string()))?;
    write.residency_head(seated, move |_| ResidencyHead {
        epoch,
        state: ResidentState::Seated,
        lanes,
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
) -> CommandResult<()> {
    let operator = new_uuid();
    let mut capabilities = canvas::axum::Capabilities::default();
    capabilities
        .insert(operator, Actor::Operator)
        .map_err(|error| environment_fault("canvas::capability-invalid", &error.to_string()))?;
    let residency = current_residency(bus, epoch)?;
    for lane in &residency.body().lanes {
        for capability in &lane.capabilities {
            capabilities
                .insert(*capability, Actor::Lane(lane.lane))
                .map_err(|error| {
                    environment_fault("canvas::capability-invalid", &error.to_string())
                })?;
        }
    }
    let (server, mut control) = canvas::axum::Server::bind(epoch, capabilities)
        .await
        .map_err(|error| environment_fault("canvas::bind-failed", &error.to_string()))?;
    let discovery = server
        .discovery(operator)
        .map_err(|error| environment_fault("canvas::discovery-invalid", &error.to_string()))?;
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
    let mut server_task = tokio::spawn(server.up_until(async move {
        let _ = stopped.await;
    }));
    loop {
        let request = tokio::select! {
            request = control.recv() => request.ok_or_else(|| {
                environment_fault("canvas::control-closed", "canvas control receiver closed")
            })?,
            served = &mut server_task => {
                return Err(server_outcome(served));
            }
        };
        let dark = is_dark_command(request.command());
        let outcome = dispatch_command(
            bus,
            repository,
            paths,
            epoch,
            config,
            seats,
            request.command(),
        );
        let reached_dark = dark && matches!(outcome, Outcome::Reply(_));
        let response = request.respond(outcome);
        if reached_dark {
            if let Ok(response) = response {
                response.wait().await;
            }
            control.close();
            drop(published);
            let _ = shutdown.send(());
            return match server_task.await {
                Ok(Ok(())) => Ok(()),
                served => Err(server_outcome(served)),
            };
        }
    }
}

fn is_dark_command(command: &wire::AdmittedCommand) -> bool {
    matches!(
        command.body(),
        Intent::UpChange(wire::intent::UpChange {
            state: ResidentState::Dark
        })
    )
}
