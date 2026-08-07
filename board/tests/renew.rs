use std::fs;
use std::path::{Path, PathBuf};

use board::Snapshot;
use board::allocate::{Mint, mint};
use board::lapse::{Request as LapseRequest, lapse};
use board::possession::{FreeRequest, free};
use board::renew::{Refusal, RenewError, Request, renew};
use journal::{FactCause, Journal, JournalWrite};
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, ClaimHead, Command, CommandKind, Instant,
    LaneResidency, Origin, Phase, Producer, ResidencyHead, ResidentState, TaskHandle, TaskHead,
    TranscriptSourceHead, WireVersion, Witness, WorkflowHead, body, intent,
};

fn uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131165{last:02}")
        .parse()
        .expect("fixture uuid")
}

fn instant(minute: u8) -> Instant {
    format!("2026-08-01T08:{minute:02}:00.000Z")
        .parse()
        .expect("fixture instant")
}

fn task() -> TaskHandle {
    "BOARD-a1".parse().expect("fixture task")
}

fn projects() -> board::ProjectPolicy {
    board::ProjectPolicy::new(
        ["board".to_owned()],
        Vec::new(),
        dialect::PROJECT_MIN_DEPTH as u64,
        dialect::PROJECT_MAX_DEPTH as u64,
    )
    .expect("fixture project policy")
}

/// One boundary with an accounted stream in it.
///
/// Deliberately not the empty `Proven` the other fixtures open with: an
/// assertion that renewal left this exact value in place can only fail if the
/// value it is compared against is one renewal could plausibly have replaced.
fn opening_boundary() -> body::SourceBoundary {
    body::SourceBoundary::Proven {
        heads: vec![TranscriptSourceHead {
            lane: uuid(20),
            vendor: "claude".parse().expect("fixture vendor"),
            stream: "session".to_owned(),
            source: uuid(23),
            next: body::TranscriptPosition {
                member: 1,
                offset: 4096,
            },
        }],
    }
}

/// Seed one active claim the way the claim command writes it.
///
/// The shared minter writes the fact and both heads, and the transaction
/// carries the command receipt beside them, so `command` is spent here exactly
/// as a real claim would spend it.
fn claim(journal: &mut Journal, command: u8) {
    claim_opening(
        journal,
        command,
        body::SourceBoundary::Proven { heads: Vec::new() },
    );
}

/// Seed one active claim opening at a given transcript boundary.
///
/// The shared minter writes the fact and both heads, and the transaction
/// carries the command receipt beside them, so `command` is spent here exactly
/// as a real claim would spend it.
fn claim_opening(journal: &mut Journal, command: u8, source: body::SourceBoundary) {
    let heads = journal
        .read_with(|read| read.task_heads())
        .expect("heads before the claim");
    let selected = heads
        .iter()
        .find(|heads| heads.statement().body().handle == task())
        .expect("the fixture task is on the board");
    let workflow = selected.workflow().body().clone();
    let expected = vec![
        Witness::Task {
            task: task(),
            head: selected.statement().position(),
        },
        Witness::Workflow {
            task: task(),
            head: selected.workflow().position(),
            phase: workflow.phase.clone(),
        },
        Witness::Claim {
            task: task(),
            head: selected.claim().map(journal::JournalHead::position),
        },
        Witness::Eligibility {
            rule: "next".parse().expect("rule"),
        },
        Witness::Residency {
            lane: uuid(20),
            occupant: uuid(21),
            epoch: uuid(22),
        },
    ];
    let actor = Actor::Lane(uuid(20));
    let mut write = JournalWrite::new(
        actor.clone(),
        "task:BOARD-a1".parse().expect("claim origin"),
        instant(5),
        Producer::CommandDoor,
    );
    mint(
        &mut write,
        Mint {
            task: &task(),
            actor: &actor,
            workflow: &workflow,
            lane: uuid(20),
            occupant: uuid(21),
            epoch: uuid(22),
            at: instant(5),
            expiry: instant(20),
            maximum: instant(35),
            git: None,
            source,
        },
    )
    .expect("the claim mints");
    write
        .command_result(
            Command {
                v: WireVersion::CURRENT,
                id: uuid(command),
                epoch: uuid(22),
                kind: CommandKind::ClaimWin,
                expected,
                body: intent::ClaimWin {
                    rule: "next".parse().expect("rule"),
                },
            },
            uuid(22),
            intent::ClaimAnswer {
                task: task(),
                holder: actor,
                warnings: Vec::new(),
                next: None,
            },
        )
        .expect("the claim receipt builds");
    journal.apply(write).expect("the claim commits");
}

