use std::fs;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::time::Duration;

use bus::{Bus, CommitOutcome, JournalCommit};
use journal::{FactCause, GuardError, JournalGuard, JournalWrite};
use lane::occupant::Seats;
use sill::{Acquisition, LockFile, LockName, Possession};
use spice::repository::Repository;
use tokio::sync::oneshot;
use wire::intent::{
    CommandAnswer, Intent, RecognitionAnswer, ReleasePublish, StateAnswer, TrustAnswer, TrustRef,
};
use wire::{
    Actor, CanonicalUuid, Command, CommandKind, Fault, FaultClass, Instant, Outcome, Producer,
    Refusal, Reply, ReplyPosition, ResidencyHead, ResidentState, Seq, SeqRange, TrustHead,
    TrustState, Witness,
};

type CommandResult<T> = Result<T, Outcome<()>>;

#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimePaths {
    runtime: PathBuf,
    data: PathBuf,
    journal: PathBuf,
    resident: PathBuf,
    canvas: PathBuf,
    degraded: PathBuf,
    tape: PathBuf,
    stage: PathBuf,
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
}

impl Drop for PublishedDiscovery {
    fn drop(&mut self) {
        let _ = remove_if_present(&self.path);
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
        Ok(runtime) => runtime.block_on(run_surface(&bus, &repository, &paths, epoch, &mut seats)),
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
    discovery
        .publish(&paths.canvas)
        .map_err(|error| environment_fault("canvas::publish-failed", &error.to_string()))?;
    let published = PublishedDiscovery {
        path: paths.canvas.clone(),
    };
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
        let outcome = dispatch_command(bus, repository, paths, epoch, seats, request.command());
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

fn dispatch_command(
    bus: &Bus,
    repository: &Repository,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command: &wire::AdmittedCommand,
) -> Outcome<CommandAnswer> {
    match (command.kind(), command.body()) {
        (CommandKind::UpChange, Intent::UpChange(change)) => {
            dispatch_up_change(bus, paths, epoch, seats, command, change)
        }
        (CommandKind::ReleasePublish, Intent::ReleasePublish(publish)) => {
            dispatch_release_publish(bus, paths, epoch, command, publish)
        }
        (CommandKind::TrustGrant | CommandKind::TrustRevoke, Intent::TrustRef(reference)) => {
            dispatch_trust_rule(bus, repository, epoch, command, reference)
        }
        _ => Outcome::Refusal(Refusal {
            reason: "command::handler-not-landed".to_owned(),
            witness: None,
            expected: Some(command.kind().to_string()),
            observed: None,
            remedy: "land the registered resident handler before exposing this CLI leaf".to_owned(),
        }),
    }
}

fn dispatch_trust_rule(
    bus: &Bus,
    repository: &Repository,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    reference: &TrustRef,
) -> Outcome<CommandAnswer> {
    if !matches!(command.actor(), Actor::Operator) {
        return Outcome::Refusal(Refusal {
            reason: "command::authority".to_owned(),
            witness: Some("principal".to_owned()),
            expected: Some("operator".to_owned()),
            observed: Some(command.actor().to_string()),
            remedy: "reconnect with the current operator capability".to_owned(),
        });
    }
    let (from, to) = match command.kind() {
        CommandKind::TrustGrant => (TrustState::Pended, TrustState::Granted),
        CommandKind::TrustRevoke => (TrustState::Granted, TrustState::Revoked),
        _ => unreachable!("trust dispatcher receives only grant or revoke"),
    };
    let surface = match crate::trust_command::Surface::load(repository) {
        Ok(surface) => surface,
        Err(error) => return crate::trust_command::load_failure(&error),
    };
    if reference.digest != *surface.digest() {
        return trust_refusal(
            "trust::digest-not-current",
            Some(surface.digest().to_string()),
            Some(reference.digest.to_string()),
            "run spice trust and rule on its exact current digest",
        );
    }
    let head = match bus.trust_head(&reference.digest) {
        Ok(Some(head)) => head,
        Ok(None) => {
            return trust_refusal(
                "trust::digest-unknown",
                Some(reference.digest.to_string()),
                None,
                "restart the resident to pend the exact current digest before ruling",
            );
        }
        Err(error) => {
            return typed_store_fault("trust::head-unreadable", &error.to_string());
        }
    };
    if head.body().state != from {
        return trust_refusal(
            "trust::standing-moved",
            Some(format!("{from:?}").to_lowercase()),
            Some(format!("{:?}", head.body().state).to_lowercase()),
            "re-read the current trust report before issuing another ruling",
        );
    }
    let expected = Witness::Trust {
        digest: reference.digest.clone(),
        standing: from,
    };
    if command.expected() != [expected] {
        return trust_refusal(
            "trust::witness-moved",
            Some(format!("{}:{from:?}", reference.digest).to_lowercase()),
            Some(format!("{:?}", command.expected())),
            "re-read the current trust head and retry its exact witness",
        );
    }
    let residency = match bus.residency_head() {
        Ok(Some(residency)) => residency,
        Ok(None) => return typed_store_fault("resident::head-missing", "residency head is absent"),
        Err(error) => {
            return typed_store_fault("resident::head-unreadable", &error.to_string());
        }
    };
    if command.epoch() != epoch || residency.body().epoch != epoch {
        return typed_protocol_fault(
            "resident::epoch-moved",
            &format!(
                "request {}, resident {}, durable head {}",
                command.epoch(),
                epoch,
                residency.body().epoch
            ),
        );
    }

    let mut proposal = match write(command.actor().clone(), epoch, Producer::CommandDoor) {
        Ok(proposal) => proposal,
        Err(outcome) => return typed_outcome(outcome),
    };
    let guarded_repository = repository.clone();
    let guarded_surface = surface.clone();
    let guarded_digest = reference.digest.clone();
    proposal.guard(JournalGuard::new(move |read| {
        let current = crate::trust_command::Surface::load(&guarded_repository)
            .map_err(|_| GuardError::refused("trust::surface-moved"))?;
        if current != guarded_surface {
            return Err(GuardError::refused("trust::surface-moved"));
        }
        let current = read
            .trust_head(&guarded_digest)?
            .ok_or_else(|| GuardError::refused("trust::digest-unknown"))?;
        if current.body().state != from {
            return Err(GuardError::refused("trust::standing-moved"));
        }
        let residency = read
            .residency_head()?
            .ok_or_else(|| GuardError::refused("resident::head-missing"))?;
        if residency.body().epoch != epoch {
            return Err(GuardError::refused("resident::epoch-moved"));
        }
        Ok(())
    }));
    let fact = match command.kind() {
        CommandKind::TrustGrant => proposal.fact(
            wire::body::TrustGranted {
                surface: crate::trust_command::SURFACE_NAME.to_owned(),
                digest: reference.digest.clone(),
            },
            FactCause::Existing(head.position()),
        ),
        CommandKind::TrustRevoke => proposal.fact(
            wire::body::TrustRevoked {
                surface: crate::trust_command::SURFACE_NAME.to_owned(),
                digest: reference.digest.clone(),
            },
            FactCause::Existing(head.position()),
        ),
        _ => unreachable!("trust dispatcher receives only grant or revoke"),
    };
    let fact = match fact {
        Ok(fact) => fact,
        Err(error) => return typed_store_fault("trust::fact-invalid", &error.to_string()),
    };
    let digest = reference.digest.clone();
    proposal.trust_head(fact, move |_| TrustHead { digest, state: to });
    let answer = TrustAnswer {
        digest: reference.digest.clone(),
        standing: to,
    };
    if let Err(error) = proposal.command_result(
        Command {
            v: wire::WireVersion::CURRENT,
            id: command.id(),
            epoch: command.epoch(),
            kind: command.kind(),
            expected: command.expected().to_vec(),
            body: reference.clone(),
        },
        epoch,
        answer.clone(),
    ) {
        return typed_store_fault("trust::receipt-invalid", &error.to_string());
    }
    match commit(bus, proposal) {
        Ok(committed) => Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(committed.first()),
                last: seq(committed.last()),
            }),
            body: CommandAnswer::Trust(answer),
        }),
        Err(outcome) => typed_outcome(outcome),
    }
}

