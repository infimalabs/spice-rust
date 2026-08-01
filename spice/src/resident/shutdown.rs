use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant as Monotonic};

use bus::{Bus, JournalCommit};
use journal::{FactCause, JournalGuard, JournalWrite};
use lane::occupant::{Occupant, OccupantWait, Seats};
use sill::{Completion, WaitOutcome};
use wire::{Actor, CanonicalUuid, Outcome, Producer, ResidencyHead, ResidentState};

use super::{
    CommandResult, RuntimePaths, commit, committed_reply, environment_fault, remove_if_present,
    seq, store_fault, write,
};

const VACANT: &str = "vacant";
const DEGRADED: &str = "degraded";
const SIGNAL_EXIT: i32 = -1;

#[derive(Clone, Copy)]
enum ExitClass {
    Drained,
    Exited,
}

#[derive(Debug)]
enum ProcessEvidence {
    Drained {
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
    },
    Exited {
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        code: i32,
    },
    Degraded {
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        marker: String,
    },
}

impl ProcessEvidence {
    fn lane(&self) -> CanonicalUuid {
        match self {
            Self::Drained { lane, .. }
            | Self::Exited { lane, .. }
            | Self::Degraded { lane, .. } => *lane,
        }
    }

    fn occupant(&self) -> CanonicalUuid {
        match self {
            Self::Drained { occupant, .. }
            | Self::Exited { occupant, .. }
            | Self::Degraded { occupant, .. } => *occupant,
        }
    }
}

#[derive(Default)]
struct ShutdownOccupants {
    running: Vec<Occupant>,
    signal_failures: BTreeMap<CanonicalUuid, String>,
}

impl ShutdownOccupants {
    fn take(seats: &mut Seats) -> Self {
        let running = seats
            .lanes()
            .into_iter()
            .filter_map(|lane| seats.vacate(lane))
            .collect();
        Self {
            running,
            signal_failures: BTreeMap::new(),
        }
    }

    fn contains(&self, lane: CanonicalUuid, occupant: CanonicalUuid) -> bool {
        self.running
            .iter()
            .any(|running| running.lane() == lane && running.occupant() == occupant)
    }

    fn signal_terminate(&mut self) {
        self.signal("terminate", Occupant::terminate);
    }

    fn signal_kill(&mut self) {
        self.signal("kill", Occupant::kill);
    }

    fn signal(&mut self, stage: &str, send: fn(&Occupant) -> Result<(), sill::WaitError>) {
        for occupant in &self.running {
            if let Err(error) = send(occupant) {
                self.signal_failures
                    .insert(occupant.lane(), format!("{stage}: {error}"));
            }
        }
    }

    fn observe(
        &mut self,
        bound: fn(&Occupant) -> Duration,
        class: ExitClass,
    ) -> Vec<ProcessEvidence> {
        let observations = std::mem::take(&mut self.running)
            .into_iter()
            .map(|occupant| {
                let duration = bound(&occupant);
                (occupant, duration)
            })
            .collect::<Vec<_>>();
        let started = Monotonic::now();
        let mut evidence = Vec::new();
        let mut running = Vec::new();
        for (occupant, duration) in observations {
            let lane = occupant.lane();
            let named = occupant.occupant();
            let remaining = duration.saturating_sub(started.elapsed());
            match occupant.wait_for(remaining) {
                Ok(OccupantWait::Exited { outcome, .. }) => {
                    self.signal_failures.remove(&lane);
                    evidence.push(exited_evidence(lane, named, outcome, class));
                }
                Ok(OccupantWait::Running(occupant)) => running.push(*occupant),
                Err(error) => evidence.push(ProcessEvidence::Degraded {
                    lane,
                    occupant: named,
                    marker: format!("wait: {error}"),
                }),
            }
        }
        self.running = running;
        evidence
    }

