use std::fs;
use std::path::{Path, PathBuf};

use board::Snapshot;
use board::allocate::{Refusal, Request, WinError, win};
use journal::{FactCause, Journal, JournalWrite};
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, ClaimHead, FactKind, Instant, LaneResidency, LeaseHead,
    Origin, Phase, Producer, ResidencyHead, ResidentState, TaskHandle, TaskHead, WorkflowHead,
    body,
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

fn phase(text: &str) -> Phase {
    text.parse().expect("fixture phase")
}

fn handle(text: &str) -> TaskHandle {
    text.parse().expect("fixture handle")
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

fn request(command: u8) -> Request {
    let lane = uuid(20);
    Request {
        projects: projects(),
        command: uuid(command),
        actor: Actor::Lane(lane),
        lane,
        occupant: uuid(21),
        epoch: uuid(22),
        at: instant(5),
        expiry: instant(20),
        maximum: instant(35),
        rule: "next".parse().expect("current allocation rule"),
    }
}

fn workflow() -> WorkflowHead {
    WorkflowHead {
        flow: vec![phase("todo"), phase("review")],
        phase: phase("todo"),
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    }
}

fn residency() -> ResidencyHead {
    ResidencyHead {
        epoch: uuid(22),
        state: ResidentState::Seated,
        lanes: vec![LaneResidency {
            lane: uuid(20),
            team: uuid(20),
            worktree: "/worktrees/lane-a".to_owned(),
            life: "drive".to_owned(),
            vendor: "claude".parse().expect("fixture vendor"),
            occupant: Some(uuid(21)),
            lineage: Vec::new(),
            capabilities: Vec::new(),
            session: None,
            disposition: "occupied".to_owned(),
        }],
    }
}

fn fixture(path: Option<&Path>, with_residency: bool) -> Journal {
    let mut journal = path.map_or_else(
        || Journal::in_memory().expect("in-memory allocation journal"),
        |path| Journal::open(path).expect("file allocation journal"),
    );
    let origin = "task:BOARD-root".parse::<Origin>().expect("fixture origin");
    let mut write = JournalWrite::new(
        Actor::Spice,
        origin.clone(),
        instant(0),
        Producer::CommandDoor,
    );
    let tasks = [
        ("BOARD-b2", uuid(2), "priority:medium"),
        ("BOARD-a1", uuid(1), "priority:high"),
    ];
    for (index, (name, identity, priority)) in tasks.into_iter().enumerate() {
        let task = handle(name);
        let flow = workflow();
        let fact = write
            .fact(
                body::TaskAdded {
                    task: task.clone(),
                    uuid: identity,
                    title: format!("Allocate {name}"),
                    description: None,
                    acceptance: Vec::new(),
                    project: "board.allocate".to_owned(),
                    tags: vec![priority.to_owned()],
                    depends: Vec::new(),
                    flow: flow.flow.clone(),
                    phase: flow.phase.clone(),
                    revision: 1,
                },
                FactCause::Root,
            )
            .expect("registered task fact");
        let statement_task = task.clone();
        let statement_origin = origin.clone();
        write.task_head(fact, move |position| TaskHead {
            handle: statement_task,
            uuid: identity,
            title: format!("Allocate {name}"),
            description: None,
            acceptance: Vec::new(),
            project: "board.allocate".to_owned(),
            tags: vec![priority.to_owned()],
            dependencies: Vec::new(),
            provenance: statement_origin,
            revision: position,
        });
        write.workflow_head(task, fact, move |_| flow);
        if index == 0 && with_residency {
            write.residency_head(fact, |_| residency());
        }
    }
    journal.apply(write).expect("seed allocation authority");
    journal
}

fn append_hidden(journal: &mut Journal) -> TaskHandle {
    let task = handle("HIDDEN-z9");
    let origin = "task:HIDDEN-root"
        .parse::<Origin>()
        .expect("fixture origin");
    let flow = workflow();
    let mut write = JournalWrite::new(
        Actor::Spice,
        origin.clone(),
        instant(1),
        Producer::CommandDoor,
    );
    let fact = write
        .fact(
            body::TaskAdded {
                task: task.clone(),
                uuid: uuid(3),
                title: "Retain hidden evidence".to_owned(),
                description: Some("Hidden scheduling detail".to_owned()),
                acceptance: vec!["Hidden evidence settles".to_owned()],
                project: ".maxim_proposal.theme_2".to_owned(),
                tags: vec!["priority:critical".to_owned()],
                depends: Vec::new(),
                flow: flow.flow.clone(),
                phase: flow.phase.clone(),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("hidden task fact");
    let statement_task = task.clone();
    write.task_head(fact, move |position| TaskHead {
        handle: statement_task,
        uuid: uuid(3),
        title: "Retain hidden evidence".to_owned(),
        description: Some("Hidden scheduling detail".to_owned()),
        acceptance: vec!["Hidden evidence settles".to_owned()],
        project: ".maxim_proposal.theme_2".to_owned(),
        tags: vec!["priority:critical".to_owned()],
        dependencies: Vec::new(),
        provenance: origin,
        revision: position,
    });
    write.workflow_head(task.clone(), fact, move |_| flow);
    journal.apply(write).expect("seed hidden task");
    task
}

#[test]
fn journal_allocator_never_claims_hidden_rows_even_at_higher_urgency() {
    let mut journal = fixture(None, true);
    let hidden = append_hidden(&mut journal);

    let won = win(&mut journal, request(30)).expect("public allocator winner");
    assert_eq!(won.task(), &handle("BOARD-a1"));
    let snapshot = Snapshot::read(&journal, &projects()).expect("mixed snapshot");
    let hidden = snapshot
        .entries()
        .iter()
        .find(|entry| entry.statement().handle == hidden)
        .expect("hidden row retained");
    assert!(hidden.project().visibility().is_hidden());
    assert!(hidden.claim().is_none());
}

#[test]
fn allocator_claims_in_deterministic_order_and_never_claims_twice() {
    let mut journal = fixture(None, true);

    let first = win(&mut journal, request(31)).expect("highest urgency task wins");
    assert_eq!(first.task(), &handle("BOARD-a1"));
    assert_eq!(first.first(), first.last());

    let before_fault = journal.fact_snapshot().expect("before duplicate receipt");
    assert!(matches!(
        win(&mut journal, request(31)),
        Err(WinError::Commit(_))
    ));
    assert_eq!(
        journal
            .fact_snapshot()
            .expect("duplicate receipt rolls back"),
        before_fault
    );
    let after_fault = Snapshot::read(&journal, &projects()).expect("board after rolled-back claim");
    assert!(
        after_fault
            .entries()
            .iter()
            .find(|entry| entry.statement().handle == handle("BOARD-b2"))
            .expect("second task")
            .claim()
            .is_none()
    );

    let second = win(&mut journal, request(32)).expect("next eligible task wins");
    assert_eq!(second.task(), &handle("BOARD-b2"));
    assert_ne!(first.task(), second.task());

    let before_empty = journal.fact_snapshot().expect("fact snapshot");
    assert!(matches!(
        win(&mut journal, request(33)),
        Err(WinError::Refused(Refusal::Empty))
    ));
    let after_empty = journal.fact_snapshot().expect("fact snapshot");
    assert_eq!(after_empty, before_empty);

    let snapshot = Snapshot::read(&journal, &projects()).expect("claimed board snapshot");
    assert_eq!(snapshot.entries().len(), 2);
    for entry in snapshot.entries() {
        assert_eq!(
            entry.claim().map(|claim| claim.disposition),
            Some(ClaimDisposition::Active)
        );
        let lease = entry.lease().expect("winning claim carries a lease");
        assert_eq!(lease.start, instant(5));
        assert_eq!(lease.renewal, instant(5));
        assert_eq!(lease.expiry, instant(20));
        assert_eq!(lease.maximum, instant(35));
        assert_eq!(lease.epoch, uuid(22));
    }

    let facts = journal.fact_snapshot().expect("claim fact stream");
    assert_eq!(facts.facts().len(), 4);
    assert_eq!(facts.facts()[2].fact.kind(), FactKind::ClaimWon);
    assert_eq!(facts.facts()[3].fact.kind(), FactKind::ClaimWon);
    let verified = journal.verify().expect("claims and receipts verify");
    assert_eq!(verified.facts, 4);
    assert_eq!(verified.receipts, 2);
}

#[test]
fn invalid_rule_deadline_and_residency_leave_authority_untouched() {
    let cases = [
        {
            let mut request = request(41);
            request.rule = "other".parse().expect("alternate rule");
            let expected = Refusal::Rule(request.rule.clone());
            (request, expected)
        },
        {
            let mut request = request(42);
            request.expiry = request.at;
            let expected = Refusal::Deadline {
                at: request.at,
                expiry: request.expiry,
                maximum: request.maximum,
            };
            (request, expected)
        },
        {
            let mut request = request(43);
            request.epoch = uuid(23);
            let expected = Refusal::StaleEpoch {
                seated: uuid(22),
                named: uuid(23),
            };
            (request, expected)
        },
        {
            let mut request = request(44);
            request.occupant = uuid(24);
            let expected = Refusal::Displaced {
                lane: uuid(20),
                seated: Some(uuid(21)),
                named: uuid(24),
            };
            (request, expected)
        },
        {
            let mut request = request(45);
            request.actor = Actor::Lane(uuid(25));
            let expected = Refusal::ActorLane {
                actor: uuid(25),
                named: uuid(20),
            };
            (request, expected)
        },
    ];

    for (request, expected) in cases {
        let mut journal = fixture(None, true);
        let before = journal.fact_snapshot().expect("before refusal");
        match win(&mut journal, request) {
            Err(WinError::Refused(observed)) => assert_eq!(observed, expected),
            other => panic!("expected {expected:?}, found {other:?}"),
        }
        assert_eq!(journal.fact_snapshot().expect("after refusal"), before);
        assert_eq!(
            journal.verify().expect("fixture remains coherent").receipts,
            0
        );
    }

    let mut missing = fixture(None, false);
    let before = missing.fact_snapshot().expect("before missing residency");
    assert!(matches!(
        win(&mut missing, request(46)),
        Err(WinError::Refused(Refusal::MissingResidency))
    ));
    assert_eq!(missing.fact_snapshot().expect("after refusal"), before);
}

#[test]
fn a_claim_without_a_lease_is_a_zero_write_snapshot_fault() {
    let mut malformed = fixture(None, true);
    let task = handle("BOARD-a1");
    let claim_request = request(47);
    let mut write = JournalWrite::new(
        claim_request.actor.clone(),
        "task:BOARD-a1".parse().expect("claim origin"),
        claim_request.at,
        Producer::CommandDoor,
    );
    let fact = write
        .fact(
            body::ClaimWon {
                task: task.clone(),
                actor: claim_request.actor.clone(),
                lane: claim_request.lane,
                occupant: claim_request.occupant,
                epoch: claim_request.epoch,
                until: claim_request.expiry,
                git: None,
            },
            FactCause::Root,
        )
        .expect("malformed claim fact");
    write.claim_head(task, fact, move |position| ClaimHead {
        actor: claim_request.actor,
        fact: position,
        lane: claim_request.lane,
        occupant: claim_request.occupant,
        epoch: claim_request.epoch,
        disposition: ClaimDisposition::Active,
    });
    malformed
        .apply(write)
        .expect("seed claim without its required lease head");
    let before = malformed.fact_snapshot().expect("before malformed read");
    assert!(matches!(
        win(&mut malformed, request(48)),
        Err(WinError::Read(_))
    ));
    assert_eq!(
        malformed.fact_snapshot().expect("after malformed read"),
        before
    );
}

#[test]
fn a_disordered_lease_is_a_zero_write_snapshot_fault() {
    let mut disordered = fixture(None, true);
    let task = handle("BOARD-a1");
    let claim_request = request(49);
    let mut write = JournalWrite::new(
        claim_request.actor.clone(),
        "task:BOARD-a1".parse().expect("claim origin"),
        claim_request.at,
        Producer::CommandDoor,
    );
    let fact = write
        .fact(
            body::ClaimWon {
                task: task.clone(),
                actor: claim_request.actor.clone(),
                lane: claim_request.lane,
                occupant: claim_request.occupant,
                epoch: claim_request.epoch,
                until: claim_request.expiry,
                git: None,
            },
            FactCause::Root,
        )
        .expect("malformed lease fact");
    write.claim_head(task.clone(), fact, move |position| ClaimHead {
        actor: claim_request.actor,
        fact: position,
        lane: claim_request.lane,
        occupant: claim_request.occupant,
        epoch: claim_request.epoch,
        disposition: ClaimDisposition::Freed,
    });
    write.lease_head(task, fact, move |_| LeaseHead {
        start: instant(20),
        renewal: instant(5),
        expiry: instant(20),
        maximum: instant(35),
        epoch: uuid(22),
    });
    disordered
        .apply(write)
        .expect("seed a canonically encoded but disordered lease");
    let before = disordered
        .fact_snapshot()
        .expect("before disordered lease read");
    assert!(matches!(
        win(&mut disordered, request(50)),
        Err(WinError::Read(_))
    ));
    assert_eq!(
        disordered
            .fact_snapshot()
            .expect("after disordered lease refusal"),
        before
    );
}

#[test]
fn independent_connections_observe_the_same_single_allocation_path() {
    let store = Store::new();
    drop(fixture(Some(store.path()), true));
    let mut first = Journal::open(store.path()).expect("first process connection");
    let mut second = Journal::open(store.path()).expect("second process connection");

    assert_eq!(
        win(&mut first, request(51))
            .expect("first connection wins")
            .task(),
        &handle("BOARD-a1")
    );
    assert_eq!(
        win(&mut second, request(52))
            .expect("second connection sees the committed claim")
            .task(),
        &handle("BOARD-b2")
    );
    assert!(matches!(
        win(&mut first, request(53)),
        Err(WinError::Refused(Refusal::Empty))
    ));

    let snapshot = Snapshot::read(&second, &projects()).expect("shared authority snapshot");
    assert_eq!(
        snapshot
            .entries()
            .iter()
            .filter(|entry| entry
                .claim()
                .is_some_and(|claim| { claim.disposition == ClaimDisposition::Active }))
            .count(),
        2
    );
    assert_eq!(
        second.verify().expect("shared authority verifies").receipts,
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
                "spice-board-allocation-{}-{}.sqlite",
                std::process::id(),
                uuid(60),
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
                Err(error) => panic!("remove allocation fixture: {error}"),
            }
        }
    }
}
