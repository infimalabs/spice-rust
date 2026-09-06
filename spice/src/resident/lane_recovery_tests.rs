use std::fs;
use std::sync::{Arc, Mutex, mpsc};

use lane::capture::{CaptureFailure, CaptureRecord, CaptureReportError};
use sill::{Cancellation, Deadlines, Environment, RestartDisposition, Spec, WaitPolicy};

use super::*;
use crate::resident::lane_surface::{
    DeclarationReporter, DeclarationState, DeclarationWitness, SessionProof,
};

static NEXT_FIXTURE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct Fixture {
    root: std::path::PathBuf,
    bus: Bus,
    old_epoch: CanonicalUuid,
    new_epoch: CanonicalUuid,
    lane: CanonicalUuid,
    ancestor: CanonicalUuid,
    successor: CanonicalUuid,
    task: wire::TaskStamp,
    at: wire::Instant,
    expiry: wire::Instant,
    maximum: wire::Instant,
}

fn uuid(value: u64) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-{value:012x}")
        .parse()
        .expect("fixture UUID")
}

fn fixture() -> Fixture {
    let fixture = NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root =
        std::env::temp_dir().join(format!("spice-lane-carry-{}-{fixture}", std::process::id()));
    std::fs::create_dir_all(&root).expect("fixture directory");
    let journal =
        journal::Journal::open(root.join("authority.sqlite3")).expect("fixture journal opens");
    Fixture {
        root,
        bus: Bus::start(journal).expect("fixture bus starts"),
        old_epoch: uuid(1),
        new_epoch: uuid(2),
        lane: uuid(3),
        ancestor: uuid(4),
        successor: uuid(5),
        task: "1kKRRsDP".parse().expect("fixture task"),
        at: "2026-08-01T06:00:00.000Z".parse().expect("fixture instant"),
        expiry: "2030-08-01T07:00:00.000Z".parse().expect("fixture expiry"),
        maximum: "2030-08-01T08:00:00.000Z".parse().expect("fixture maximum"),
    }
}

const PREDECESSOR: &str = "/worktrees/predecessor";
const SUCCESSOR: &str = "/worktrees/successor";
const PRE_EXTENSION_RESIDENCY: &[u8] = concat!(
    r#"{"epoch":"018f82d4-6f59-7d06-92e3-000000000001","state":"seated","topology_revision":1,"lanes":[{"#,
    r#""lane":"018f82d4-6f59-7d06-92e3-000000000003","#,
    r#""lineage_id":"018f82d4-6f59-7d06-92e3-000000000003","name":"Ava","#,
    r#""worktree":"/worktrees/predecessor","life":"drive","vendor":"claude","#,
    r#""occupant":"018f82d4-6f59-7d06-92e3-000000000004","#,
    r#""lineage":["018f82d4-6f59-7d06-92e3-000000000004"],"#,
    r#""capabilities":["018f82d4-6f59-7d06-92e3-000000000006"],"session":{"#,
    r#""lane":"018f82d4-6f59-7d06-92e3-000000000003","#,
    r#""occupant":"018f82d4-6f59-7d06-92e3-000000000004","#,
    r#""worktree":"/worktrees/predecessor","vendor":"claude","#,
    r#""epoch":"018f82d4-6f59-7d06-92e3-000000000001","stream":"stdout","#,
    r#""identity":"pre-extension-session","#,
    r#""locus":{"member":7,"offset":31,"length":93}},"disposition":"occupied"}]}"#,
)
.as_bytes();

fn occupied_row(fixture: &Fixture) -> wire::LaneResidency {
    wire::LaneResidency {
        lane: fixture.lane,
        lineage_id: fixture.lane,
        name: Some("Ava".to_owned()),
        worktree: PREDECESSOR.into(),
        life: "drive".into(),
        vendor: "claude".parse().expect("fixture vendor"),
        occupant: Some(fixture.ancestor),
        lineage: vec![fixture.ancestor],
        capabilities: vec![uuid(6)],
        session: None,
        disposition: OCCUPIED.into(),
    }
}

fn vacant_row(fixture: &Fixture) -> wire::LaneResidency {
    wire::LaneResidency {
        occupant: None,
        disposition: VACANT.into(),
        ..occupied_row(fixture)
    }
}

fn seed_residency(fixture: &Fixture, seed: &mut JournalWrite, lanes: Vec<wire::LaneResidency>) {
    let seated = seed
        .fact(
            wire::body::UpSeated {
                epoch: fixture.old_epoch,
            },
            FactCause::Root,
        )
        .expect("resident fact");
    let epoch = fixture.old_epoch;
    seed.residency_head(seated, move |_| wire::ResidencyHead {
        epoch,
        state: wire::ResidentState::Seated,
        topology_revision: 0,
        lanes,
        subscriptions: Vec::new(),
        renewals: Vec::new(),
    });
}