fn trust_refusal(
    reason: &str,
    expected: Option<String>,
    observed: Option<String>,
    remedy: &str,
) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: Some("executable-policy".to_owned()),
        expected,
        observed,
        remedy: remedy.to_owned(),
    })
}

fn dispatch_release_publish(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    publish: &ReleasePublish,
) -> Outcome<CommandAnswer> {
    let prepared = match prepare_release_publish(bus, paths, command, publish) {
        Ok(prepared) => prepared,
        Err(outcome) => return outcome,
    };
    if let crate::release_command::PublicationState::Published { fact } = prepared.state {
        return release_recognition(publish.key.clone(), fact, fact, false);
    }
    let crate::release_command::PublicationState::Fresh { proof } = prepared.state else {
        unreachable!("published state returned above")
    };
    let origin = prepared
        .snapshot
        .facts()
        .iter()
        .find(|fact| fact.seq == proof)
        .map(|fact| fact.origin.clone())
        .expect("observed proof position belongs to the same canonical snapshot");
    let proposal = match propose_release_publish(
        paths,
        command,
        publish,
        prepared.release,
        prepared.state,
        proof,
        origin,
        epoch,
    ) {
        Ok(proposal) => proposal,
        Err(outcome) => return outcome,
    };
    match commit(bus, proposal) {
        Ok(committed) => release_recognition(
            publish.key.clone(),
            seq(committed.first()),
            seq(committed.last()),
            true,
        ),
        Err(outcome) => typed_outcome(outcome),
    }
}

