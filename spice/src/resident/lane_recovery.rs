//! Predecessor lane import and atomic authority transfer.

use std::ffi::OsString;
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use bus::Bus;
use journal::{FactCause, JournalGuard, JournalWrite, PendingFact};
use lane::capture::{CaptureFailure, CaptureRecord, CaptureReportError, CaptureReporter};
use lane::occupant::{OccupantWait, Seats};
use sill::{
    Cancellation, Deadlines, Environment, Observation, RestartDisposition, Spec, WaitPolicy,
};
use spice::repository::Repository;
use wire::intent::{CommandAnswer, LaneChangeAnswer, LaneState};
use wire::{
    Actor, CanonicalUuid, Command, Outcome, Producer, Refusal, Reply, ReplyPosition, SeqRange,
};

use super::shutdown::{current_residency, guard_residency};
use super::{
    CommandResult, RuntimePaths, commit, new_uuid, seq, store_fault, typed_outcome, write,
};

const OCCUPIED: &str = "occupied";
const VACANT: &str = "vacant";
const CAPTURE_LIMIT: usize = 1024 * 1024;
const OUTPUT_DRAIN: Duration = Duration::from_secs(2);
const REAP: Duration = Duration::from_secs(2);
const TERMINATION_GRACE: Duration = Duration::from_secs(1);
const RECORD_QUEUE: usize = 64;
const SESSION_PROOF: Duration = Duration::from_secs(10);
const SESSION_SETTLE: Duration = Duration::from_millis(25);

pub(super) struct Runtime {
    capture: Arc<dyn CaptureReporter>,
}

