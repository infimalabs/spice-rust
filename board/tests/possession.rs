use std::fs;
use std::path::{Path, PathBuf};

use board::ClaimRefusal;
use board::Snapshot;
use board::allocate::{Request as WinRequest, win};
use board::possession::{FreeError, FreeRefusal, FreeRequest, free};
use journal::{FactCause, Journal, JournalWrite};
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, ClaimHead, Instant, LaneResidency, Origin, Phase,
    Producer, ResidencyHead, ResidentState, TaskHandle, TaskHead, WorkflowHead, body, intent,
};

fn uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
        .parse()
        .expect("fixture uuid")
}

fn instant(minute: u8) -> Instant {
    format!("2026-08-01T06:{minute:02}:00.000Z")
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

fn free_request(command: u8) -> FreeRequest {
    FreeRequest {
        projects: projects(),
        command: uuid(command),
        actor: Actor::Lane(uuid(20)),
        lane: uuid(20),
        occupant: uuid(21),
        epoch: uuid(22),
        at: instant(10),
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
                title: "Release possession".to_owned(),
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
        title: "Release possession".to_owned(),
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
fn release_preserves_lease_and_makes_the_task_allocatable_again() {
    let mut journal = claimed();
    let before = Snapshot::read(&journal, &projects()).expect("claimed snapshot");
    let statement = before.entries()[0].statement().clone();
    let workflow = before.entries()[0].workflow().clone();
    let lease = before.entries()[0].lease().expect("lease").clone();

    let released = free(&mut journal, free_request(32)).expect("release claim");
    assert_eq!(released.task(), &task());
    assert_eq!(released.first(), released.last());
    let after = Snapshot::read(&journal, &projects()).expect("released snapshot");
    assert_eq!(
        after.entries()[0].claim().expect("claim").disposition,
        ClaimDisposition::Freed
    );
    assert_eq!(after.entries()[0].statement(), &statement);
    assert_eq!(after.entries()[0].workflow(), &workflow);
    assert_eq!(after.entries()[0].lease(), Some(&lease));
    assert_eq!(journal.verify().expect("authority verifies").receipts, 2);

    let mut request = win_request(33);
    request.at = instant(11);
    request.expiry = instant(25);
    assert_eq!(
        win(&mut journal, request)
            .expect("task is eligible again")
            .task(),
        &task()
    );
}

#[test]
fn claim_and_residency_refusals_leave_authority_unchanged() {
    let cases = [
        {
            let mut request = free_request(40);
            request.intent.task = "BOARD-missing".parse().expect("missing task");
            request
        },
        {
            let mut request = free_request(41);
            request.actor = Actor::Operator;
            request
        },
        {
            let mut request = free_request(42);
            request.at = instant(4);
            request
        },
        {
            let mut request = free_request(43);
            request.at = instant(20);
            request
        },
        {
            let mut request = free_request(44);
            request.epoch = uuid(23);
            request
        },
        {
            let mut request = free_request(45);
            request.occupant = uuid(24);
            request
        },
    ];
    for request in cases {
        let mut journal = claimed();
        let before = journal.fact_snapshot().expect("before refusal");
        assert!(matches!(
            free(&mut journal, request),
            Err(FreeError::Refused(_))
        ));
        assert_eq!(journal.fact_snapshot().expect("after refusal"), before);
    }

    let mut journal = claimed();
    free(&mut journal, free_request(46)).expect("first release");
    let before = journal.fact_snapshot().expect("before inactive refusal");
    assert!(matches!(
        free(&mut journal, free_request(47)),
        Err(FreeError::Refused(FreeRefusal::Claim(
            ClaimRefusal::Inactive(ClaimDisposition::Freed)
        )))
    ));
    assert_eq!(
        journal.fact_snapshot().expect("after inactive refusal"),
        before
    );
}

#[test]
fn an_active_claim_without_its_lease_is_a_typed_zero_write_refusal() {
    let mut journal = fixture(None);
    let request = win_request(48);
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
    assert!(matches!(
        free(&mut journal, free_request(49)),
        Err(FreeError::Refused(FreeRefusal::Claim(
            ClaimRefusal::MissingLease
        )))
    ));
    assert_eq!(
        journal.fact_snapshot().expect("after malformed refusal"),
        before
    );
}

#[test]
fn duplicate_receipt_rolls_back_and_independent_connections_share_release() {
    let store = Store::new();
    let mut seeded = fixture(Some(store.path()));
    win(&mut seeded, win_request(31)).expect("claim through seed connection");
    drop(seeded);
    let mut first = Journal::open(store.path()).expect("first connection");
    let mut second = Journal::open(store.path()).expect("second connection");

    let before = first.fact_snapshot().expect("before duplicate");
    assert!(matches!(
        free(&mut first, free_request(31)),
        Err(FreeError::Commit(_))
    ));
    assert_eq!(first.fact_snapshot().expect("duplicate rolls back"), before);

    free(&mut second, free_request(52)).expect("second connection releases");
    let mut request = win_request(53);
    request.at = instant(11);
    request.expiry = instant(25);
    assert_eq!(
        win(&mut first, request).expect("first sees release").task(),
        &task()
    );
    assert_eq!(
        first.verify().expect("shared authority verifies").receipts,
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
                "spice-board-possession-{}-{}.sqlite",
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
                Err(error) => panic!("remove possession fixture: {error}"),
            }
        }
    }
}