struct PreparedReleasePublish {
    release: crate::release_command::AdmittedRelease,
    snapshot: journal::FactSnapshot,
    state: crate::release_command::PublicationState,
}

fn prepare_release_publish(
    bus: &Bus,
    paths: &RuntimePaths,
    command: &wire::AdmittedCommand,
    publish: &ReleasePublish,
) -> Result<PreparedReleasePublish, Outcome<CommandAnswer>> {
    let release = crate::release_command::admit_stage(&paths.stage)
        .map_err(|error| release_refusal("release::manifest-refused", &error.to_string()))?;
    if publish.key != *release.key() || publish.manifest != *release.manifest() {
        return Err(Outcome::Refusal(Refusal {
            reason: "release::plan-moved".to_owned(),
            witness: Some("publication-plan".to_owned()),
            expected: Some(format!("{}:{}", release.key(), release.manifest())),
            observed: Some(format!("{}:{}", publish.key, publish.manifest)),
            remedy: "preview the current sealed manifest and apply its exact key".to_owned(),
        }));
    }
    let snapshot = bus
        .fact_snapshot()
        .map_err(|error| typed_store_fault("release::journal-unreadable", &error.to_string()))?;
    let state = crate::release_command::observe(&snapshot, &release)
        .map_err(|error| release_refusal("release::evidence-refused", &error.to_string()))?;
    if !release_witnesses_match(command.expected(), &release, state) {
        return Err(Outcome::Refusal(Refusal {
            reason: "release::witness-moved".to_owned(),
            witness: Some("observation-and-seals".to_owned()),
            expected: Some(format!(
                "one {} observation at {} and both detached receipts",
                release.key(),
                state.observation()
            )),
            observed: Some(format!("{:?}", command.expected())),
            remedy: "re-read the current release plan and retry its exact witnesses".to_owned(),
        }));
    }
    Ok(PreparedReleasePublish {
        release,
        snapshot,
        state,
    })
}

#[allow(clippy::too_many_arguments)]
fn propose_release_publish(
    paths: &RuntimePaths,
    command: &wire::AdmittedCommand,
    publish: &ReleasePublish,
    release: crate::release_command::AdmittedRelease,
    state: crate::release_command::PublicationState,
    proof: Seq,
    origin: wire::Origin,
    epoch: CanonicalUuid,
) -> Result<JournalWrite, Outcome<CommandAnswer>> {
    let mut proposal = write_at_origin(
        command.actor().clone(),
        origin,
        Producer::ResidentRecognition,
    )
    .map_err(typed_outcome)?;
    let guarded_release = release.clone();
    let guarded_stage = paths.stage.clone();
    proposal.guard(JournalGuard::new(move |read| {
        let current = crate::release_command::admit_stage(&guarded_stage)
            .map_err(|_| GuardError::refused("release::manifest-moved"))?;
        if current != guarded_release {
            return Err(GuardError::refused("release::manifest-moved"));
        }
        let snapshot = read
            .fact_snapshot()
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        let current = crate::release_command::observe(&snapshot, &current)
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        if current != state {
            return Err(GuardError::refused("release::observation-moved"));
        }
        Ok(())
    }));
    proposal
        .fact(release.fact(), FactCause::Existing(proof))
        .map_err(|error| typed_store_fault("release::fact-invalid", &error.to_string()))?;
    let answer = RecognitionAnswer {
        key: publish.key.clone(),
        fresh: true,
    };
    proposal
        .command_result(
            Command {
                v: wire::WireVersion::CURRENT,
                id: command.id(),
                epoch: command.epoch(),
                kind: CommandKind::ReleasePublish,
                expected: command.expected().to_vec(),
                body: publish.clone(),
            },
            epoch,
            answer,
        )
        .map_err(|error| typed_store_fault("release::receipt-invalid", &error.to_string()))?;
    Ok(proposal)
}