impl Runtime {
    pub(super) const fn new(capture: Arc<dyn CaptureReporter>) -> Self {
        Self { capture }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SessionProofError {
    Missing,
    Capture(String),
    CrossLane(CanonicalUuid),
    CrossOccupant(CanonicalUuid),
    WrongStream,
    Mismatched { expected: String, observed: String },
    Duplicate(String),
}

impl SessionProofError {
    fn refusal(&self) -> Refusal {
        let (reason, expected, observed, remedy) = match self {
            Self::Missing => (
                "lane::session-missing",
                Some("one session declaration".into()),
                None,
                "retry after the vendor emits initialization",
            ),
            Self::Capture(detail) => (
                "lane::session-capture-failed",
                Some("intact selected-stream capture".into()),
                Some(detail.clone()),
                "repair capture and retry",
            ),
            Self::CrossLane(observed) => (
                "lane::session-cross-lane",
                None,
                Some(observed.to_string()),
                "use this lane's supervised declaration",
            ),
            Self::CrossOccupant(observed) => (
                "lane::session-cross-occupant",
                None,
                Some(observed.to_string()),
                "use the current occupant's declaration",
            ),
            Self::WrongStream => (
                "lane::session-wrong-stream",
                Some("vendor-selected stream".into()),
                None,
                "declare on the selected JSONL stream",
            ),
            Self::Mismatched { expected, observed } => (
                "lane::session-mismatched",
                Some(expected.clone()),
                Some(observed.clone()),
                "resume the exact durable identity",
            ),
            Self::Duplicate(identity) => (
                "lane::session-duplicate",
                Some("one declaration".into()),
                Some(identity.clone()),
                "emit initialization exactly once",
            ),
        };
        Refusal {
            reason: reason.into(),
            witness: Some("session-declaration".into()),
            expected,
            observed,
            remedy: remedy.into(),
        }
    }
}

struct DeclarationState {
    reply: mpsc::SyncSender<Result<wire::LaneSession, SessionProofError>>,
    seen: Option<String>,
}

struct DeclarationReporter {
    inner: Arc<dyn CaptureReporter>,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    worktree: String,
    vendor: wire::Name,
    epoch: CanonicalUuid,
    stream: vendor::RecordStream,
    expected: Option<String>,
    state: Mutex<DeclarationState>,
}

impl DeclarationReporter {
    fn reject(&self, error: SessionProofError) -> CaptureReportError {
        if let Ok(state) = self.state.lock() {
            let _sent = state.reply.try_send(Err(error.clone()));
        }
        CaptureReportError::new(format!("session declaration refused: {error:?}"))
    }

    fn declaration(&self, record: &CaptureRecord) -> Result<wire::LaneSession, CaptureReportError> {
        let declaration = record
            .declaration
            .as_ref()
            .expect("caller selected declaration");
        if record.lane != self.lane {
            return Err(self.reject(SessionProofError::CrossLane(record.lane)));
        }
        if record.occupant != self.occupant {
            return Err(self.reject(SessionProofError::CrossOccupant(record.occupant)));
        }
        if record.stream != self.stream {
            return Err(self.reject(SessionProofError::WrongStream));
        }
        let identity = declaration.session.as_str().to_owned();
        if self
            .expected
            .as_ref()
            .is_some_and(|expected| expected != &identity)
        {
            return Err(self.reject(SessionProofError::Mismatched {
                expected: self.expected.clone().expect("checked expected identity"),
                observed: identity,
            }));
        }
        Ok(wire::LaneSession {
            lane: self.lane,
            occupant: self.occupant,
            worktree: self.worktree.clone(),
            vendor: self.vendor.clone(),
            epoch: self.epoch,
            stream: record_stream(self.stream).into(),
            identity,
            locus: wire::TapeLocus {
                member: declaration.locus.member,
                offset: declaration.locus.offset,
                length: declaration.locus.length,
            },
        })
    }
}

impl CaptureReporter for DeclarationReporter {
    fn record(&self, record: CaptureRecord) -> Result<(), CaptureReportError> {
        if record.declaration.is_none() {
            return self.inner.record(record);
        }
        let session = self.declaration(&record)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| CaptureReportError::new("session proof lock is poisoned"))?;
        if let Some(identity) = &state.seen {
            let error = SessionProofError::Duplicate(identity.clone());
            let _sent = state.reply.try_send(Err(error.clone()));
            return Err(CaptureReportError::new(format!(
                "session declaration refused: {error:?}"
            )));
        }
        self.inner.record(record)?;
        state.seen = Some(session.identity.clone());
        state.reply.try_send(Ok(session)).map_err(|error| {
            CaptureReportError::new(format!(
                "session proof receiver refused declaration: {error}"
            ))
        })
    }

    fn failed(&self, failure: CaptureFailure) {
        if let Ok(state) = self.state.lock() {
            let _sent = state
                .reply
                .try_send(Err(SessionProofError::Capture(failure.failure.to_string())));
        }
        self.inner.failed(failure);
    }
}

struct ImportContext {
    observed: journal::JournalHead<wire::ResidencyHead>,
    row: wire::LaneResidency,
    worktree: String,
    ancestor: CanonicalUuid,
    resume: vendor::Resume,
    carried: Option<CarriedClaim>,
}

struct ProvenOccupant {
    occupant: CanonicalUuid,
    group: u32,
    session: wire::LaneSession,
}

#[derive(Clone)]
struct CarriedClaim {
    task: wire::TaskHandle,
    claim: journal::JournalHead<wire::ClaimHead>,
    lease: journal::JournalHead<wire::LeaseHead>,
}

pub(super) fn import(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    runtime: &Runtime,
    command: &wire::AdmittedCommand,
    request: &LaneState,
) -> Outcome<CommandAnswer> {
    if !matches!(command.actor(), Actor::Operator) {
        return lane_refusal(
            "command::authority",
            Some("operator".into()),
            Some(command.actor().to_string()),
            "reconnect through the operator capability",
        );
    }
    if request.action != wire::intent::LaneAction::Wake {
        return lane_refusal(
            "lane::state-shape",
            Some("wake action with successor worktree".into()),
            Some(format!("{:?}", request.action).to_lowercase()),
            "use lane import through its registered CLI contract",
        );
    }
    let context = match admit_import(bus, paths, epoch, seats, request) {
        Ok(context) => context,
        Err(outcome) => return typed_outcome(outcome),
    };
    let proven = match launch_import(paths, epoch, seats, runtime, &context) {
        Ok(proven) => proven,
        Err(outcome) => return typed_outcome(outcome),
    };
    let answer = LaneChangeAnswer {
        lane: request.lane,
        from: context.row.worktree.clone(),
        to: context.worktree.clone(),
    };
    let proposal = propose_import(command, epoch, request, &context, &proven, answer.clone());
    let proposal = match proposal {
        Ok(proposal) => proposal,
        Err(outcome) => {
            stop_lane(seats, request.lane);
            return typed_outcome(outcome);
        }
    };
    match commit(bus, proposal) {
        Ok(committed) => committed_reply(committed, answer),
        Err(outcome) => {
            stop_lane(seats, request.lane);
            typed_outcome(outcome)
        }
    }
}

fn admit_import(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    request: &LaneState,
) -> CommandResult<ImportContext> {
    let worktree = request.worktree.clone().ok_or_else(|| {
        lane_refusal_unit(
            "lane::import-worktree-missing",
            Some("one canonical linked worktree root".into()),
            None,
            "name the worktree receiving this vacant team seat",
        )
    })?;
    let observed = current_residency(bus, epoch)?;
    let row = observed
        .body()
        .lanes
        .iter()
        .find(|row| row.lane == request.lane)
        .cloned()
        .ok_or_else(|| {
            lane_refusal_unit(
                "lane::missing",
                Some("one current lane".into()),
                Some(request.lane.to_string()),
                "read lane and retry with a current lane identity",
            )
        })?;
    admit_row(paths, seats, observed.body(), &row, &worktree)?;
    super::steering::lane_capability(&row)?;
    let ancestor = row.lineage.last().copied().ok_or_else(|| {
        store_fault(
            "lane::lineage-missing",
            "a durable lane import has no predecessor occupant lineage",
        )
    })?;
    let resume = resume_session(paths, &row)?;
    let at = wire::Instant::new(jiff::Timestamp::now())
        .map_err(|error| store_fault("lane::clock-invalid", &error.to_string()))?;
    let carried = observe_carried_claim(bus, row.lane, ancestor, at)?;
    Ok(ImportContext {
        observed,
        row,
        worktree,
        ancestor,
        resume,
        carried,
    })
}

fn admit_row(
    paths: &RuntimePaths,
    seats: &Seats,
    residency: &wire::ResidencyHead,
    row: &wire::LaneResidency,
    worktree: &str,
) -> CommandResult<()> {
    if row.occupant.is_some() || seats.occupied(row.lane) {
        return Err(lane_refusal_unit(
            "lane::import-occupied",
            Some(VACANT.into()),
            Some(row.disposition.clone()),
            "drain the predecessor before importing",
        ));
    }
    if row.life == "drain" {
        return Err(lane_refusal_unit(
            "lane::life-ended",
            Some("steer or drive".into()),
            Some(row.life.clone()),
            "seat a new lane because drain life is terminal",
        ));
    }
    if worktree == row.worktree {
        return Err(lane_refusal_unit(
            "lane::import-unmoved",
            Some("a different linked worktree".into()),
            Some(worktree.into()),
            "use lane wake in the prior worktree",
        ));
    }
    if residency
        .lanes
        .iter()
        .any(|other| other.lane != row.lane && other.worktree == worktree)
    {
        return Err(lane_refusal_unit(
            "lane::worktree-occupied",
            Some("one lane per worktree".into()),
            Some(worktree.into()),
            "relocate the lane already bound there",
        ));
    }
    validate_worktree(paths, worktree)
}

fn launch_import(
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    runtime: &Runtime,
    context: &ImportContext,
) -> CommandResult<ProvenOccupant> {
    let occupant = new_uuid();
    let launch = occupant_launch(
        paths,
        OccupantLaunchRequest {
            worktree: &context.worktree,
            vendor: &context.row.vendor,
            epoch,
            lane: context.row.lane,
            occupant,
            resume: context.resume.clone(),
        },
        Arc::clone(&runtime.capture),
    )?;
    let receiver = launch.session;
    let group = seats
        .seat_observed(context.row.lane, occupant, launch.spec, launch.observation)
        .map_err(|error| store_fault("lane::import-launch-failed", &error.to_string()))?
        .group();
    let session = match await_session(receiver) {
        Ok(session) => session,
        Err(error) => {
            stop_lane(seats, context.row.lane);
            return Err(Outcome::Refusal(error.refusal()));
        }
    };
    Ok(ProvenOccupant {
        occupant,
        group,
        session,
    })
}

fn propose_import(
    command: &wire::AdmittedCommand,
    epoch: CanonicalUuid,
    request: &LaneState,
    context: &ImportContext,
    proven: &ProvenOccupant,
    answer: LaneChangeAnswer,
) -> CommandResult<JournalWrite> {
    let mut next = context.observed.body().clone();
    let row = next
        .lanes
        .iter_mut()
        .find(|row| row.lane == request.lane)
        .expect("observed imported lane remains in its cloned head");
    row.worktree.clone_from(&context.worktree);
    row.occupant = Some(proven.occupant);
    row.lineage.push(proven.occupant);
    row.session = Some(proven.session.clone());
    row.disposition = OCCUPIED.into();
    let mut proposal = write(
        command.actor().clone(),
        epoch,
        Producer::ResidentRecognition,
    )?;
    guard_residency(&mut proposal, &context.observed);
    if let Some(carried) = &context.carried {
        guard_carried_claim(&mut proposal, carried);
    }
    let started = import_facts(&mut proposal, epoch, context, proven)?;
    let head = match &context.carried {
        Some(carried) => append_claim_carry(
            &mut proposal,
            carried,
            context,
            proven.occupant,
            epoch,
            started,
        )?,
        None => started,
    };
    proposal.residency_head(head, move |_| next);
    command_result(&mut proposal, command, request.clone(), epoch, answer)?;
    Ok(proposal)
}

fn import_facts(
    write: &mut JournalWrite,
    epoch: CanonicalUuid,
    context: &ImportContext,
    proven: &ProvenOccupant,
) -> CommandResult<PendingFact> {
    let imported = write
        .fact(
            wire::body::LaneImported {
                lane: context.row.lane,
                ancestor: context.ancestor,
                from_worktree: context.row.worktree.clone(),
                worktree: context.worktree.clone(),
                vendor: context.row.vendor.clone(),
                identity: proven.session.identity.clone(),
                epoch,
            },
            FactCause::Existing(context.observed.position()),
        )
        .map_err(|error| store_fault("lane::import-fact-invalid", &error.to_string()))?;
    let declared = write
        .fact(session_fact(&proven.session), FactCause::Pending(imported))
        .map_err(|error| store_fault("lane::import-fact-invalid", &error.to_string()))?;
    write
        .fact(
            wire::body::LaneStarted {
                lane: context.row.lane,
                occupant: proven.occupant,
                epoch,
                group: proven.group,
            },
            FactCause::Pending(declared),
        )
        .map_err(|error| store_fault("lane::import-fact-invalid", &error.to_string()))
}

fn observe_carried_claim(
    bus: &Bus,
    lane: CanonicalUuid,
    ancestor: CanonicalUuid,
    at: wire::Instant,
) -> CommandResult<Option<CarriedClaim>> {
    let snapshot = bus
        .task_snapshot()
        .map_err(|error| store_fault("lane::claim-heads-unreadable", &error.to_string()))?;
    let mut carried = None;
    for heads in snapshot.tasks() {
        let Some(claim) = heads.claim() else { continue };
        if claim.body().disposition != wire::ClaimDisposition::Active {
            continue;
        }
        if claim.body().lane != lane && claim.body().actor != Actor::Lane(lane) {
            continue;
        }
        if carried.is_some() {
            return Err(store_fault(
                "lane::claim-ambiguous",
                "one lane owns more than one active claim",
            ));
        }
        let actor_bound = matches!(claim.body().actor, Actor::Operator)
            || claim.body().actor == Actor::Lane(lane);
        if !actor_bound || claim.body().lane != lane || claim.body().occupant != ancestor {
            return Err(store_fault(
                "lane::claim-lineage-moved",
                "active claim ownership differs from predecessor lineage",
            ));
        }
        let lease = heads.lease().ok_or_else(|| {
            store_fault(
                "lane::claim-lease-missing",
                "an imported active claim has no durable lease head",
            )
        })?;
        if lease.body().epoch != claim.body().epoch {
            return Err(store_fault(
                "lane::claim-epoch-mismatch",
                "claim and lease generations disagree",
            ));
        }
        if at < lease.body().start || at >= lease.body().expiry {
            return Err(lane_refusal_unit(
                "lane::claim-lease-stale",
                Some(format!(
                    "{} <= import instant < {}",
                    lease.body().start,
                    lease.body().expiry
                )),
                Some(at.to_string()),
                "reallocate an expired claim before importing its lane",
            ));
        }
        carried = Some(CarriedClaim {
            task: heads.statement().body().handle.clone(),
            claim: claim.clone(),
            lease: lease.clone(),
        });
    }
    Ok(carried)
}

fn guard_carried_claim(write: &mut JournalWrite, carried: &CarriedClaim) {
    let task = carried.task.clone();
    let claim = carried.claim.clone();
    let lease = carried.lease.clone();
    write.guard(JournalGuard::new(move |read| {
        if read.claim_head(&task)? != Some(claim.clone()) {
            return Err(journal::GuardError::refused("lane::claim-head-moved"));
        }
        if read.lease_head(&task)? != Some(lease.clone()) {
            return Err(journal::GuardError::refused("lane::lease-head-moved"));
        }
        Ok(())
    }));
}

fn append_claim_carry(
    write: &mut JournalWrite,
    carried: &CarriedClaim,
    context: &ImportContext,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
    cause: PendingFact,
) -> CommandResult<PendingFact> {
    let fact = write
        .fact(
            wire::body::ClaimCarried {
                task: carried.task.clone(),
                lane: context.row.lane,
                occupant,
                ancestor: context.ancestor,
                epoch,
            },
            FactCause::Pending(cause),
        )
        .map_err(|error| store_fault("lane::claim-carry-invalid", &error.to_string()))?;
    let mut claim = carried.claim.body().clone();
    claim.occupant = occupant;
    claim.epoch = epoch;
    write.claim_head(carried.task.clone(), fact, move |position| {
        claim.fact = position;
        claim
    });
    let mut lease = carried.lease.body().clone();
    lease.epoch = epoch;
    write.lease_head(carried.task.clone(), fact, move |_| lease);
    Ok(fact)
}

struct OccupantLaunch {
    spec: Spec,
    observation: Observation,
    session: mpsc::Receiver<Result<wire::LaneSession, SessionProofError>>,
}

struct OccupantLaunchRequest<'a> {
    worktree: &'a str,
    vendor: &'a wire::Name,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    resume: vendor::Resume,
}