fn renew_request(command: u8) -> Request {
    Request {
        projects: projects(),
        command: uuid(command),
        occupant: uuid(21),
        epoch: uuid(22),
        at: instant(10),
        until: instant(25),
        intent: intent::ClaimRenew {
            task: task(),
            lease_seconds: 15,
        },
    }
}

fn lapse_request(command: u8, expiry: u8) -> LapseRequest {
    LapseRequest {
        projects: projects(),
        command: uuid(command),
        occupant: uuid(21),
        epoch: uuid(22),
        expiry: instant(expiry),
        at: instant(expiry),
        intent: intent::TaskRef { task: task() },
    }
}

fn fixture(path: Option<&Path>) -> Journal {
    let mut journal = path.map_or_else(
        || Journal::in_memory().expect("memory journal"),
        |path| Journal::open(path).expect("file journal"),
    );
    let phase = "todo".parse::<Phase>().expect("phase");
    let origin = "task:BOARD-root".parse::<Origin>().expect("origin");
    let mut write = JournalWrite::new(
        Actor::Spice,
        origin.clone(),
        instant(0),
        Producer::CommandDoor,
    );
    let fact = write
        .fact(
            body::TaskAdded {
                task: task(),
                uuid: uuid(1),
                title: "Renew possession".to_owned(),
                description: None,
                acceptance: Vec::new(),
                project: "board.possession".to_owned(),
                tags: vec!["priority:high".to_owned()],
                depends: Vec::new(),
                flow: vec![phase.clone()],
                phase: phase.clone(),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("task fact");
    let statement_origin = origin;
    write.task_head(fact, move |position| TaskHead {
        handle: task(),
        uuid: uuid(1),
        title: "Renew possession".to_owned(),
        description: None,
        acceptance: Vec::new(),
        project: "board.possession".to_owned(),
        tags: vec!["priority:high".to_owned()],
        dependencies: Vec::new(),
        provenance: statement_origin,
        revision: position,
    });
    write.workflow_head(task(), fact, move |_| WorkflowHead {
        flow: vec![phase.clone()],
        phase,
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    });
    write.residency_head(fact, |_| ResidencyHead {
        epoch: uuid(22),
        state: ResidentState::Seated,
        lanes: vec![LaneResidency {
            lane: uuid(20),
            lineage_id: uuid(20),
            name: None,
            worktree: "/worktrees/lane-a".to_owned(),
            life: "drive".to_owned(),
            vendor: "claude".parse().expect("fixture vendor"),
            occupant: Some(uuid(21)),
            lineage: Vec::new(),
            capabilities: Vec::new(),
            session: None,
            disposition: "occupied".to_owned(),
        }],
        subscriptions: vec![wire::LineageSubscription {
            lineage_id: uuid(20),
            policy: wire::intent::LineageLife::Drain,
            manual_projects: Vec::new(),
            auto_projects: Vec::new(),
        }],
    });
    journal.apply(write).expect("seed authority");
    journal
}

fn claimed() -> Journal {
    let mut journal = fixture(None);
    claim(&mut journal, 31);
    journal
}

#[test]
fn renewal_preserves_claim_and_extends_only_the_lease() {
    let mut journal = claimed();
    let before = Snapshot::read(&journal, &projects()).expect("claimed snapshot");
    let statement = before.entries()[0].statement().clone();
    let workflow = before.entries()[0].workflow().clone();
    let claim = before.entries()[0].claim().expect("claim").clone();
    let old_lease = before.entries()[0].lease().expect("lease").clone();

    let renewed = renew(&mut journal, renew_request(32)).expect("renew claim");
    assert_eq!(renewed.task(), &task());
    assert_eq!(renewed.first(), renewed.last());
    let after = Snapshot::read(&journal, &projects()).expect("renewed snapshot");
    assert_eq!(after.entries()[0].statement(), &statement);
    assert_eq!(after.entries()[0].workflow(), &workflow);
    assert_eq!(after.entries()[0].claim(), Some(&claim));
    let lease = after.entries()[0].lease().expect("renewed lease");
    assert_eq!(lease.start, old_lease.start);
    assert_eq!(lease.renewal, instant(10));
    assert_eq!(lease.expiry, instant(25));
    assert_eq!(lease.maximum, old_lease.maximum);
    assert_eq!(lease.epoch, old_lease.epoch);
    let facts = journal.fact_snapshot().expect("renewed facts");
    assert_eq!(facts.facts().len(), 3);
    assert_eq!(facts.facts()[2].fact.kind(), wire::FactKind::ClaimRenewed);
    assert_eq!(journal.verify().expect("authority verifies").receipts, 2);
}

/// Renewing an occupant's possession leaves the opening boundary open.
///
/// A renewal extends a lease inside one phase interval; it neither closes the
/// interval nor reopens it. A renewal that wrote its own boundary would make
/// the interval appear to restart at every heartbeat, and every sample spent
/// before that heartbeat would fall outside the accounted window the reducer
/// later splits on. Two renewals rather than one, because a single one cannot
/// tell *left alone* apart from *rewritten to the value it already held*.
#[test]
fn renewing_an_occupant_leaves_the_opening_boundary_open() {
    let mut journal = fixture(None);
    claim_opening(&mut journal, 31, opening_boundary());

    renew(&mut journal, renew_request(32)).expect("first renewal");
    let mut again = renew_request(33);
    again.at = instant(12);
    again.until = instant(30);
    renew(&mut journal, again).expect("second renewal");

    let facts = journal.fact_snapshot().expect("renewed facts");
    let kinds = facts
        .facts()
        .iter()
        .map(|fact| fact.fact.kind())
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            wire::FactKind::TaskAdded,
            wire::FactKind::ClaimWon,
            wire::FactKind::ClaimRenewed,
            wire::FactKind::ClaimRenewed,
        ],
    );
    let won: body::ClaimWon = facts.facts()[1]
        .fact
        .read()
        .expect("the opening fact reads back through the registry");
    assert_eq!(won.source, opening_boundary());

    // Read as text rather than as a field, because the absence being proven is
    // the absence of a field: an added one would compile everywhere and only
    // show up as a key nothing here would otherwise look at.
    for position in [2, 3] {
        let renewal: body::ClaimRenewed = facts.facts()[position]
            .fact
            .read()
            .expect("the renewal reads back through the registry");
        let encoded = String::from_utf8(wire::encode(&renewal).expect("encode the renewal"))
            .expect("a renewal encodes as text");
        assert!(
            !encoded.contains("source"),
            "renewal {position} recorded a boundary of its own: {encoded}",
        );
    }
}