    fn finish_residue(&mut self) -> Vec<ProcessEvidence> {
        std::mem::take(&mut self.running)
            .into_iter()
            .map(|occupant| {
                let lane = occupant.lane();
                let marker = self
                    .signal_failures
                    .remove(&lane)
                    .unwrap_or_else(|| "process survived configured reap bound".to_owned());
                ProcessEvidence::Degraded {
                    lane,
                    occupant: occupant.occupant(),
                    marker,
                }
            })
            .collect()
    }
}

fn exited_evidence(
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    outcome: WaitOutcome,
    class: ExitClass,
) -> ProcessEvidence {
    match class {
        ExitClass::Drained => ProcessEvidence::Drained { lane, occupant },
        ExitClass::Exited => ProcessEvidence::Exited {
            lane,
            occupant,
            code: exit_code(outcome.completion()),
        },
    }
}

fn exit_code(completion: &Completion) -> i32 {
    completion.status().code().unwrap_or(SIGNAL_EXIT)
}

struct ShutdownProgress {
    descent: lane::drain::Descent,
    first: Option<u64>,
    previous: u64,
}

impl ShutdownProgress {
    fn new(head: &journal::JournalHead<ResidencyHead>) -> Self {
        Self {
            descent: lane::drain::Descent::resume(head.body().state),
            first: None,
            previous: head.position().get(),
        }
    }

    fn remember(&mut self, committed: JournalCommit) {
        self.first.get_or_insert(committed.first());
        self.previous = committed.last();
    }

    fn transition(
        &mut self,
        bus: &Bus,
        epoch: CanonicalUuid,
        to: ResidentState,
    ) -> CommandResult<()> {
        let committed = commit_transition(bus, epoch, self.descent.at(), to, self.previous)?;
        self.descent
            .step(to)
            .map_err(|error| store_fault("resident::ladder-invalid", &error.to_string()))?;
        self.remember(committed);
        Ok(())
    }

    fn evidence(
        &mut self,
        bus: &Bus,
        epoch: CanonicalUuid,
        evidence: Vec<ProcessEvidence>,
    ) -> CommandResult<()> {
        if let Some(committed) = commit_process_evidence(bus, epoch, evidence, self.previous)? {
            self.remember(committed);
        }
        Ok(())
    }

    fn reply(&self) -> Outcome<()> {
        committed_reply(self.first.unwrap_or(self.previous), self.previous)
    }
}

pub(super) fn drive_shutdown(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
) -> Outcome<()> {
    let head = match current_residency(bus, epoch) {
        Ok(head) => head,
        Err(outcome) => return outcome,
    };
    let mut progress = ShutdownProgress::new(&head);
    let mut occupants = ShutdownOccupants::take(seats);
    loop {
        let result = match progress.descent.at() {
            ResidentState::Seated => progress.transition(bus, epoch, ResidentState::Quiescing),
            ResidentState::Quiescing => progress.transition(bus, epoch, ResidentState::Draining),
            ResidentState::Draining => drain_occupants(bus, epoch, &mut progress, &mut occupants),
            ResidentState::Terminating => {
                terminate_occupants(bus, epoch, &mut progress, &mut occupants)
            }
            ResidentState::Killing => kill_occupants(bus, epoch, &mut progress, &mut occupants),
            ResidentState::Verifying => recognize_disk_residue(bus, paths, epoch, &mut progress)
                .and_then(|()| verify_and_dark(bus, epoch, &mut progress)),
            ResidentState::Dark => return progress.reply(),
        };
        if let Err(outcome) = result {
            return outcome;
        }
    }
}

fn drain_occupants(
    bus: &Bus,
    epoch: CanonicalUuid,
    progress: &mut ShutdownProgress,
    occupants: &mut ShutdownOccupants,
) -> CommandResult<()> {
    let mut evidence = occupants.observe(Occupant::drain_remaining, ExitClass::Drained);
    evidence.extend(unowned_evidence(bus, epoch, occupants, &evidence)?);
    progress.evidence(bus, epoch, evidence)?;
    progress.transition(bus, epoch, ResidentState::Terminating)
}

