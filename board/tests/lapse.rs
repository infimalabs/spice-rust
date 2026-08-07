use std::fs;
use std::path::{Path, PathBuf};

use board::Snapshot;
use board::allocate::{Mint, mint};
use board::lapse::{LapseError, Refusal, Request, lapse};
use board::possession::{FreeRequest, free};
use journal::{FactCause, Journal, JournalWrite};
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, ClaimHead, Command, CommandKind, Instant,
    LaneResidency, Origin, Phase, Producer, ResidencyHead, ResidentState, TaskHandle, TaskHead,
    WireVersion, Witness, WorkflowHead, body, intent,
};

fn uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131164{last:02}")
        .parse()
        .expect("fixture uuid")
}

fn instant(minute: u8) -> Instant {
    format!("2026-08-01T07:{minute:02}:00.000Z")
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

/// Seed one active claim the way the claim command writes it.
///
/// The shared minter writes the fact and both heads, and the transaction
/// carries the command receipt beside them, so `command` is spent here exactly
/// as a real claim would spend it.
fn claim(journal: &mut Journal, command: u8) {
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

fn lapse_request(command: u8) -> Request {
    Request {
        projects: projects(),
        command: uuid(command),
        occupant: uuid(21),
        epoch: uuid(22),
        expiry: instant(20),
        at: instant(20),
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
                title: "Lapse possession".to_owned(),
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
        title: "Lapse possession".to_owned(),
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
fn lapse_preserves_lease_and_makes_the_task_allocatable_again() {
    let mut journal = claimed();
    let before = Snapshot::read(&journal, &projects()).expect("claimed snapshot");
    let statement = before.entries()[0].statement().clone();
    let workflow = before.entries()[0].workflow().clone();
    let lease = before.entries()[0].lease().expect("lease").clone();

    let expired = lapse(&mut journal, lapse_request(32)).expect("lapse claim");
    assert_eq!(expired.task(), &task());
    assert_eq!(expired.first(), expired.last());
    let after = Snapshot::read(&journal, &projects()).expect("lapsed snapshot");
    assert_eq!(
        after.entries()[0].claim().expect("claim").disposition,
        ClaimDisposition::Lapsed
    );
    assert_eq!(after.entries()[0].statement(), &statement);
    assert_eq!(after.entries()[0].workflow(), &workflow);
    assert_eq!(after.entries()[0].lease(), Some(&lease));
    let facts = journal.fact_snapshot().expect("lapsed facts");
    assert_eq!(facts.facts().len(), 3);
    assert_eq!(facts.facts()[2].fact.kind(), wire::FactKind::ClaimLapsed);
    assert_eq!(journal.verify().expect("authority verifies").receipts, 2);

    assert!(
        board::eligible_entry(after.entries(), &after.entries()[0], &Actor::Lane(uuid(20))),
        "a lapsed claim leaves the task allocatable again"
    );
}

#[test]
fn timer_and_authority_refusals_leave_the_journal_unchanged() {
    let cases = [
        {
            let mut request = lapse_request(40);
            request.intent.task = "BOARD-missing".parse().expect("missing task");
            let expected = Refusal::MissingTask(request.intent.task.clone());
            (request, expected)
        },
        {
            let mut request = lapse_request(41);
            request.at = instant(19);
            let expected = Refusal::Early {
                at: request.at,
                expiry: request.expiry,
            };
            (request, expected)
        },
        {
            let mut request = lapse_request(42);
            request.expiry = instant(19);
            let expected = Refusal::StaleExpiry {
                stored: instant(20),
                fired: request.expiry,
            };
            (request, expected)
        },
        {
            let mut request = lapse_request(43);
            request.occupant = uuid(24);
            let expected = Refusal::ClaimOccupant {
                holding: uuid(21),
                named: uuid(24),
            };
            (request, expected)
        },
        {
            let mut request = lapse_request(44);
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
        match lapse(&mut journal, request) {
            Err(LapseError::Refused(observed)) => assert_eq!(observed, expected),
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
            command: uuid(45),
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
        lapse(&mut journal, lapse_request(46)),
        Err(LapseError::Refused(Refusal::Inactive(
            ClaimDisposition::Freed
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
        lapse(&mut journal, lapse_request(48)),
        Err(LapseError::Refused(Refusal::MissingLease))
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
fn duplicate_receipt_rolls_back_and_independent_connections_share_lapse() {
    let store = Store::new();
    let mut seeded = fixture(Some(store.path()));
    claim(&mut seeded, 31);
    drop(seeded);
    let mut first = Journal::open(store.path()).expect("first connection");
    let mut second = Journal::open(store.path()).expect("second connection");

    let before = first.fact_snapshot().expect("before duplicate");
    let heads = Snapshot::read(&first, &projects()).expect("heads before duplicate");
    assert!(matches!(
        lapse(&mut first, lapse_request(31)),
        Err(LapseError::Commit(_))
    ));
    assert_eq!(first.fact_snapshot().expect("duplicate rolls back"), before);
    assert_eq!(
        Snapshot::read(&first, &projects()).expect("duplicate preserves heads"),
        heads
    );

    lapse(&mut second, lapse_request(52)).expect("second connection lapses");
    let shared = Snapshot::read(&first, &projects()).expect("first sees the lapse");
    assert!(
        board::eligible_entry(
            shared.entries(),
            &shared.entries()[0],
            &Actor::Lane(uuid(20))
        ),
        "the connection that did not lapse still sees the task allocatable"
    );
    assert_eq!(
        first.verify().expect("shared authority verifies").receipts,
        2
    );
}

struct Store {
    path: PathBuf,
}

impl Store {
    fn new() -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "spice-board-lapse-{}-{}.sqlite",
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
                Err(error) => panic!("remove lapse fixture: {error}"),
            }
        }
    }
}