/// A task command whose transaction appends no fact is refused, not receipted.
///
/// A journal write can carry a command receipt without carrying a fact, and
/// that shape is exactly a partial application: the caller is told its command
/// succeeded while the canonical stream records nothing it could have succeeded
/// at, so a later reader sees a receipt with no cause. The refusal has to come
/// from the transaction itself, because every producer assembles its receipt
/// before it can know whether its facts survived the guards.
///
/// The witness positions are fixture values. Command law admits a witness list
/// by kind rather than by position, and this transaction is refused before any
/// guard reads, so nothing here depends on which position they name.
#[test]
fn a_task_command_that_appends_no_fact_is_refused_instead_of_receipted() {
    let mut journal = claimed();
    let before = journal.fact_snapshot().expect("facts before").facts().len();
    let holder = Actor::Lane(uuid(20));
    let head = wire::Seq::new(2).expect("fixture head position");

    let mut write = JournalWrite::new(
        holder.clone(),
        "task:BOARD-a1".parse::<Origin>().expect("origin"),
        instant(10),
        Producer::BusTimer,
    );
    write
        .command_result(
            wire::Command {
                v: wire::WireVersion::CURRENT,
                id: uuid(34),
                epoch: uuid(22),
                kind: wire::CommandKind::ClaimRenew,
                expected: vec![
                    wire::Witness::Claim {
                        task: task(),
                        head: Some(head),
                    },
                    wire::Witness::Lease { task: task(), head },
                    wire::Witness::Residency {
                        lane: uuid(20),
                        occupant: uuid(21),
                        epoch: uuid(22),
                    },
                ],
                body: intent::ClaimRenew {
                    task: task(),
                    lease_seconds: 15,
                },
            },
            uuid(22),
            intent::ClaimAnswer {
                task: task(),
                holder,
                warnings: Vec::new(),
                next: None,
            },
        )
        .expect("the receipt itself is well formed");

    let error = journal
        .apply(write)
        .expect_err("a transaction appending no fact cannot apply");
    assert!(
        error.to_string().contains("must append at least one fact"),
        "a factless transaction was refused for another reason: {error}",
    );
    assert_eq!(
        journal.fact_snapshot().expect("facts after").facts().len(),
        before,
        "a refused transaction appended a fact",
    );
}