fn terminate_occupants(
    bus: &Bus,
    epoch: CanonicalUuid,
    progress: &mut ShutdownProgress,
    occupants: &mut ShutdownOccupants,
) -> CommandResult<()> {
    occupants.signal_terminate();
    let mut evidence = occupants.observe(Occupant::termination_grace, ExitClass::Exited);
    evidence.extend(unowned_evidence(bus, epoch, occupants, &evidence)?);
    progress.evidence(bus, epoch, evidence)?;
    progress.transition(bus, epoch, ResidentState::Killing)
}

fn kill_occupants(
    bus: &Bus,
    epoch: CanonicalUuid,
    progress: &mut ShutdownProgress,
    occupants: &mut ShutdownOccupants,
) -> CommandResult<()> {
    occupants.signal_kill();
    let mut evidence = occupants.observe(Occupant::reap_bound, ExitClass::Exited);
    evidence.extend(occupants.finish_residue());
    evidence.extend(unowned_evidence(bus, epoch, occupants, &evidence)?);
    progress.evidence(bus, epoch, evidence)?;
    progress.transition(bus, epoch, ResidentState::Verifying)
}

fn verify_and_dark(
    bus: &Bus,
    epoch: CanonicalUuid,
    progress: &mut ShutdownProgress,
) -> CommandResult<()> {
    bus.verify()
        .map_err(|error| store_fault("resident::verification-failed", &error.to_string()))?;
    progress.transition(bus, epoch, ResidentState::Dark)?;
    bus.verify()
        .map_err(|error| store_fault("resident::final-dark-invalid", &error.to_string()))?;
    Ok(())
}

pub(super) fn current_residency(
    bus: &Bus,
    epoch: CanonicalUuid,
) -> CommandResult<journal::JournalHead<ResidencyHead>> {
    let head = bus
        .residency_head()
        .map_err(|error| store_fault("resident::head-unreadable", &error.to_string()))?
        .ok_or_else(|| store_fault("resident::head-missing", "residency head is absent"))?;
    if head.body().epoch != epoch {
        return Err(store_fault(
            "resident::epoch-moved",
            &format!("expected {epoch}, observed {}", head.body().epoch),
        ));
    }
    Ok(head)
}

fn unowned_evidence(
    bus: &Bus,
    epoch: CanonicalUuid,
    occupants: &ShutdownOccupants,
    pending: &[ProcessEvidence],
) -> CommandResult<Vec<ProcessEvidence>> {
    let head = current_residency(bus, epoch)?;
    Ok(head
        .body()
        .lanes
        .iter()
        .filter_map(|lane| lane.occupant.map(|occupant| (lane.lane, occupant)))
        .filter(|(lane, occupant)| !occupants.contains(*lane, *occupant))
        .filter(|(lane, _)| !pending.iter().any(|evidence| evidence.lane() == *lane))
        .map(|(lane, occupant)| ProcessEvidence::Degraded {
            lane,
            occupant,
            marker: "durable occupant had no supervised process group".to_owned(),
        })
        .collect())
}

fn commit_process_evidence(
    bus: &Bus,
    epoch: CanonicalUuid,
    evidence: Vec<ProcessEvidence>,
    previous: u64,
) -> CommandResult<Option<JournalCommit>> {
    if evidence.is_empty() {
        return Ok(None);
    }
    let observed = current_residency(bus, epoch)?;
    let mut head = observed.body().clone();
    for observed in &evidence {
        vacate_observed_lane(&mut head, observed)?;
    }
    let mut write = write(Actor::Spice, epoch, Producer::ResidentRecognition)?;
    guard_residency(&mut write, &observed);
    let mut cause = FactCause::Existing(seq(previous));
    let mut last = None;
    for observed in evidence {
        let fact = match observed {
            ProcessEvidence::Drained { lane, occupant } => {
                write.fact(wire::body::LaneDrained { lane, occupant }, cause)
            }
            ProcessEvidence::Exited {
                lane,
                occupant,
                code,
            } => write.fact(
                wire::body::LaneExited {
                    lane,
                    occupant,
                    code,
                },
                cause,
            ),
            ProcessEvidence::Degraded { lane, marker, .. } => {
                write.fact(wire::body::LaneDegraded { lane, marker }, cause)
            }
        }
        .map_err(|error| store_fault("journal::write-invalid", &error.to_string()))?;
        cause = FactCause::Pending(fact);
        last = Some(fact);
    }
    write.residency_head(
        last.expect("non-empty evidence has a last fact"),
        move |_| head,
    );
    commit(bus, write).map(Some)
}

