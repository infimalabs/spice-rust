use std::fs;
use std::io::{self, Seek, SeekFrom, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use bus::{Bus, CommitOutcome, JournalCommit};
use journal::{FactCause, JournalGuard, JournalWrite};
use sill::{Acquisition, LockFile, LockName, Possession};
use tokio::sync::oneshot;
use tungstenite::client::IntoClientRequest;
use tungstenite::http::header::ORIGIN;
use tungstenite::{Message, WebSocket};
use wire::{
    Actor, CanonicalUuid, Fault, FaultClass, Instant, Outcome, Producer, Refusal, Reply,
    ReplyPosition, ResidencyHead, ResidentState, Seq, SeqRange,
};

const JOURNAL_FILE: &str = "spicejournal.sqlite3";
const SOCKET_TIMEOUT: Duration = Duration::from_secs(10);

type CommandResult<T> = Result<T, Outcome<()>>;

#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimePaths {
    runtime: PathBuf,
    data: PathBuf,
    journal: PathBuf,
    resident: PathBuf,
    canvas: PathBuf,
}

impl RuntimePaths {
    fn discover(start: &Path) -> io::Result<Self> {
        let mut common = None;
        for directory in start.ancestors() {
            if let Some(found) = git_common_dir(directory) {
                common = Some(found?);
                break;
            }
        }
        let common = common
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no .git marker found"))?;
        let runtime = common.join(".spice");
        let data = runtime.join("data");
        Ok(Self {
            journal: data.join(JOURNAL_FILE),
            resident: runtime.join("resident"),
            canvas: runtime.join("canvas"),
            runtime,
            data,
        })
    }
}

fn git_common_dir(directory: &Path) -> Option<io::Result<PathBuf>> {
    let marker = directory.join(".git");
    let metadata = match fs::metadata(&marker) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => return Some(Err(error)),
    };
    if metadata.is_dir() {
        return Some(Ok(marker));
    }
    Some((|| {
        let marker_text = fs::read_to_string(&marker)?;
        let git_dir = marker_text
            .trim()
            .strip_prefix("gitdir:")
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid .git file"))?;
        let git_dir = resolve(directory, Path::new(git_dir));
        let common_marker = git_dir.join("commondir");
        match fs::read_to_string(&common_marker) {
            Ok(common) => {
                let common = common.trim();
                if common.is_empty() {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "empty git commondir",
                    ))
                } else {
                    Ok(resolve(&git_dir, Path::new(common)))
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(git_dir),
            Err(error) => Err(error),
        }
    })())
}

fn resolve(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
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
    let paths = match RuntimePaths::discover(current) {
        Ok(paths) => paths,
        Err(error) => return environment_fault("resident::git-unavailable", &error.to_string()),
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
        Err(error) => return store_fault("journal::open-failed", &error.to_string()),
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
    let startup = match commit_startup(&bus, epoch, migration) {
        Ok(startup) => startup,
        Err(outcome) => return outcome,
    };

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return environment_fault("resident::runtime-failed", &error.to_string()),
    };
    let surface = runtime.block_on(run_surface(&bus, &paths, epoch, startup.last));
    let shutdown = bus.shutdown();
    drop(resident_lock);
    if let Err(error) = shutdown {
        return store_fault("bus::shutdown-failed", &error.to_string());
    }
    match surface {
        Ok(()) => committed_reply(startup.first, startup.last),
        Err(outcome) => outcome,
    }
}

pub(crate) fn down() -> Outcome<()> {
    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => return environment_fault("resident::cwd-unavailable", &error.to_string()),
    };
    down_from(&current)
}

fn down_from(current: &Path) -> Outcome<()> {
    let paths = match RuntimePaths::discover(current) {
        Ok(paths) => paths,
        Err(error) => return environment_fault("resident::git-unavailable", &error.to_string()),
    };
    let discovery = match read_discovery(&paths.canvas) {
        Ok(discovery) => discovery,
        Err(outcome) => return outcome,
    };
    request_dark(&discovery)
}

#[derive(Clone, Copy, Debug)]
struct StartupCommit {
    first: u64,
    last: u64,
}

fn commit_startup(
    bus: &Bus,
    epoch: CanonicalUuid,
    migration: Option<journal::Migration>,
) -> CommandResult<StartupCommit> {
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

    let mut write = write(Actor::Spice, epoch, Producer::CommandDoor)?;
    let seated = write
        .fact(wire::body::UpSeated { epoch }, cause)
        .map_err(|error| store_fault("journal::write-invalid", &error.to_string()))?;
    write.residency_head(seated, move |_| ResidencyHead {
        epoch,
        state: ResidentState::Seated,
        lanes: Vec::new(),
    });
    let committed = commit(bus, write)?;
    Ok(StartupCommit {
        first: first.unwrap_or_else(|| committed.first()),
        last: committed.last(),
    })
}