#[test]
fn renewal_refusals_preserve_every_fact_and_head() {
    let cases = [
        {
            let mut request = renew_request(40);
            request.intent.task = "BOARD-missing".parse().expect("missing task");
            let expected = Refusal::MissingTask(request.intent.task.clone());
            (request, expected)
        },
        {
            let mut request = renew_request(41);
            request.at = instant(4);
            let expected = Refusal::Claim(board::ClaimRefusal::NotStarted { start: instant(5) });
            (request, expected)
        },
        {
            let mut request = renew_request(42);
            request.at = instant(20);
            let expected = Refusal::Claim(board::ClaimRefusal::Expired {
                expiry: instant(20),
            });
            (request, expected)
        },
        {
            let mut request = renew_request(43);
            request.until = instant(20);
            let expected = Refusal::NonExtending {
                stored: instant(20),
                proposed: instant(20),
            };
            (request, expected)
        },
        {
            let mut request = renew_request(44);
            request.until = instant(36);
            let expected = Refusal::BeyondMaximum {
                proposed: instant(36),
                maximum: instant(35),
            };
            (request, expected)
        },
        {
            let mut request = renew_request(45);
            request.occupant = uuid(24);
            let expected = Refusal::ClaimOccupant {
                holding: uuid(21),
                named: uuid(24),
            };
            (request, expected)
        },
        {
            let mut request = renew_request(46);
            request.epoch = uuid(23);
            let expected = Refusal::ClaimEpoch {
                holding: uuid(22),
                named: uuid(23),
            };
            (request, expected)
        },
    ];
    for (request, expected) in cases {
        let mut journal = claimed();
        let before = journal.fact_snapshot().expect("before refusal");
        let heads = Snapshot::read(&journal, &projects()).expect("heads before refusal");
        match renew(&mut journal, request) {
            Err(RenewError::Refused(observed)) => assert_eq!(observed, expected),
            other => panic!("expected {expected:?}, found {other:?}"),
        }
        assert_eq!(journal.fact_snapshot().expect("after refusal"), before);
        assert_eq!(
            Snapshot::read(&journal, &projects()).expect("heads after refusal"),
            heads
        );
    }

    let mut journal = claimed();
    free(
        &mut journal,
        FreeRequest {
            projects: board::ProjectPolicy::new(
                ["board".to_owned()],
                Vec::new(),
                dialect::PROJECT_MIN_DEPTH as u64,
                dialect::PROJECT_MAX_DEPTH as u64,
            )
            .expect("fixture project policy"),
            command: uuid(47),
            actor: Actor::Lane(uuid(20)),
            lane: uuid(20),
            occupant: uuid(21),
            epoch: uuid(22),
            at: instant(10),
            intent: intent::TaskRef { task: task() },
        },
    )
    .expect("free first");
    let before = journal.fact_snapshot().expect("before inactive refusal");
    let heads = Snapshot::read(&journal, &projects()).expect("heads before inactive refusal");
    assert!(matches!(
        renew(&mut journal, renew_request(48)),
        Err(RenewError::Refused(Refusal::Claim(
            board::ClaimRefusal::Inactive(ClaimDisposition::Freed)
        )))
    ));
    assert_eq!(journal.fact_snapshot().expect("after refusal"), before);
    assert_eq!(
        Snapshot::read(&journal, &projects()).expect("heads after inactive refusal"),
        heads
    );
}