fn occupant_launch(
    paths: &RuntimePaths,
    request: OccupantLaunchRequest<'_>,
    reporter: Arc<dyn CaptureReporter>,
) -> CommandResult<OccupantLaunch> {
    let repository = launch_repository(paths, request.worktree)?;
    let resolved = spice::config::load_repository(&repository, &[])
        .map_err(|error| configuration_refusal(&error))?;
    let plan = launch_plan(&repository, &resolved, &request)?;
    let record = plan
        .record()
        .map_err(|error| store_fault("lane::launch-invalid", &error.to_string()))?;
    let process = launch_process(
        &repository,
        &resolved,
        plan.environment().clone(),
        request.lane,
    )?;
    let spec = record.spec(&process);
    let stream = record.stream();
    let expected = record.resume().session_id().map(str::to_owned);
    let tape = tape::Tape::open(
        paths
            .tape
            .join(request.lane.to_string())
            .join(request.vendor.as_str()),
    )
    .map_err(|error| store_fault("lane::capture-tape-unavailable", &error.to_string()))?;
    let (reply, session) = mpsc::sync_channel(4);
    let reporter = declaration_reporter(reporter, &repository, &request, stream, expected, reply);
    let observation = lane::capture::observation(
        request.lane,
        request.occupant,
        record,
        tape,
        CAPTURE_LIMIT,
        RECORD_QUEUE,
        reporter,
    )
    .map_err(|error| store_fault("lane::capture-invalid", &error.to_string()))?;
    Ok(OccupantLaunch {
        spec,
        observation,
        session,
    })
}

