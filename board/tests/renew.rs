use std::fs;
use std::path::{Path, PathBuf};

use board::Snapshot;
use board::allocate::{Request as WinRequest, win};
use board::lapse::{Request as LapseRequest, lapse};
use board::possession::{FreeRequest, free};
use board::renew::{Refusal, RenewError, Request, renew};
use journal::{FactCause, Journal, JournalWrite};
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, ClaimHead, Instant, LaneResidency, Origin, Phase,
    Producer, ResidencyHead, ResidentState, TaskHandle, TaskHead, WorkflowHead, body, intent,
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

fn win_request(command: u8) -> WinRequest {
    WinRequest {
        projects: projects(),
        command: uuid(command),
        actor: Actor::Lane(uuid(20)),
        lane: uuid(20),
        occupant: uuid(21),
        epoch: uuid(22),
        at: instant(5),
        expiry: instant(20),
        maximum: instant(35),
        rule: "next".parse().expect("rule"),
    }
}

fn renew_request(command: u8) -> Request {
    Request {
        projects: projects(),
        command: uuid(command),
        occupant: uuid(21),
        epoch: uuid(22),
        at: instant(10),
        until: instant(25),
        intent: intent::TaskRef { task: task() },
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
            worktree: "/worktrees/lane-a".to_owned(),
            life: "drive".to_owned(),
            vendor: "claude".parse().expect("fixture vendor"),
            occupant: Some(uuid(21)),
            lineage: Vec::new(),
            capabilities: Vec::new(),
            session: None,
            disposition: "occupied".to_owned(),
        }],
    });
    journal.apply(write).expect("seed authority");
    journal
}

fn claimed() -> Journal {
    let mut journal = fixture(None);
    win(&mut journal, win_request(31)).expect("claim task");
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
    let request = win_request(49);
    let mut write = JournalWrite::new(
        request.actor.clone(),
        "task:BOARD-a1".parse().expect("claim origin"),
        request.at,
        Producer::CommandDoor,
    );
    let fact = write
        .fact(
            body::ClaimWon {
                task: task(),
                actor: request.actor.clone(),
                lane: request.lane,
                occupant: request.occupant,
                epoch: request.epoch,
                until: request.expiry,
                git: None,
            },
            FactCause::Root,
        )
        .expect("claim fact");
    write.claim_head(task(), fact, move |position| ClaimHead {
        actor: request.actor,
        fact: position,
        lane: request.lane,
        occupant: request.occupant,
        epoch: request.epoch,
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
    win(&mut seeded, win_request(31)).expect("claim through seed connection");
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
    let mut request = win_request(55);
    request.at = instant(26);
    request.expiry = instant(30);
    assert_eq!(
        win(&mut second, request)
            .expect("lapsed task reallocates")
            .task(),
        &task()
    );
    assert_eq!(
        second.verify().expect("shared authority verifies").receipts,
        4
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