fn seed_task(fixture: &Fixture, seed: &mut JournalWrite) -> PendingFact {
    let added = seed
        .fact(
            wire::body::TaskAdded {
                task: fixture.task.clone(),
                title: "Carry active work".into(),
                description: None,
                acceptance: Vec::new(),
                project: "rim.recovery".into(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: vec!["todo".parse().expect("fixture phase")],
                phase: "todo".parse().expect("fixture phase"),
                creation: wire::body::TaskCreationContext::unavailable(
                    wire::body::TaskCreationUnavailable::SpiceActor,
                ),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("task fact");
    let task = fixture.task.clone();
    seed.task_head(added, move |position| wire::TaskHead {
        born: task,
        queue_origin: position,
        title: "Carry active work".into(),
        description: None,
        acceptance: Vec::new(),
        project: "rim.recovery".into(),
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: "resident:018f82d4-6f59-7d06-92e3-000000000001"
            .parse()
            .expect("fixture origin"),
        creation: wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::SpiceActor,
        ),
        revision: position,
    });
    let task = fixture.task.clone();
    seed.workflow_head(task, added, move |_| wire::WorkflowHead {
        flow: vec!["todo".parse().expect("fixture phase")],
        phase: "todo".parse().expect("fixture phase"),
        status: "pending".into(),
        review: "unassigned".into(),
        assignment: None,
        deferral: None,
        completion: None,
    });
    added
}

fn seed_claim(fixture: &Fixture, seed: &mut JournalWrite, added: PendingFact) {
    let won = seed
        .fact(
            wire::body::ClaimWon {
                task: fixture.task.clone(),
                actor: Actor::Operator,
                lane: fixture.lane,
                occupant: fixture.ancestor,
                epoch: fixture.old_epoch,
                cell: Some(wire::ClaimCell {
                    project: "rim.recovery".into(),
                    phase: "todo".parse().expect("fixture phase"),
                }),
                until: fixture.expiry,
                git: None,
                recovery: None,
                source: wire::body::SourceBoundary::Proven { heads: Vec::new() },
            },
            FactCause::Pending(added),
        )
        .expect("claim fact");
    let task = fixture.task.clone();
    let lane = fixture.lane;
    let occupant = fixture.ancestor;
    let epoch = fixture.old_epoch;
    seed.claim_head(task, won, move |fact| wire::ClaimHead {
        actor: Actor::Operator,
        fact,
        claimed: fact,
        cell: wire::ClaimCell {
            project: "rim.recovery".into(),
            phase: "todo".parse().expect("fixture phase"),
        },
        lane,
        occupant,
        epoch,
        recovery: None,
        disposition: wire::ClaimDisposition::Active,
    });
    let task = fixture.task.clone();
    let at = fixture.at;
    let maximum = fixture.maximum;
    let expiry = fixture.expiry;
    seed.lease_head(task, won, move |_| wire::LeaseHead {
        start: at,
        renewal: at,
        maximum,
        expiry,
        epoch,
    });
}

fn seed(fixture: &Fixture) {
    seed_lanes(fixture, vec![occupied_row(fixture)]);
}

fn seed_lanes(fixture: &Fixture, lanes: Vec<wire::LaneResidency>) {
    let origin: wire::Origin = format!("resident:{}", fixture.old_epoch)
        .parse()
        .expect("fixture origin");
    let mut seed = JournalWrite::new(Actor::Operator, origin, fixture.at, Producer::CommandDoor);
    seed_residency(fixture, &mut seed, lanes);
    let added = seed_task(fixture, &mut seed);
    seed_claim(fixture, &mut seed, added);
    assert!(matches!(
        fixture.bus.commit(seed).expect("seed reaches writer"),
        bus::CommitOutcome::Committed(_)
    ));
}

fn finish(fixture: Fixture) {
    fixture.bus.shutdown().expect("fixture bus stops");
    std::fs::remove_dir_all(&fixture.root).expect("fixture cleanup");
}

#[test]
fn authority_reopens_a_session_whose_launch_identity_was_not_recorded() {
    let fixture = fixture();
    seed(&fixture);
    let root = fixture.root.clone();
    fixture.bus.shutdown().expect("the first authority closes");
    let connection = rusqlite::Connection::open(root.join("authority.sqlite3"))
        .expect("the seeded authority opens for literal fixture injection");
    let updated = connection
        .execute(
            "UPDATE residency_head SET body = ?1 WHERE singleton = 1",
            rusqlite::params![PRE_EXTENSION_RESIDENCY],
        )
        .expect("the literal pre-extension head replaces the normalized fixture");
    assert_eq!(updated, 1);
    drop(connection);

    let journal = journal::Journal::open(root.join("authority.sqlite3"))
        .expect("the current journal reopens");
    let bus = Bus::start(journal).expect("the current authority reopens");
    let head = bus
        .residency_head()
        .expect("the reopened residency reads")
        .expect("the reopened residency exists");
    let session = head.body().lanes[0]
        .session
        .as_ref()
        .expect("session authority");
    assert_eq!(session.identity, "pre-extension-session");
    assert_eq!(session.launch, None);

    bus.shutdown().expect("the reopened authority closes");
    std::fs::remove_dir_all(root).expect("fixture cleanup");
}

fn reseat(fixture: &Fixture) {
    let old = fixture
        .bus
        .residency_head()
        .expect("residency reads")
        .expect("residency exists");
    let mut next = old.body().clone();
    next.epoch = fixture.new_epoch;
    let origin: wire::Origin = format!("resident:{}", fixture.new_epoch)
        .parse()
        .expect("fixture origin");
    let mut write = JournalWrite::new(Actor::Spice, origin, fixture.at, Producer::CommandDoor);
    let fact = write
        .fact(
            wire::body::UpSeated {
                epoch: fixture.new_epoch,
            },
            FactCause::Root,
        )
        .expect("new generation fact");
    write.residency_head(fact, move |_| next);
    assert!(matches!(
        fixture.bus.commit(write).expect("reseat reaches writer"),
        bus::CommitOutcome::Committed(_)
    ));
}

fn carry(fixture: &Fixture) {
    let carried = observe_carried_claim(&fixture.bus, fixture.lane, fixture.ancestor, fixture.at)
        .expect("claim observation")
        .expect("operator claim is lane-bound");
    let observed = current_residency(&fixture.bus, fixture.new_epoch).expect("current residency");
    let row = observed.body().lanes[0].clone();
    let context = ImportContext {
        observed: observed.clone(),
        row,
        worktree: "/worktrees/successor".into(),
        ancestor: fixture.ancestor,
        resume: vendor::Resume::Fresh,
        carried: Some(carried.clone()),
    };
    let mut next = observed.body().clone();
    next.lanes[0].occupant = Some(fixture.successor);
    next.lanes[0].lineage.push(fixture.successor);
    let mut write = super::write(
        Actor::Spice,
        fixture.new_epoch,
        Producer::ResidentRecognition,
    )
    .expect("carry proposal");
    guard_residency(&mut write, &observed);
    guard_carried_claim(&mut write, &carried);
    let renewed = write
        .fact(
            wire::body::LaneRenewed {
                lane: fixture.lane,
                occupant: fixture.successor,
                ancestor: fixture.ancestor,
                epoch: fixture.new_epoch,
                group: 42,
            },
            FactCause::Existing(observed.position()),
        )
        .expect("renewal fact");
    let fact = append_claim_carry(
        &mut write,
        &carried,
        &context,
        fixture.successor,
        fixture.new_epoch,
        renewed,
    )
    .expect("claim carry appends");
    write.residency_head(fact, move |_| next);
    commit(&fixture.bus, write).expect("carry commits atomically");
}

fn assert_carried(fixture: &Fixture) {
    let snapshot = fixture.bus.task_snapshot().expect("carried heads read");
    let heads = snapshot.tasks().first().expect("fixture task remains");
    let claim = heads.claim().expect("claim remains active");
    let lease = heads.lease().expect("lease remains present");
    let residency = snapshot.residency().expect("residency remains present");
    assert_eq!(claim.position(), lease.position());
    assert_eq!(claim.position(), residency.position());
    assert_eq!(claim.body().actor, Actor::Operator);
    assert_eq!(claim.body().occupant, fixture.successor);
    assert_eq!(claim.body().epoch, fixture.new_epoch);
    assert_eq!(lease.body().epoch, fixture.new_epoch);
    assert_eq!(lease.body().expiry, fixture.expiry);
    assert_eq!(lease.body().maximum, fixture.maximum);
    assert!(
        fixture
            .bus
            .fact_snapshot()
            .expect("facts read")
            .facts()
            .iter()
            .any(|fact| fact.fact.kind() == wire::FactKind::ClaimCarried)
    );
    fixture.bus.verify().expect("carried authority verifies");
}

#[test]
fn successor_carry_moves_operator_claim_and_lease_authority_together() {
    let fixture = fixture();
    seed(&fixture);
    reseat(&fixture);
    carry(&fixture);
    assert_carried(&fixture);
    finish(fixture);
}

#[test]
fn import_refuses_an_active_head_whose_lease_is_no_longer_live() {
    let fixture = fixture();
    seed(&fixture);

    assert!(matches!(
        observe_carried_claim(
            &fixture.bus,
            fixture.lane,
            fixture.ancestor,
            fixture.expiry,
        ),
        Err(Outcome::Refusal(ref refusal))
            if refusal.reason() == "lane::claim-lease-stale"
    ));

    finish(fixture);
}

// Import admission: every reason that keeps stale predecessor state from
// becoming authoritative, each proven by the exact refusal it must raise.

fn runtime_paths(fixture: &Fixture, common: std::path::PathBuf) -> RuntimePaths {
    RuntimePaths {
        worktree: fixture.root.clone(),
        common,
        runtime: fixture.root.join("runtime"),
        data: fixture.root.join("data"),
        journal: fixture.root.join("authority.sqlite3"),
        resident: fixture.root.join("resident"),
        canvas: fixture.root.join("canvas"),
        lane_canvas: fixture.root.join("lane-canvas"),
        inbox: fixture.root.join("inbox"),
        legacy_inbox: fixture.root.join("inbox"),
        degraded: fixture.root.join("degraded"),
        tape: fixture.root.join("tape"),
        stage: fixture.root.join("stage"),
    }
}

/// A fixture-owned worktree for admissions that reach [`validate_worktree`].
///
/// The mutation gate runs workspace tests from a materialized source snapshot,
/// not a Git checkout, so no fixture may borrow repository identity from the
/// directory this crate happened to be compiled in.
fn real_worktree(fixture: &Fixture) -> (String, std::path::PathBuf) {
    fs::create_dir(fixture.root.join(".git")).expect("the fixture Git directory creates");
    let repository = Repository::discover(&fixture.root).expect("the fixture is a repository");
    (
        repository.worktree_root().display().to_string(),
        repository.common_dir().to_path_buf(),
    )
}

fn refusal_reason<T>(result: CommandResult<T>) -> String {
    match result {
        Err(Outcome::Refusal(refusal)) => refusal.reason().to_owned(),
        Err(Outcome::Fault(fault)) => fault.reason,
        Err(Outcome::Reply(_)) | Ok(_) => panic!("import admitted a lane it must refuse"),
    }
}

fn wake(lane: CanonicalUuid, worktree: Option<&str>) -> LaneState {
    LaneState {
        lane,
        action: wire::intent::LaneAction::Wake,
        worktree: worktree.map(str::to_owned),
    }
}

fn admit_row_reason(
    fixture: &Fixture,
    lanes: &[wire::LaneResidency],
    row: &wire::LaneResidency,
    worktree: &str,
) -> String {
    let paths = runtime_paths(fixture, fixture.root.clone());
    let residency = wire::ResidencyHead {
        epoch: fixture.old_epoch,
        state: wire::ResidentState::Seated,
        topology_revision: 0,
        lanes: lanes.to_vec(),
        subscriptions: Vec::new(),
        renewals: Vec::new(),
    };
    refusal_reason(admit_row(&paths, &Seats::new(), &residency, row, worktree))
}

#[test]
fn import_refuses_a_wake_that_names_no_successor_worktree() {
    let fixture = fixture();
    seed(&fixture);
    let paths = runtime_paths(&fixture, fixture.root.clone());
    assert_eq!(
        refusal_reason(admit_import(
            &fixture.bus,
            &paths,
            fixture.old_epoch,
            &Seats::new(),
            &wake(fixture.lane, None),
        )),
        "lane::import-worktree-missing"
    );
    finish(fixture);
}

/// The command surface routes any worktree-bearing lane state here, so a drain
/// carrying a worktree reaches import and must be refused on shape.
#[test]
fn import_refuses_a_worktree_bearing_action_that_is_not_wake() {
    let fixture = fixture();
    let paths = runtime_paths(&fixture, fixture.root.clone());
    let request = LaneState {
        lane: fixture.lane,
        action: wire::intent::LaneAction::Drain,
        worktree: Some(SUCCESSOR.into()),
    };
    let outcome = import(
        &fixture.bus,
        &paths,
        fixture.old_epoch,
        &mut Seats::new(),
        &Runtime::new(Arc::new(SilentReporter)),
        &admitted(&fixture, &request),
        &request,
    );
    match outcome {
        Outcome::Refusal(refusal) => assert_eq!(refusal.reason(), "lane::state-shape"),
        Outcome::Reply(_) | Outcome::Fault(_) => {
            panic!("import admitted a drain action carrying a successor worktree")
        }
    }
    finish(fixture);
}

#[test]
fn import_refuses_a_lane_absent_from_the_current_residency() {
    let fixture = fixture();
    seed(&fixture);
    let paths = runtime_paths(&fixture, fixture.root.clone());
    assert_eq!(
        refusal_reason(admit_import(
            &fixture.bus,
            &paths,
            fixture.old_epoch,
            &Seats::new(),
            &wake(uuid(9), Some(SUCCESSOR)),
        )),
        "lane::missing"
    );
    finish(fixture);
}

#[test]
fn import_refuses_a_durable_lane_carrying_no_predecessor_lineage() {
    let fixture = fixture();
    let (worktree, common) = real_worktree(&fixture);
    seed_lanes(
        &fixture,
        vec![wire::LaneResidency {
            lineage: Vec::new(),
            ..vacant_row(&fixture)
        }],
    );
    let paths = runtime_paths(&fixture, common);
    assert_eq!(
        refusal_reason(admit_import(
            &fixture.bus,
            &paths,
            fixture.old_epoch,
            &Seats::new(),
            &wake(fixture.lane, Some(&worktree)),
        )),
        "lane::lineage-missing"
    );
    finish(fixture);
}

/// Import resumes through the one durable-session chain the seat and recall
/// surfaces already use, so it refuses every head that chain refuses. Which
/// heads those are is proven once, beside that chain.
#[test]
fn import_refuses_a_lane_whose_durable_session_head_no_longer_holds() {
    let fixture = fixture();
    let (worktree, common) = real_worktree(&fixture);
    seed_lanes(&fixture, vec![vacant_row(&fixture)]);
    let paths = runtime_paths(&fixture, common);
    assert_eq!(
        refusal_reason(admit_import(
            &fixture.bus,
            &paths,
            fixture.old_epoch,
            &Seats::new(),
            &wake(fixture.lane, Some(&worktree)),
        )),
        "lane::session-authority-missing"
    );
    finish(fixture);
}

#[test]
fn import_refuses_a_lane_whose_predecessor_still_holds_the_seat() {
    let fixture = fixture();
    let row = occupied_row(&fixture);
    assert_eq!(
        admit_row_reason(&fixture, std::slice::from_ref(&row), &row, SUCCESSOR),
        "lane::import-occupied"
    );
    finish(fixture);
}

/// A spec the seat door will open, for a process that ends on its own at once.
fn sleeping_spec() -> Spec {
    Spec {
        argv: sill::Argv::new("/bin/sleep", ["0"]).expect("fixture argv"),
        cwd: std::path::PathBuf::from("/"),
        environment: Environment::new([("PATH", "/usr/bin:/bin")]).expect("fixture environment"),
        group: sill::ProcessGroup::New,
        deadlines: Deadlines::new(
            Duration::from_secs(10),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect("fixture deadlines"),
        wait: WaitPolicy::new("lane", Cancellation::Never, RestartDisposition::Never)
            .expect("fixture wait policy"),
        output: sill::Output::new(
            sill::StdinPolicy::Null,
            sill::StreamPolicy::Null,
            sill::StreamPolicy::Null,
        ),
        redaction: sill::Redaction::None,
        grace: Duration::from_secs(1),
    }
}

/// The other door onto the same refusal: a durably vacant row is still refused
/// while a live occupant sits in that lane, which the row alone cannot show.
#[test]
fn import_refuses_a_vacant_row_whose_lane_seat_is_still_live() {
    let fixture = fixture();
    let row = vacant_row(&fixture);
    let paths = runtime_paths(&fixture, fixture.root.clone());
    let residency = wire::ResidencyHead {
        epoch: fixture.old_epoch,
        state: wire::ResidentState::Seated,
        topology_revision: 0,
        lanes: vec![row.clone()],
        subscriptions: Vec::new(),
        renewals: Vec::new(),
    };
    let mut seats = Seats::new();
    seats
        .seat(fixture.lane, fixture.successor, sleeping_spec())
        .expect("the seat door opens");
    assert_eq!(
        refusal_reason(admit_row(&paths, &seats, &residency, &row, SUCCESSOR)),
        "lane::import-occupied"
    );
    seats
        .vacate(fixture.lane)
        .expect("the seat was occupied")
        .wait()
        .expect("the supervised group ends");
    finish(fixture);
}

#[test]
fn explicit_import_admits_an_already_seated_drain_lane() {
    let fixture = fixture();
    let row = wire::LaneResidency {
        life: "drain".into(),
        ..vacant_row(&fixture)
    };
    let (successor, common) = real_worktree(&fixture);
    let paths = runtime_paths(&fixture, common);
    let residency = wire::ResidencyHead {
        epoch: fixture.old_epoch,
        state: wire::ResidentState::Seated,
        topology_revision: 0,
        lanes: vec![row.clone()],
        subscriptions: Vec::new(),
        renewals: Vec::new(),
    };

    admit_row(&paths, &Seats::new(), &residency, &row, &successor)
        .expect("operator import may relocate an already seated Drain lane");
    finish(fixture);
}

#[test]
fn watchband_drain_does_not_end_a_drive_lanes_import_life() {
    let fixture = fixture();
    let row = vacant_row(&fixture);
    let (successor, common) = real_worktree(&fixture);
    let paths = runtime_paths(&fixture, common);
    let residency = wire::ResidencyHead {
        epoch: fixture.old_epoch,
        state: wire::ResidentState::Seated,
        topology_revision: 0,
        lanes: vec![row.clone()],
        subscriptions: vec![wire::LineageSubscription {
            lineage_id: row.lineage_id,
            policy: wire::intent::LineageLife::Drain,
            manual_projects: Vec::new(),
            auto_projects: Vec::new(),
        }],
        renewals: Vec::new(),
    };

    admit_row(&paths, &Seats::new(), &residency, &row, &successor)
        .expect("task-subscription Drain leaves the drive lane importable");
    finish(fixture);
}

#[test]
fn import_refuses_a_worktree_identical_to_the_predecessor_root() {
    let fixture = fixture();
    let row = vacant_row(&fixture);
    assert_eq!(
        admit_row_reason(&fixture, std::slice::from_ref(&row), &row, PREDECESSOR),
        "lane::import-unmoved"
    );
    finish(fixture);
}

#[test]
fn import_refuses_a_worktree_already_bound_to_another_lane() {
    let fixture = fixture();
    let row = vacant_row(&fixture);
    let neighbour = wire::LaneResidency {
        lane: uuid(9),
        lineage_id: uuid(9),
        worktree: SUCCESSOR.into(),
        ..vacant_row(&fixture)
    };
    assert_eq!(
        admit_row_reason(&fixture, &[row.clone(), neighbour], &row, SUCCESSOR),
        "lane::worktree-occupied"
    );
    finish(fixture);
}

// Session proof: the successor is only authoritative once it declares exactly
// one identity, on the selected stream, for this lane and this occupant.

struct SilentReporter;

impl CaptureReporter for SilentReporter {
    fn record(&self, _record: CaptureRecord) -> Result<(), CaptureReportError> {
        Ok(())
    }

    fn failed(&self, _failure: CaptureFailure) {}
}

struct Proof {
    reporter: DeclarationReporter,
    session: SessionProof,
}

fn proof(expected: Option<&str>) -> Proof {
    let (reply, session) = mpsc::sync_channel(4);
    let state = Arc::new(Mutex::new(DeclarationState {
        reply,
        seen: None,
        witness: DeclarationWitness::default(),
    }));
    Proof {
        reporter: DeclarationReporter {
            inner: Arc::new(SilentReporter),
            lane: uuid(3),
            occupant: uuid(5),
            worktree: SUCCESSOR.into(),
            vendor: "claude".parse().expect("fixture vendor"),
            model: "sonnet".into(),
            effort: "high".into(),
            git: "current".into(),
            rtk: wire::body::RtkHealth::new(
                wire::body::RtkHealthState::Active,
                Some("0.45.0".to_owned()),
                "fixture active",
            ),
            epoch: uuid(2),
            stream: vendor::RecordStream::Stdout,
            expected: expected.map(str::to_owned),
            state: Arc::clone(&state),
        },
        session: SessionProof {
            session,
            state,
            // Every declaration and witness in these unit proofs is recorded
            // synchronously before `await_session`; no producer remains to
            // schedule. A zero horizon reaches the missing-declaration arm
            // deterministically without borrowing production's wait budget.
            horizon: std::time::Duration::ZERO,
        },
    }
}

fn locus() -> listen::Locus {
    listen::Locus::new(0, 0, 64)
}

fn declared(identity: &str) -> CaptureRecord {
    CaptureRecord {
        lane: uuid(3),
        occupant: uuid(5),
        stream: vendor::RecordStream::Stdout,
        boundary: sill::RecordBoundary::Complete,
        locus: locus(),
        declaration: Some(vendor::SessionDeclaration {
            session: vendor::SessionId::new(identity.to_owned()).expect("fixture identity"),
            locus: locus(),
        }),
        decoded: listen::Decoded::Events {
            context: Default::default(),
            events: Vec::new(),
        },
    }
}

/// The refusal one rejected declaration reaches its caller with.
///
/// The whole refusal is returned rather than its reason alone: import and
/// seating shared these seven reasons through separate enums until
/// RECOVER-1kK7X4f2, and the two disagreed on what each one witnessed, so the
/// reason is the part a collapse cannot get wrong.
fn session_refusal(session: SessionProof) -> Refusal {
    match await_session(session) {
        Ok(admitted) => panic!("session proof admitted {}", admitted.identity),
        Err(error) => {
            let refusal = error.refusal();
            assert_eq!(
                refusal
                    .diagnostic()
                    .and_then(wire::RefusalDiagnostic::witness),
                Some("session-declaration")
            );
            refusal
        }
    }
}

/// The identifying triple a refused declaration carries.
fn session_evidence(refusal: &Refusal) -> (&str, Option<&str>, Option<&str>) {
    let diagnostic = refusal
        .diagnostic()
        .expect("session proof refusal is diagnostic");
    (
        refusal.reason(),
        diagnostic.expected(),
        diagnostic.observed(),
    )
}

#[test]
fn session_proof_refuses_a_declaration_belonging_to_another_lane() {
    let proof = proof(None);
    let record = CaptureRecord {
        lane: uuid(9),
        ..declared("successor-session")
    };
    proof
        .reporter
        .record(record)
        .expect_err("a foreign lane declaration must be refused");
    let refusal = session_refusal(proof.session);
    assert_eq!(
        session_evidence(&refusal),
        (
            "lane::session-cross-lane",
            Some(uuid(3).to_string().as_str()),
            Some(uuid(9).to_string().as_str()),
        )
    );
}

#[test]
fn session_proof_refuses_a_declaration_belonging_to_another_occupant() {
    let proof = proof(None);
    let record = CaptureRecord {
        occupant: uuid(9),
        ..declared("successor-session")
    };
    proof
        .reporter
        .record(record)
        .expect_err("a foreign occupant declaration must be refused");
    let refusal = session_refusal(proof.session);
    assert_eq!(
        session_evidence(&refusal),
        (
            "lane::session-cross-occupant",
            Some(uuid(5).to_string().as_str()),
            Some(uuid(9).to_string().as_str()),
        )
    );
}

#[test]
fn session_proof_refuses_a_declaration_off_the_selected_stream() {
    let proof = proof(None);
    let record = CaptureRecord {
        stream: vendor::RecordStream::Stderr,
        ..declared("successor-session")
    };
    proof
        .reporter
        .record(record)
        .expect_err("an unselected stream declaration must be refused");
    let refusal = session_refusal(proof.session);
    assert_eq!(
        session_evidence(&refusal),
        (
            "lane::session-wrong-stream",
            Some("the vendor-selected record stream"),
            Some("another child stream"),
        )
    );
}

#[test]
fn session_proof_refuses_a_declaration_resuming_another_identity() {
    let proof = proof(Some("predecessor-session"));
    proof
        .reporter
        .record(declared("successor-session"))
        .expect_err("a resumed launch must declare the exact durable identity");
    let refusal = session_refusal(proof.session);
    assert_eq!(
        session_evidence(&refusal),
        (
            "lane::session-mismatched",
            Some("predecessor-session"),
            Some("successor-session"),
        )
    );
}

#[test]
fn session_proof_carries_the_one_admitted_launch_identity() {
    let proof = proof(None);
    proof
        .reporter
        .record(declared("successor-session"))
        .expect("the initialization record is admitted");

    let session = await_session(proof.session).expect("one declaration proves the session");

    let launch = session
        .launch
        .expect("current declaration records its launch");
    assert_eq!(launch.model, "sonnet");
    assert_eq!(launch.effort, "high");
    let rtk = launch.rtk.expect("current declaration records RTK health");
    assert_eq!(rtk.state(), wire::body::RtkHealthState::Active);
    assert_eq!(rtk.version(), Some("0.45.0"));
    assert_eq!(rtk.detail(), "fixture active");
    assert_eq!(session.identity, "successor-session");
}

#[test]
fn session_proof_refuses_a_second_declaration_from_one_occupant() {
    let proof = proof(None);
    proof
        .reporter
        .record(declared("successor-session"))
        .expect("the one initialization record is admitted");
    proof
        .reporter
        .record(declared("intruder-session"))
        .expect_err("a second declaration must be refused");
    let refusal = session_refusal(proof.session);
    // The two identities differ so the observed one names which declaration
    // the refusal witnesses. Declaring both under one identity passes whether
    // the admitted or the refused record is reported.
    assert_eq!(
        session_evidence(&refusal),
        (
            "lane::session-duplicate",
            Some("one declaration per occupant"),
            Some("intruder-session"),
        )
    );
}

#[test]
fn session_proof_refuses_a_terminal_capture_failure() {
    let proof = proof(None);
    proof.reporter.failed(CaptureFailure {
        lane: uuid(3),
        occupant: uuid(5),
        failure: sill::ObservationFailure::Reader {
            stream: sill::OutputStream::Stdout,
            detail: "selected pipe closed".into(),
        },
    });
    let refusal = session_refusal(proof.session);
    assert_eq!(refusal.reason(), "lane::session-capture-failed");
    assert_eq!(
        refusal
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::expected),
        Some("one intact selected-stream capture")
    );
    let observed = refusal
        .diagnostic()
        .and_then(wire::RefusalDiagnostic::observed)
        .expect("a capture failure witnesses its detail");
    assert!(
        observed.contains("selected pipe closed"),
        "the refusal carries the failure that ended the capture: {observed}"
    );
}

#[test]
fn session_proof_refuses_a_launch_that_never_declared() {
    let proof = proof(None);
    drop(proof.reporter);
    let refusal = session_refusal(proof.session);
    assert_eq!(
        session_evidence(&refusal),
        (
            "lane::session-missing",
            Some("one current selected-stream session declaration"),
            Some("no record on the selected stream"),
        )
    );
}

#[test]
fn session_proof_separates_a_silent_launch_from_a_talkative_one() {
    let typed = proof(None);
    typed
        .reporter
        .record(CaptureRecord {
            declaration: None,
            ..declared("successor-session")
        })
        .expect("a record declaring nothing is still captured");
    drop(typed.reporter);
    let talkative = session_refusal(typed.session);

    let silent = proof(None);
    drop(silent.reporter);

    // Both refuse for the same reason, so only the observation separates an
    // occupant that wrote nothing from one that wrote records it never
    // declared in.
    assert_eq!(talkative.reason(), session_refusal(silent.session).reason());
    assert_eq!(
        talkative
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::observed),
        Some("records on the selected stream, none declaring a session"),
    );
}

#[test]
fn session_proof_witnesses_only_the_stream_it_waits_on() {
    let proof = proof(None);
    proof
        .reporter
        .record(CaptureRecord {
            declaration: None,
            stream: vendor::RecordStream::Stderr,
            ..declared("successor-session")
        })
        .expect("an unselected stream record is still captured");
    drop(proof.reporter);
    let refusal = session_refusal(proof.session);
    // A vendor whose only output is diagnostics on the other stream has said
    // nothing on the one the deadline waits on, so it stays the silent case.
    assert_eq!(
        refusal
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::observed),
        Some("no record on the selected stream"),
    );
}