#[test]
fn a_claim_without_its_lease_is_a_typed_zero_write_refusal() {
    let mut journal = fixture(None);
    let actor = Actor::Lane(uuid(20));
    let mut write = JournalWrite::new(
        actor.clone(),
        "task:BOARD-a1".parse().expect("claim origin"),
        instant(5),
        Producer::CommandDoor,
    );
    let fact = write
        .fact(
            body::ClaimWon {
                task: task(),
                actor: actor.clone(),
                lane: uuid(20),
                occupant: uuid(21),
                epoch: uuid(22),
                until: instant(20),
                git: None,
                source: body::SourceBoundary::Proven { heads: Vec::new() },
            },
            FactCause::Root,
        )
        .expect("claim fact");
    write.claim_head(task(), fact, move |position| ClaimHead {
        actor,
        fact: position,
        lane: uuid(20),
        occupant: uuid(21),
        epoch: uuid(22),
        disposition: ClaimDisposition::Active,
    });
    journal.apply(write).expect("seed malformed claim");

    let before = journal.fact_snapshot().expect("before malformed refusal");
    let heads = Snapshot::read(&journal, &projects()).expect("heads before malformed refusal");
    assert!(matches!(
        renew(&mut journal, renew_request(50)),
        Err(RenewError::Refused(Refusal::MissingLease))
    ));
    assert_eq!(
        journal.fact_snapshot().expect("after malformed refusal"),
        before
    );
    assert_eq!(
        Snapshot::read(&journal, &projects()).expect("heads after malformed refusal"),
        heads
    );
}

#[test]
fn independent_connections_observe_stale_and_current_expiry_events() {
    let store = Store::new();
    let mut seeded = fixture(Some(store.path()));
    claim(&mut seeded, 31);
    drop(seeded);
    let mut first = Journal::open(store.path()).expect("first connection");
    let mut second = Journal::open(store.path()).expect("second connection");

    let before = first.fact_snapshot().expect("before duplicate");
    let heads = Snapshot::read(&first, &projects()).expect("heads before duplicate");
    assert!(matches!(
        renew(&mut first, renew_request(31)),
        Err(RenewError::Commit(_))
    ));
    assert_eq!(first.fact_snapshot().expect("duplicate rolls back"), before);
    assert_eq!(
        Snapshot::read(&first, &projects()).expect("duplicate preserves heads"),
        heads
    );

    renew(&mut second, renew_request(52)).expect("second connection renews");
    let before_stale = first.fact_snapshot().expect("before stale lapse");
    let heads = Snapshot::read(&first, &projects()).expect("heads before stale lapse");
    assert!(matches!(
        lapse(&mut first, lapse_request(53, 20)),
        Err(board::lapse::LapseError::Refused(
            board::lapse::Refusal::StaleExpiry { .. }
        ))
    ));
    assert_eq!(
        first.fact_snapshot().expect("stale timer writes nothing"),
        before_stale
    );
    assert_eq!(
        Snapshot::read(&first, &projects()).expect("stale timer preserves heads"),
        heads
    );
    let snapshot = Snapshot::read(&first, &projects()).expect("renewed heads");
    board::Active::admit(&snapshot.entries()[0], &Actor::Lane(uuid(20)), instant(24))
        .expect("renewed lease remains active");

    lapse(&mut first, lapse_request(54, 25)).expect("new expiry lapses");
    let reallocatable = Snapshot::read(&second, &projects()).expect("second sees the new expiry");
    assert!(
        board::eligible_entry(
            reallocatable.entries(),
            &reallocatable.entries()[0],
            &Actor::Lane(uuid(20))
        ),
        "a lease lapsed at its renewed expiry leaves the task allocatable again"
    );
    assert_eq!(
        second.verify().expect("shared authority verifies").receipts,
        3
    );
}

struct Store {
    path: PathBuf,
}

impl Store {
    fn new() -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "spice-board-renew-{}-{}.sqlite",
                std::process::id(),
                uuid(60)
            )),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        for path in [
            self.path.clone(),
            self.path.with_extension("sqlite-wal"),
            self.path.with_extension("sqlite-shm"),
        ] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("remove renewal fixture: {error}"),
            }
        }
    }
}