fn launch_repository(paths: &RuntimePaths, worktree: &str) -> CommandResult<Repository> {
    let repository = Repository::discover(worktree)
        .map_err(|error| store_fault("lane::worktree-unresolved", &error.to_string()))?;
    if repository.common_dir() != paths.common || repository.worktree_root() != Path::new(worktree)
    {
        return Err(store_fault(
            "lane::worktree-moved",
            "durable lane worktree is not one current repository worktree root",
        ));
    }
    Ok(repository)
}

fn launch_plan(
    repository: &Repository,
    resolved: &spice::config::Resolved,
    request: &OccupantLaunchRequest<'_>,
) -> CommandResult<spice::agent_launch::Plan> {
    let executable = std::env::current_exe()
        .map_err(|error| store_fault("lane::executable-unresolved", &error.to_string()))?
        .display()
        .to_string();
    let environment = launch_environment()?;
    spice::agent_launch::compose(
        resolved.root(),
        spice::agent_launch::Inputs {
            worktree: repository.worktree_root().to_path_buf(),
            vendor: request.vendor.clone(),
            prompt: "[$spice](.agents/skills/spice/SKILL.md)".into(),
            executable,
            environment,
            lane: request.lane,
            resume: request.resume.clone(),
        },
    )
    .map_err(|error| match error {
        spice::agent_launch::ComposeError::UnsupportedVendor(other) => lane_refusal_unit(
            "lane::vendor-unsupported",
            Some("claude or codex".into()),
            Some(other),
            "select a vendor with a landed Rust launch dialect",
        ),
        other => store_fault("lane::launch-invalid", &other.to_string()),
    })
}