// A successful import: one atomic transfer of seat, identity, lineage, session
// authority, and active claim into the successor worktree.

fn admitted(fixture: &Fixture, request: &LaneState) -> wire::AdmittedCommand {
    Command {
        v: wire::WireVersion::CURRENT,
        id: uuid(8),
        epoch: fixture.new_epoch,
        kind: wire::CommandKind::LaneState,
        expected: Vec::new(),
        body: serde_json::to_value(request).expect("lane intent encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch: fixture.new_epoch,
    })
    .expect("operator lane intent admits")
}

fn proven(fixture: &Fixture) -> ProvenOccupant {
    ProvenOccupant {
        occupant: fixture.successor,
        group: 42,
        git: "current".into(),
        session: wire::LaneSession {
            lane: fixture.lane,
            occupant: fixture.successor,
            worktree: SUCCESSOR.into(),
            vendor: "claude".parse().expect("fixture vendor"),
            launch: Some(wire::body::LaneLaunchIdentity::new("sonnet", "high").with_git("current")),
            epoch: fixture.new_epoch,
            stream: "stdout".into(),
            identity: "successor-session".into(),
            locus: wire::TapeLocus {
                member: 0,
                offset: 0,
                length: 64,
            },
        },
    }
}

fn assert_imported(fixture: &Fixture) {
    let snapshot = fixture.bus.task_snapshot().expect("imported heads read");
    let residency = snapshot.residency().expect("residency remains present");
    let row = &residency.body().lanes[0];
    assert_eq!(row.worktree, SUCCESSOR);
    assert_eq!(row.occupant, Some(fixture.successor));
    assert_eq!(row.lineage, [fixture.ancestor, fixture.successor]);
    assert_eq!(row.disposition, OCCUPIED);
    assert_eq!(row.capabilities, [uuid(6)]);
    assert_eq!(row.vendor.as_str(), "claude");
    let session = row.session.as_ref().expect("import declares one authority");
    assert_eq!(session.identity, "successor-session");
    let launch = session
        .launch
        .as_ref()
        .expect("import preserves launch identity");
    assert_eq!(launch.model, "sonnet");
    assert_eq!(launch.effort, "high");
    assert_eq!(session.occupant, fixture.successor);
    assert_eq!(session.epoch, fixture.new_epoch);
    assert!(
        residency
            .body()
            .lanes
            .iter()
            .all(|row| row.worktree != PREDECESSOR && row.occupant != Some(fixture.ancestor)),
        "a predecessor row remained authoritative after import"
    );
    let claim = snapshot
        .tasks()
        .first()
        .expect("fixture task remains")
        .claim()
        .expect("claim remains active");
    assert_eq!(claim.body().occupant, fixture.successor);
    assert_eq!(claim.body().epoch, fixture.new_epoch);
    assert_eq!(claim.position(), residency.position());
    assert_imported_facts(fixture);
    fixture.bus.verify().expect("imported authority verifies");
}