fn vacate_observed_lane(head: &mut ResidencyHead, evidence: &ProcessEvidence) -> CommandResult<()> {
    let lane = head
        .lanes
        .iter_mut()
        .find(|lane| lane.lane == evidence.lane())
        .ok_or_else(|| {
            store_fault(
                "resident::lane-missing",
                &format!("lane {} is absent from residency", evidence.lane()),
            )
        })?;
    if lane.occupant != Some(evidence.occupant()) {
        return Err(store_fault(
            "resident::occupant-moved",
            &format!(
                "lane {} expected {}, observed {:?}",
                evidence.lane(),
                evidence.occupant(),
                lane.occupant
            ),
        ));
    }
    lane.occupant = None;
    lane.disposition = match evidence {
        ProcessEvidence::Degraded { .. } => DEGRADED,
        ProcessEvidence::Drained { .. } | ProcessEvidence::Exited { .. } => VACANT,
    }
    .to_owned();
    Ok(())
}

fn commit_transition(
    bus: &Bus,
    epoch: CanonicalUuid,
    from: ResidentState,
    to: ResidentState,
    previous: u64,
) -> CommandResult<JournalCommit> {
    let observed = current_residency(bus, epoch)?;
    if observed.body().state != from {
        return Err(store_fault(
            "resident::state-moved",
            &format!("expected {from:?}, observed {:?}", observed.body().state),
        ));
    }
    let occupants = observed
        .body()
        .lanes
        .iter()
        .filter(|lane| lane.occupant.is_some())
        .count();
    let occupants = u32::try_from(occupants)
        .map_err(|error| store_fault("resident::occupant-overflow", &error.to_string()))?;
    let mut next = observed.body().clone();
    next.state = to;
    let mut write = write(Actor::Operator, epoch, Producer::CommandDoor)?;
    guard_residency(&mut write, &observed);
    let fact = transition_fact(&mut write, epoch, to, occupants, previous)?;
    write.residency_head(fact, move |_| next);
    commit(bus, write)
}

pub(super) fn guard_residency(
    write: &mut JournalWrite,
    observed: &journal::JournalHead<ResidencyHead>,
) {
    let expected = observed.clone();
    write.guard(JournalGuard::new(move |read| {
        let Some(current) = read.residency_head()? else {
            return Err(journal::GuardError::refused("resident::head-missing"));
        };
        if current != expected {
            return Err(journal::GuardError::refused("resident::head-moved"));
        }
        Ok(())
    }));
}

fn transition_fact(
    write: &mut JournalWrite,
    epoch: CanonicalUuid,
    to: ResidentState,
    occupants: u32,
    previous: u64,
) -> CommandResult<journal::PendingFact> {
    let cause = FactCause::Existing(seq(previous));
    match to {
        ResidentState::Quiescing => write.fact(wire::body::UpQuiescing { epoch }, cause),
        ResidentState::Draining => write.fact(wire::body::UpDraining { epoch, occupants }, cause),
        ResidentState::Terminating => {
            write.fact(wire::body::UpTerminating { epoch, occupants }, cause)
        }
        ResidentState::Killing => write.fact(wire::body::UpKilling { epoch, occupants }, cause),
        ResidentState::Verifying => write.fact(wire::body::UpVerifying { epoch }, cause),
        ResidentState::Dark => write.fact(wire::body::UpDark { epoch }, cause),
        ResidentState::Seated => {
            return Err(store_fault(
                "resident::ladder-invalid",
                "shutdown attempted to seat a second generation",
            ));
        }
    }
    .map_err(|error| store_fault("journal::write-invalid", &error.to_string()))
}