fn launch_environment() -> CommandResult<Environment> {
    let values = ["HOME", "PATH", "SHELL", "TMPDIR", "USER"]
        .into_iter()
        .filter_map(|key| crate::bootstrap_environment(key).map(|value| (key.into(), value)))
        .collect::<Vec<(OsString, OsString)>>();
    Environment::new(values)
        .map_err(|error| store_fault("lane::environment-invalid", &error.to_string()))
}

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

fn declaration_reporter(
    inner: Arc<dyn CaptureReporter>,
    repository: &Repository,
    request: &OccupantLaunchRequest<'_>,
    stream: vendor::RecordStream,
    expected: Option<String>,
    reply: mpsc::SyncSender<Result<wire::LaneSession, SessionProofError>>,
) -> Arc<dyn CaptureReporter> {
    Arc::new(DeclarationReporter {
        inner,
        lane: request.lane,
        occupant: request.occupant,
        worktree: repository.worktree_root().display().to_string(),
        vendor: request.vendor.clone(),
        epoch: request.epoch,
        stream,
        expected,
        state: Mutex::new(DeclarationState { reply, seen: None }),
    })
}

fn resume_session(
    paths: &RuntimePaths,
    row: &wire::LaneResidency,
) -> CommandResult<vendor::Resume> {
    let session = validate_session_head(row)?;
    let dialect = session_dialect(&row.vendor)?;
    prove_session_tape(paths, row, session, dialect)?;
    let identity = vendor::SessionId::new(session.identity.clone()).map_err(|error| {
        session_refusal(
            "lane::session-identity-invalid",
            Some("nonempty vendor identity".into()),
            Some(error.to_string()),
            "repair durable session authority",
        )
    })?;
    Ok(vendor::Resume::Session(identity))
}