async fn run_surface(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seated: u64,
) -> CommandResult<()> {
    let operator = new_uuid();
    let mut capabilities = canvas::axum::Capabilities::default();
    capabilities
        .insert(operator, Actor::Operator)
        .map_err(|error| environment_fault("canvas::capability-invalid", &error.to_string()))?;
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
    let mut descent = lane::drain::Descent::new();
    let mut previous = seated;

    loop {
        let request = tokio::select! {
            request = control.recv() => request.ok_or_else(|| {
                environment_fault("canvas::control-closed", "canvas control receiver closed")
            })?,
            served = &mut server_task => {
                return Err(server_outcome(served));
            }
        };
        let outcome = drive_shutdown(bus, epoch, &mut descent, &mut previous);
        let dark = matches!(outcome, Outcome::Reply(_)) && descent.dark();
        let _ = request.respond(outcome);
        if dark {
            drop(published);
            let _ = shutdown.send(());
            return match server_task.await {
                Ok(Ok(())) => Ok(()),
                served => Err(server_outcome(served)),
            };
        }
    }
}

fn drive_shutdown(
    bus: &Bus,
    epoch: CanonicalUuid,
    descent: &mut lane::drain::Descent,
    previous: &mut u64,
) -> Outcome<()> {
    let mut first = None;
    while let Some(next) = descent.next() {
        if matches!(next, ResidentState::Dark)
            && let Err(error) = bus.verify()
        {
            return store_fault("resident::verification-failed", &error.to_string());
        }
        let committed = match commit_transition(bus, epoch, descent.at(), next, *previous) {
            Ok(committed) => committed,
            Err(outcome) => return outcome,
        };
        if let Err(error) = descent.step(next) {
            return store_fault("resident::ladder-invalid", &error.to_string());
        }
        first.get_or_insert(committed.first());
        *previous = committed.last();
    }
    committed_reply(first.unwrap_or(*previous), *previous)
}

fn commit_transition(
    bus: &Bus,
    epoch: CanonicalUuid,
    from: ResidentState,
    to: ResidentState,
    previous: u64,
) -> CommandResult<JournalCommit> {
    let mut write = write(Actor::Operator, epoch, Producer::CommandDoor)?;
    write.guard(JournalGuard::new(move |read| {
        let Some(head) = read.residency_head()? else {
            return Err(journal::GuardError::refused("resident::head-missing"));
        };
        if head.body().epoch != epoch {
            return Err(journal::GuardError::refused("resident::epoch-moved"));
        }
        if head.body().state != from {
            return Err(journal::GuardError::refused("resident::state-moved"));
        }
        Ok(())
    }));
    let cause = FactCause::Existing(seq(previous));
    let fact = match to {
        ResidentState::Quiescing => write.fact(wire::body::UpQuiescing { epoch }, cause),
        ResidentState::Draining => write.fact(
            wire::body::UpDraining {
                epoch,
                occupants: 0,
            },
            cause,
        ),
        ResidentState::Terminating => write.fact(
            wire::body::UpTerminating {
                epoch,
                occupants: 0,
            },
            cause,
        ),
        ResidentState::Killing => write.fact(
            wire::body::UpKilling {
                epoch,
                occupants: 0,
            },
            cause,
        ),
        ResidentState::Verifying => write.fact(wire::body::UpVerifying { epoch }, cause),
        ResidentState::Dark => write.fact(wire::body::UpDark { epoch }, cause),
        ResidentState::Seated => {
            return Err(store_fault(
                "resident::ladder-invalid",
                "shutdown attempted to seat a second generation",
            ));
        }
    }
    .map_err(|error| store_fault("journal::write-invalid", &error.to_string()))?;
    write.residency_head(fact, move |_| ResidencyHead {
        epoch,
        state: to,
        lanes: Vec::new(),
    });
    commit(bus, write)
}

fn write(actor: Actor, epoch: CanonicalUuid, producer: Producer) -> CommandResult<JournalWrite> {
    let origin = format!("resident:{epoch}")
        .parse::<wire::Origin>()
        .map_err(|error| store_fault("resident::origin-invalid", &error.to_string()))?;
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

fn read_discovery(path: &Path) -> CommandResult<canvas::Discovery> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(path).map_err(|error| {
            environment_fault("canvas::discovery-unavailable", &error.to_string())
        })?;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(environment_fault(
                "canvas::discovery-insecure",
                "canvas discovery is readable or writable by group or other",
            ));
        }
    }
    let bytes = fs::read(path)
        .map_err(|error| environment_fault("canvas::discovery-unavailable", &error.to_string()))?;
    wire::decode(&bytes)
        .map_err(|error| protocol_fault("canvas::discovery-undecodable", &error.to_string()))
}