fn assert_imported_facts(fixture: &Fixture) {
    let facts = fixture.bus.fact_snapshot().expect("facts read");
    let kinds = facts
        .facts()
        .iter()
        .map(|fact| fact.fact.kind())
        .collect::<Vec<_>>();
    for kind in [
        wire::FactKind::LaneImported,
        wire::FactKind::LaneSessionDeclared,
        wire::FactKind::LaneStarted,
        wire::FactKind::ClaimCarried,
    ] {
        assert!(kinds.contains(&kind), "import recorded no {kind:?}");
    }
}

#[test]
fn import_moves_seat_lineage_session_and_claim_into_the_successor_worktree() {
    let fixture = fixture();
    seed_lanes(&fixture, vec![vacant_row(&fixture)]);
    reseat(&fixture);
    let carried = observe_carried_claim(&fixture.bus, fixture.lane, fixture.ancestor, fixture.at)
        .expect("claim observation")
        .expect("the predecessor holds one active operator claim");
    let observed = current_residency(&fixture.bus, fixture.new_epoch).expect("current residency");
    let context = ImportContext {
        row: observed.body().lanes[0].clone(),
        observed,
        worktree: SUCCESSOR.into(),
        ancestor: fixture.ancestor,
        resume: vendor::Resume::Session(
            vendor::SessionId::new("predecessor-session".to_owned()).expect("fixture identity"),
        ),
        carried: Some(carried),
    };
    let request = wake(fixture.lane, Some(SUCCESSOR));
    let answer = LaneChangeAnswer {
        lane: fixture.lane,
        topology_revision: context.observed.body().topology_revision,
        from: context.row.worktree.clone(),
        to: context.worktree.clone(),
        git: Some("current".into()),
        rtk: None,
    };
    let proposal = propose_import(
        &admitted(&fixture, &request),
        fixture.new_epoch,
        &request,
        &context,
        &proven(&fixture),
        answer,
    )
    .expect("import proposal composes");
    commit(&fixture.bus, proposal).expect("import commits atomically");
    assert_imported(&fixture);
    finish(fixture);
}