fn validate_session_head(row: &wire::LaneResidency) -> CommandResult<&wire::LaneSession> {
    let session = row.session.as_ref().ok_or_else(|| {
        session_refusal(
            "lane::session-authority-missing",
            Some("current declared authority".into()),
            None,
            "drain and re-seat this legacy lane",
        )
    })?;
    if session.lane != row.lane || session.worktree != row.worktree || session.vendor != row.vendor
    {
        return Err(session_refusal(
            "lane::session-head-moved",
            Some("exact lane/worktree/vendor".into()),
            Some(format!(
                "{}/{}/{}",
                session.lane, session.worktree, session.vendor
            )),
            "restore the exact session head",
        ));
    }
    let ancestor = row.lineage.last().copied().ok_or_else(|| {
        session_refusal(
            "lane::session-lineage-missing",
            Some("latest predecessor".into()),
            None,
            "repair durable lane lineage",
        )
    })?;
    if session.occupant != ancestor || session.stream != "stdout" {
        return Err(session_refusal(
            "lane::session-authority-moved",
            Some(format!("{ancestor}/stdout")),
            Some(format!("{}/{}", session.occupant, session.stream)),
            "restore the latest selected-stream declaration",
        ));
    }
    Ok(session)
}

fn session_dialect(name: &wire::Name) -> CommandResult<listen::Dialect> {
    match name.as_str() {
        "claude" => Ok(listen::Dialect::Claude),
        "codex" => Ok(listen::Dialect::Codex),
        other => Err(session_refusal(
            "lane::session-vendor-unsupported",
            Some("claude or codex".into()),
            Some(other.into()),
            "select a current session dialect",
        )),
    }
}