fn release_recognition(
    key: wire::Digest,
    first: Seq,
    last: Seq,
    fresh: bool,
) -> Outcome<CommandAnswer> {
    Outcome::Reply(Reply {
        position: ReplyPosition::Committed(SeqRange { first, last }),
        body: CommandAnswer::Recognition(RecognitionAnswer { key, fresh }),
    })
}

fn release_witnesses_match(
    witnesses: &[Witness],
    release: &crate::release_command::AdmittedRelease,
    state: crate::release_command::PublicationState,
) -> bool {
    let observations = witnesses
        .iter()
        .filter_map(|witness| match witness {
            Witness::Observation { key, head } => Some((key, *head)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut sealed = witnesses
        .iter()
        .filter_map(|witness| match witness {
            Witness::Sealed { proof, manifest } if manifest == release.manifest() => Some(proof),
            Witness::Sealed { .. } => None,
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut receipts = release.receipts().collect::<Vec<_>>();
    sealed.sort_unstable();
    receipts.sort_unstable();
    witnesses.len() == 3
        && observations == [(release.key(), state.observation())]
        && sealed == receipts
}

fn release_refusal(reason: &str, observed: &str) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: Some("sealed-release".to_owned()),
        expected: Some("one current complete two-target proof and manifest chain".to_owned()),
        observed: Some(observed.to_owned()),
        remedy: "restore and recognize the exact current sealed release before publishing"
            .to_owned(),
    })
}

fn dispatch_up_change(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command: &wire::AdmittedCommand,
    change: &wire::intent::UpChange,
) -> Outcome<CommandAnswer> {
    if !matches!(command.actor(), Actor::Operator) {
        return Outcome::Refusal(Refusal {
            reason: "command::authority".to_owned(),
            witness: Some("principal".to_owned()),
            expected: Some("operator".to_owned()),
            observed: Some(command.actor().to_string()),
            remedy: "reconnect with the current operator capability".to_owned(),
        });
    }
    if !matches!(change.state, ResidentState::Dark) {
        return Outcome::Refusal(Refusal {
            reason: "down::target-not-dark".to_owned(),
            witness: Some("transition".to_owned()),
            expected: Some("dark".to_owned()),
            observed: Some(format!("{:?}", change.state).to_lowercase()),
            remedy: "request Dark and let the resident drive every bounded edge".to_owned(),
        });
    }
    let head = match bus.residency_head() {
        Ok(Some(head)) => head,
        Ok(None) => {
            return typed_store_fault("resident::head-missing", "residency head is absent");
        }
        Err(error) => {
            return typed_store_fault("resident::head-unreadable", &error.to_string());
        }
    };
    if head.body().epoch != epoch || command.epoch() != epoch {
        return typed_protocol_fault(
            "resident::epoch-moved",
            &format!(
                "request {}, resident {}, durable head {}",
                command.epoch(),
                epoch,
                head.body().epoch
            ),
        );
    }
    let from = head.body().state;
    let transition_matches = command.expected().iter().any(|witness| {
        matches!(
            witness,
            Witness::Transition {
                from: witnessed_from,
                to: ResidentState::Dark,
            } if *witnessed_from == from
        )
    });
    if !transition_matches {
        return Outcome::Refusal(Refusal {
            reason: "resident::transition-moved".to_owned(),
            witness: Some("transition".to_owned()),
            expected: Some(format!("{:?}->dark", from).to_lowercase()),
            observed: command.expected().iter().find_map(|witness| match witness {
                Witness::Transition { from, to } => {
                    Some(format!("{from:?}->{to:?}").to_lowercase())
                }
                _ => None,
            }),
            remedy: "read the current resident state and retry the exact transition".to_owned(),
        });
    }
    match drive_shutdown(bus, paths, epoch, seats) {
        Outcome::Reply(reply) => Outcome::Reply(Reply {
            position: reply.position,
            body: CommandAnswer::State(StateAnswer {
                from,
                to: ResidentState::Dark,
            }),
        }),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

mod shutdown;

use shutdown::{current_residency, drive_shutdown};

fn write(actor: Actor, epoch: CanonicalUuid, producer: Producer) -> CommandResult<JournalWrite> {
    let origin = format!("resident:{epoch}")
        .parse::<wire::Origin>()
        .map_err(|error| store_fault("resident::origin-invalid", &error.to_string()))?;
    write_at_origin(actor, origin, producer)
}

fn write_at_origin(
    actor: Actor,
    origin: wire::Origin,
    producer: Producer,
) -> CommandResult<JournalWrite> {
    let at = Instant::new(jiff::Timestamp::now())
        .map_err(|error| environment_fault("resident::clock-invalid", &error.to_string()))?;
    Ok(JournalWrite::new(actor, origin, at, producer))
}

fn commit(bus: &Bus, write: JournalWrite) -> CommandResult<JournalCommit> {
    match bus
        .commit(write)
        .map_err(|error| store_fault("bus::commit-failed", &error.to_string()))?
    {
        CommitOutcome::Committed(committed) => Ok(committed),
        CommitOutcome::Refused(refusal) => Err(Outcome::Refusal(Refusal {
            reason: refusal.reason().to_owned(),
            witness: Some("journal-guard".to_owned()),
            expected: None,
            observed: None,
            remedy: "read current resident discovery and retry against its epoch".to_owned(),
        })),
        CommitOutcome::Fault(fault) => Err(store_fault("journal::commit-failed", fault.detail())),
    }
}

fn decode_dark_outcome(outcome: Outcome<CommandAnswer>) -> Outcome<()> {
    match outcome {
        Outcome::Reply(reply) => match reply.body {
            CommandAnswer::State(StateAnswer {
                to: ResidentState::Dark,
                ..
            }) => Outcome::Reply(Reply {
                position: reply.position,
                body: (),
            }),
            answer => protocol_fault(
                "down::answer-shape",
                &format!("expected a state answer ending in dark, received {answer:?}"),
            ),
        },
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

fn server_outcome(served: Result<Result<(), io::Error>, tokio::task::JoinError>) -> Outcome<()> {
    match served {
        Ok(Ok(())) => environment_fault("canvas::stopped", "canvas server stopped early"),
        Ok(Err(error)) => environment_fault("canvas::serve-failed", &error.to_string()),
        Err(error) => environment_fault("canvas::task-failed", &error.to_string()),
    }
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn committed_reply(first: u64, last: u64) -> Outcome<()> {
    Outcome::Reply(Reply {
        position: ReplyPosition::Committed(SeqRange {
            first: seq(first),
            last: seq(last),
        }),
        body: (),
    })
}

fn seq(value: u64) -> Seq {
    Seq::new(value).expect("journal commits always have non-zero positions")
}

fn new_uuid() -> CanonicalUuid {
    CanonicalUuid::new(uuid::Uuid::new_v4())
}

fn protocol_fault(reason: &str, detail: &str) -> Outcome<()> {
    fault(FaultClass::Protocol, reason, detail)
}

fn typed_protocol_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    fault(FaultClass::Protocol, reason, detail)
}

fn store_fault(reason: &str, detail: &str) -> Outcome<()> {
    fault(FaultClass::Store, reason, detail)
}

fn typed_store_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    fault(FaultClass::Store, reason, detail)
}

fn typed_outcome(outcome: Outcome<()>) -> Outcome<CommandAnswer> {
    match outcome {
        Outcome::Reply(_) => unreachable!("unit command failures never carry replies"),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

fn environment_fault(reason: &str, detail: &str) -> Outcome<()> {
    fault(FaultClass::Environment, reason, detail)
}

fn fault<B>(class: FaultClass, reason: &str, detail: &str) -> Outcome<B> {
    Outcome::Fault(Fault {
        class,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant as Monotonic;

    use sill::{
        Argv, Cancellation, Deadlines, Environment, Output, ProcessGroup, Redaction,
        RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitPolicy,
    };

    use super::*;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    fn fixture(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "spice-resident-{name}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn runtime_paths_follow_the_repository_common_directory() {
        let root = fixture("main");
        let nested = root.join("a/b");
        fs::create_dir_all(root.join(".git")).expect("git directory");
        fs::create_dir_all(&nested).expect("nested directory");

        let paths = RuntimePaths::discover(&nested).expect("paths resolve");
        let common = fs::canonicalize(root.join(".git")).expect("canonical common directory");

        assert_eq!(paths.runtime, common.join(".spice-rust"));
        assert_eq!(
            paths.journal,
            common.join(".spice-rust/data/spicejournal.sqlite3")
        );
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn linked_worktrees_share_the_git_common_directory() {
        let root = fixture("linked");
        let worktree = root.join("worktree");
        let common = root.join("common.git");
        let git_dir = common.join("worktrees/linked");
        fs::create_dir_all(&worktree).expect("worktree directory");
        fs::create_dir_all(&git_dir).expect("git directory");
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .expect("git marker");
        fs::write(git_dir.join("commondir"), "../..\n").expect("common marker");

        let paths = RuntimePaths::discover(&worktree).expect("paths resolve");
        let common = fs::canonicalize(&common).expect("canonical common directory");

        assert_eq!(paths.runtime, common.join(".spice-rust"));
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn cooperative_and_forced_groups_drive_observed_counts_and_residue() {
        let (root, paths) = resident_fixture("groups");
        let repository = Repository::discover(&root).expect("fixture repository resolves");
        let surface =
            crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
        let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
        let bus = Bus::start(journal).expect("fixture bus starts");
        let epoch = test_uuid(1);
        commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
        let mut seats = Seats::new();

        let drained_lane = test_uuid(2);
        let drained_occupant = test_uuid(3);
        seats
            .seat(
                drained_lane,
                drained_occupant,
                process_spec("sleep 0.05", Duration::from_secs(5)),
            )
            .expect("cooperative group starts");
        seed_lane(&bus, epoch, drained_lane, drained_occupant);

        let killed_lane = test_uuid(4);
        let killed_occupant = test_uuid(5);
        seats
            .seat(
                killed_lane,
                killed_occupant,
                process_spec(
                    "trap '' TERM; while :; do sleep 1; done",
                    Duration::from_millis(100),
                ),
            )
            .expect("non-cooperative group starts");
        seed_lane(&bus, epoch, killed_lane, killed_occupant);

        fs::create_dir_all(&paths.degraded).expect("degraded directory");
        fs::write(
            paths.degraded.join(drained_lane.to_string()),
            "offline run\n",
        )
        .expect("degraded marker");
        fs::create_dir_all(&paths.tape).expect("tape directory");
        let tape_lane = test_uuid(6);
        fs::write(paths.tape.join(tape_lane.to_string()), "not a tape")
            .expect("malformed tape residue");
        let sealed_lane = test_uuid(8);
        let tape =
            tape::Tape::open(paths.tape.join(sealed_lane.to_string())).expect("live tape opens");
        let locus = tape
            .tee(b"resident shutdown evidence")
            .expect("tape captures");

        let shutdown_started = Monotonic::now();
        assert!(matches!(
            drive_shutdown(&bus, &paths, epoch, &mut seats),
            Outcome::Reply(_)
        ));
        assert!(
            shutdown_started.elapsed() < Duration::from_secs(2),
            "one group's unused runtime extended another group's drain bound"
        );
        assert!(seats.is_empty());
        bus.verify().expect("Dark authority verifies");
        let snapshot = bus.fact_snapshot().expect("facts read through bus");
        assert_eq!(
            fact_u64(&snapshot, wire::FactKind::UpDraining, "occupants"),
            2
        );
        assert_eq!(
            fact_u64(&snapshot, wire::FactKind::UpTerminating, "occupants"),
            1
        );
        assert_eq!(
            fact_u64(&snapshot, wire::FactKind::UpKilling, "occupants"),
            1
        );
        assert!(
            snapshot
                .facts()
                .iter()
                .any(|fact| fact.fact.kind() == wire::FactKind::LaneDrained)
        );
        assert!(
            snapshot
                .facts()
                .iter()
                .any(|fact| fact.fact.kind() == wire::FactKind::LaneExited)
        );
        let degraded = snapshot
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::LaneDegraded)
            .count();
        assert_eq!(degraded, 2, "disk residue was not named once per lane");
        let head = bus
            .residency_head()
            .expect("head read")
            .expect("head exists")
            .into_body();
        assert_eq!(head.state, ResidentState::Dark);
        assert_eq!(head.lanes.len(), 2);
        assert!(head.lanes.iter().all(|lane| lane.occupant.is_none()));
        assert!(!paths.degraded.join(drained_lane.to_string()).exists());
        assert_eq!(
            tape.read(locus).expect("sealed tape remains readable"),
            b"resident shutdown evidence"
        );
        assert!(matches!(
            tape.seal().expect("tape was sealed"),
            tape::Seal::Empty
        ));
        bus.shutdown().expect("fixture bus stops");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn a_post_seated_surface_failure_compensates_to_verified_dark() {
        let (root, paths) = resident_fixture("compensation");
        let repository = Repository::discover(&root).expect("fixture repository resolves");
        let surface =
            crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
        let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
        let bus = Bus::start(journal).expect("fixture bus starts");
        let epoch = test_uuid(7);
        let startup =
            commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
        let original = environment_fault("canvas::fixture-failed", "after UpSeated");

        let settled = settle_surface(
            &bus,
            &paths,
            epoch,
            startup,
            &mut Seats::new(),
            Err(original),
        );

        let Outcome::Fault(fault) = settled else {
            panic!("the original startup failure must remain visible");
        };
        assert_eq!(fault.reason, "canvas::fixture-failed");
        let head = bus
            .residency_head()
            .expect("head read")
            .expect("head exists")
            .into_body();
        assert_eq!(head.state, ResidentState::Dark);
        assert!(head.lanes.is_empty());
        bus.verify().expect("compensated authority verifies");
        bus.shutdown().expect("fixture bus stops");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    fn resident_fixture(name: &str) -> (PathBuf, RuntimePaths) {
        let root = fixture(name);
        fs::create_dir_all(root.join(".git")).expect("git directory");
        let paths = RuntimePaths::discover(&root).expect("runtime paths resolve");
        fs::create_dir_all(&paths.data).expect("data directory");
        (root, paths)
    }

    fn process_spec(command: &str, runtime: Duration) -> Spec {
        Spec {
            argv: Argv::new("/bin/sh", ["-c", command]).expect("fixture argv"),
            cwd: PathBuf::from("/"),
            environment: Environment::new([("PATH", "/usr/bin:/bin")])
                .expect("fixture environment"),
            group: ProcessGroup::New,
            deadlines: Deadlines::new(runtime, Duration::from_secs(1), Duration::from_secs(1))
                .expect("fixture deadlines"),
            wait: WaitPolicy::new(
                "resident fixture",
                Cancellation::Never,
                RestartDisposition::Never,
            )
            .expect("fixture wait policy"),
            output: Output::new(StdinPolicy::Null, StreamPolicy::Null, StreamPolicy::Null),
            redaction: Redaction::None,
            grace: Duration::from_millis(100),
        }
    }

    fn seed_lane(bus: &Bus, epoch: CanonicalUuid, lane: CanonicalUuid, occupant: CanonicalUuid) {
        let current = current_residency(bus, epoch).expect("current residency");
        let mut head = current.body().clone();
        head.lanes.push(wire::LaneResidency {
            lane,
            occupant: Some(occupant),
            lineage: vec![occupant],
            capabilities: vec![test_uuid(9)],
            disposition: "occupied".to_owned(),
        });
        head.lanes.sort_by_key(|lane| lane.lane);
        let mut write = write(Actor::Spice, epoch, Producer::ResidentRecognition)
            .expect("fixture write starts");
        let seated = write
            .fact(
                wire::body::LaneSeated {
                    lane,
                    capability: test_uuid(9),
                    worktree: "/fixture".to_owned(),
                },
                FactCause::Existing(current.position()),
            )
            .expect("lane seats");
        let started = write
            .fact(
                wire::body::LaneStarted {
                    lane,
                    occupant,
                    epoch,
                },
                FactCause::Pending(seated),
            )
            .expect("occupant starts");
        write.residency_head(started, move |_| head);
        commit(bus, write).expect("fixture lane commits");
    }

    fn fact_u64(snapshot: &journal::FactSnapshot, kind: wire::FactKind, field: &str) -> u64 {
        snapshot
            .facts()
            .iter()
            .find(|fact| fact.fact.kind() == kind)
            .and_then(|fact| fact.fact.body().get(field))
            .and_then(|value| value.as_u64())
            .expect("fact carries numeric fixture field")
    }

    fn test_uuid(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
            .parse()
            .expect("fixture UUID")
    }
}