fn recognize_disk_residue(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    progress: &mut ShutdownProgress,
) -> CommandResult<()> {
    let mut evidence = degraded_evidence(&paths.degraded)?;
    evidence.extend(tape_evidence(&paths.tape)?);
    if evidence.is_empty() {
        return Ok(());
    }
    let committed = commit_disk_evidence(bus, epoch, &evidence, progress.previous)?;
    progress.remember(committed);
    for observed in evidence {
        if let Some(path) = observed.consume {
            remove_if_present(&path).map_err(|error| {
                environment_fault("resident::residue-unconsumed", &error.to_string())
            })?;
        }
    }
    Ok(())
}

struct DiskEvidence {
    lane: CanonicalUuid,
    marker: String,
    consume: Option<PathBuf>,
}

fn degraded_evidence(root: &Path) -> CommandResult<Vec<DiskEvidence>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(environment_fault(
                "resident::degraded-unreadable",
                &error.to_string(),
            ));
        }
    };
    let mut paths = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| environment_fault("resident::degraded-unreadable", &error.to_string()))?;
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let lane = path_lane(&path, "resident::degraded-invalid")?;
            let marker = fs::read_to_string(&path).map_err(|error| {
                environment_fault("resident::degraded-unreadable", &error.to_string())
            })?;
            if marker.is_empty() {
                return Err(environment_fault(
                    "resident::degraded-invalid",
                    &format!("{} is empty", path.display()),
                ));
            }
            Ok(DiskEvidence {
                lane,
                marker: format!("degraded: {}", marker.trim_end()),
                consume: Some(path),
            })
        })
        .collect()
}

fn tape_evidence(root: &Path) -> CommandResult<Vec<DiskEvidence>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(environment_fault(
                "resident::tape-unreadable",
                &error.to_string(),
            ));
        }
    };
    let mut paths = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| environment_fault("resident::tape-unreadable", &error.to_string()))?;
    paths.sort();
    let mut evidence = Vec::new();
    for path in paths {
        let lane = path_lane(&path, "resident::tape-invalid")?;
        let tape = match tape::Tape::open_existing(&path) {
            Ok(tape) => tape,
            Err(error) => {
                evidence.push(DiskEvidence {
                    lane,
                    marker: format!("tape open: {error}"),
                    consume: None,
                });
                continue;
            }
        };
        if let Err(error) = tape.seal() {
            evidence.push(DiskEvidence {
                lane,
                marker: format!("tape seal: {error}"),
                consume: None,
            });
        }
    }
    Ok(evidence)
}

fn path_lane(path: &Path, reason: &str) -> CommandResult<CanonicalUuid> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            environment_fault(
                reason,
                &format!("{} has no UTF-8 lane name", path.display()),
            )
        })?;
    name.parse()
        .map_err(|error| environment_fault(reason, &format!("{}: {error}", path.display())))
}

fn commit_disk_evidence(
    bus: &Bus,
    epoch: CanonicalUuid,
    evidence: &[DiskEvidence],
    previous: u64,
) -> CommandResult<JournalCommit> {
    let observed = current_residency(bus, epoch)?;
    let mut head = observed.body().clone();
    for observed in evidence {
        if let Some(lane) = head
            .lanes
            .iter_mut()
            .find(|lane| lane.lane == observed.lane)
        {
            lane.disposition = DEGRADED.to_owned();
        }
    }
    let mut write = write(Actor::Spice, epoch, Producer::ResidentRecognition)?;
    guard_residency(&mut write, &observed);
    let mut cause = FactCause::Existing(seq(previous));
    let mut last = None;
    for observed in evidence {
        let fact = write
            .fact(
                wire::body::LaneDegraded {
                    lane: observed.lane,
                    marker: observed.marker.clone(),
                },
                cause,
            )
            .map_err(|error| store_fault("journal::write-invalid", &error.to_string()))?;
        cause = FactCause::Pending(fact);
        last = Some(fact);
    }
    write.residency_head(
        last.expect("non-empty disk evidence has a last fact"),
        move |_| head,
    );
    commit(bus, write)
}