fn prove_session_tape(
    paths: &RuntimePaths,
    row: &wire::LaneResidency,
    session: &wire::LaneSession,
    dialect: listen::Dialect,
) -> CommandResult<()> {
    let tape = tape::Tape::open_existing(
        paths
            .tape
            .join(row.lane.to_string())
            .join(row.vendor.as_str()),
    )
    .map_err(|error| {
        session_refusal(
            "lane::session-tape-moved",
            Some("exact durable tape".into()),
            Some(error.to_string()),
            "restore the preserved tape",
        )
    })?;
    let locus = listen::Locus::new(
        session.locus.member,
        session.locus.offset,
        session.locus.length,
    );
    let bytes = tape.read(locus).map_err(|error| {
        session_refusal(
            "lane::session-tape-moved",
            Some(locus.to_string()),
            Some(error.to_string()),
            "restore the exact declaration bytes",
        )
    })?;
    let decoded = listen::decode(dialect, &bytes, locus).map_err(|error| {
        session_refusal(
            "lane::session-tape-moved",
            Some(session.identity.clone()),
            Some(error.to_string()),
            "restore the exact declaration record",
        )
    })?;
    let declared = decoded.events().iter().find_map(|event| match event {
        listen::Event::Session(declaration) => Some(declaration.identity.as_str()),
        _ => None,
    });
    if declared != Some(session.identity.as_str()) {
        return Err(session_refusal(
            "lane::session-tape-moved",
            Some(session.identity.clone()),
            declared.map(str::to_owned),
            "restore the durable declaration locus",
        ));
    }
    Ok(())
}

fn await_session(
    receiver: mpsc::Receiver<Result<wire::LaneSession, SessionProofError>>,
) -> Result<wire::LaneSession, SessionProofError> {
    match receiver.recv_timeout(SESSION_PROOF) {
        Ok(Ok(session)) => match receiver.recv_timeout(SESSION_SETTLE) {
            Ok(Err(error)) => Err(error),
            Ok(Ok(duplicate)) => Err(SessionProofError::Duplicate(duplicate.identity)),
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
                Ok(session)
            }
        },
        Ok(Err(error)) => Err(error),
        Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
            Err(SessionProofError::Missing)
        }
    }
}

fn stop_lane(seats: &mut Seats, lane: CanonicalUuid) {
    let Some(occupant) = seats.vacate(lane) else {
        return;
    };
    let occupant = match occupant.wait_for(Duration::ZERO) {
        Ok(OccupantWait::Running(occupant)) => occupant,
        Ok(OccupantWait::Exited { .. }) | Err(_) => return,
    };
    if occupant.terminate().is_err() {
        return;
    }
    let grace = occupant.termination_grace();
    let occupant = match occupant.wait_for(grace) {
        Ok(OccupantWait::Running(occupant)) => occupant,
        Ok(OccupantWait::Exited { .. }) | Err(_) => return,
    };
    if occupant.kill().is_ok() {
        let reap = occupant.reap_bound();
        let _waited = occupant.wait_for(reap);
    }
}