fn request_dark(discovery: &canvas::Discovery) -> Outcome<()> {
    let Some(authority) = discovery.endpoint.strip_prefix("http://") else {
        return protocol_fault(
            "canvas::endpoint-invalid",
            "canvas endpoint is not an HTTP loopback origin",
        );
    };
    let address = match authority.parse::<SocketAddr>() {
        Ok(address) if address.ip().is_loopback() => address,
        Ok(address) => {
            return protocol_fault(
                "canvas::endpoint-not-loopback",
                &format!("canvas endpoint names {address}"),
            );
        }
        Err(error) => return protocol_fault("canvas::endpoint-invalid", &error.to_string()),
    };
    let stream = match TcpStream::connect_timeout(&address, SOCKET_TIMEOUT) {
        Ok(stream) => stream,
        Err(error) => return environment_fault("canvas::connect-failed", &error.to_string()),
    };
    if let Err(error) = stream.set_read_timeout(Some(SOCKET_TIMEOUT)) {
        return environment_fault("canvas::timeout-failed", &error.to_string());
    }
    if let Err(error) = stream.set_write_timeout(Some(SOCKET_TIMEOUT)) {
        return environment_fault("canvas::timeout-failed", &error.to_string());
    }
    let mut request = match format!("ws://{authority}/socket").into_client_request() {
        Ok(request) => request,
        Err(error) => return protocol_fault("canvas::request-invalid", &error.to_string()),
    };
    let origin = match discovery.endpoint.parse::<tungstenite::http::HeaderValue>() {
        Ok(origin) => origin,
        Err(error) => return protocol_fault("canvas::origin-invalid", &error.to_string()),
    };
    request.headers_mut().insert(ORIGIN, origin);
    let (mut socket, _) = match tungstenite::client(request, stream) {
        Ok(opened) => opened,
        Err(error) => return environment_fault("canvas::opening-failed", &error.to_string()),
    };
    let hello = wire::Hello {
        wire: wire::WireVersion::CURRENT,
        capability: discovery.capability,
        epoch: discovery.epoch,
    };
    if let Err(outcome) = send(&mut socket, &hello) {
        return outcome;
    }
    let opening = match receive(&mut socket) {
        Ok(opening) => opening,
        Err(outcome) => return outcome,
    };
    match wire::decode::<wire::Accepted>(&opening) {
        Ok(accepted)
            if accepted.wire.is_current()
                && accepted.epoch == discovery.epoch
                && matches!(accepted.actor, Actor::Operator) => {}
        Ok(accepted) => {
            return protocol_fault(
                "hello::acceptance-invalid",
                &format!(
                    "resident accepted as {:?} in {}",
                    accepted.actor, accepted.epoch
                ),
            );
        }
        Err(_) => {
            if let Ok(refusal) = wire::decode::<Refusal>(&opening) {
                return Outcome::Refusal(refusal);
            }
            if let Ok(fault) = wire::decode::<Fault>(&opening) {
                return Outcome::Fault(fault);
            }
            return protocol_fault(
                "hello::answer-undecodable",
                "resident returned neither an acceptance, refusal, nor fault",
            );
        }
    }

    if let Err(outcome) = send(
        &mut socket,
        &wire::intent::UpChange {
            state: ResidentState::Dark,
        },
    ) {
        return outcome;
    }
    match receive(&mut socket) {
        Ok(answer) => wire::decode::<Outcome<()>>(&answer)
            .unwrap_or_else(|error| protocol_fault("down::answer-undecodable", &error.to_string())),
        Err(outcome) => outcome,
    }
}

fn send<T: serde::Serialize>(socket: &mut WebSocket<TcpStream>, value: &T) -> CommandResult<()> {
    let bytes = wire::encode(value)
        .map_err(|error| protocol_fault("canvas::request-unencodable", &error.to_string()))?;
    socket
        .send(Message::Binary(bytes.into()))
        .map_err(|error| environment_fault("canvas::send-failed", &error.to_string()))
}

fn receive(socket: &mut WebSocket<TcpStream>) -> CommandResult<Vec<u8>> {
    loop {
        match socket
            .read()
            .map_err(|error| environment_fault("canvas::receive-failed", &error.to_string()))?
        {
            Message::Binary(bytes) => return Ok(bytes.to_vec()),
            Message::Text(text) => return Ok(text.as_bytes().to_vec()),
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
            Message::Close(_) => {
                return Err(protocol_fault(
                    "canvas::closed-without-answer",
                    "resident closed the socket before answering",
                ));
            }
        }
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

fn store_fault(reason: &str, detail: &str) -> Outcome<()> {
    fault(FaultClass::Store, reason, detail)
}

fn environment_fault(reason: &str, detail: &str) -> Outcome<()> {
    fault(FaultClass::Environment, reason, detail)
}

fn fault(class: FaultClass, reason: &str, detail: &str) -> Outcome<()> {
    Outcome::Fault(Fault {
        class,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

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

        assert_eq!(paths.runtime, root.join(".git/.spice"));
        assert_eq!(
            paths.journal,
            root.join(".git/.spice/data").join(JOURNAL_FILE)
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

        assert_eq!(paths.runtime, git_dir.join("../..").join(".spice"));
        fs::remove_dir_all(root).expect("fixture cleanup");
    }
}