fn validate_worktree(paths: &RuntimePaths, worktree: &str) -> CommandResult<()> {
    let repository = Repository::discover(worktree)
        .map_err(|error| store_fault("lane::worktree-unresolved", &error.to_string()))?;
    if repository.common_dir() != paths.common {
        return Err(lane_refusal_unit(
            "lane::foreign-worktree",
            Some(paths.common.display().to_string()),
            Some(repository.common_dir().display().to_string()),
            "choose a resident repository worktree",
        ));
    }
    if repository.worktree_root() != Path::new(worktree) {
        return Err(lane_refusal_unit(
            "lane::worktree-not-root",
            Some(repository.worktree_root().display().to_string()),
            Some(worktree.into()),
            "name the canonical worktree root",
        ));
    }
    Ok(())
}

fn session_fact(session: &wire::LaneSession) -> wire::body::LaneSessionDeclared {
    wire::body::LaneSessionDeclared {
        lane: session.lane,
        occupant: session.occupant,
        worktree: session.worktree.clone(),
        vendor: session.vendor.clone(),
        epoch: session.epoch,
        stream: session.stream.clone(),
        identity: session.identity.clone(),
        member: session.locus.member,
        offset: session.locus.offset,
        length: session.locus.length,
    }
}

fn command_result<B: serde::Serialize>(
    write: &mut JournalWrite,
    admitted: &wire::AdmittedCommand,
    body: B,
    epoch: CanonicalUuid,
    answer: LaneChangeAnswer,
) -> CommandResult<()> {
    write
        .command_result(
            Command {
                v: wire::WireVersion::CURRENT,
                id: admitted.id(),
                epoch: admitted.epoch(),
                kind: admitted.kind(),
                expected: admitted.expected().to_vec(),
                body,
            },
            epoch,
            answer,
        )
        .map_err(|error| store_fault("lane::receipt-invalid", &error.to_string()))
}

fn committed_reply(
    committed: bus::JournalCommit,
    answer: LaneChangeAnswer,
) -> Outcome<CommandAnswer> {
    Outcome::Reply(Reply {
        position: ReplyPosition::Committed(SeqRange {
            first: seq(committed.first()),
            last: seq(committed.last()),
        }),
        body: CommandAnswer::LaneChange(answer),
    })
}

fn configuration_refusal(error: &spice::config::LoadError) -> Outcome<()> {
    lane_refusal_unit(
        "lane::configuration-refused",
        Some("admitted repository configuration".into()),
        Some(error.diagnostic()),
        "correct the named configuration inputs",
    )
}

fn session_refusal(
    reason: &str,
    expected: Option<String>,
    observed: Option<String>,
    remedy: &str,
) -> Outcome<()> {
    Outcome::Refusal(Refusal {
        reason: reason.into(),
        witness: Some("lane-session-head".into()),
        expected,
        observed,
        remedy: remedy.into(),
    })
}

fn lane_refusal_unit(
    reason: &str,
    expected: Option<String>,
    observed: Option<String>,
    remedy: &str,
) -> Outcome<()> {
    Outcome::Refusal(Refusal {
        reason: reason.into(),
        witness: Some("lane".into()),
        expected,
        observed,
        remedy: remedy.into(),
    })
}

fn lane_refusal(
    reason: &str,
    expected: Option<String>,
    observed: Option<String>,
    remedy: &str,
) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal {
        reason: reason.into(),
        witness: Some("lane".into()),
        expected,
        observed,
        remedy: remedy.into(),
    })
}

fn record_stream(stream: vendor::RecordStream) -> &'static str {
    match stream {
        vendor::RecordStream::Stdout => "stdout",
        vendor::RecordStream::Stderr => "stderr",
    }
}

#[cfg(test)]
#[path = "lane_recovery_tests.rs"]
mod tests;
