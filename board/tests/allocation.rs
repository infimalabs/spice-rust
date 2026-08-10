use std::collections::BTreeSet;

use board::Snapshot;
use board::allocate::{Mint, mint};
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
        topology_revision: 0,
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
    }
}

fn fixture() -> Journal {
    let mut journal = Journal::in_memory().expect("in-memory allocation journal");
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
            queue_origin: position,
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
        if index == 0 {
            write.residency_head(fact, |_| residency());
        }
    }
    journal.apply(write).expect("seed allocation authority");
    journal
}

/// A proven boundary carrying one accounted head.
///
/// Deliberately not an empty `Proven`: an empty one is byte-identical for every
/// lane, so a claim fact that dropped the caller's boundary entirely would
/// still compare equal to this fixture and the loss would not show.
fn source_boundary(lane: CanonicalUuid) -> body::SourceBoundary {
    body::SourceBoundary::Proven {
        heads: vec![wire::TranscriptSourceHead {
            lane,
            vendor: "claude".parse().expect("fixture vendor"),
            stream: "session".to_owned(),
            source: uuid(23),
            next: body::TranscriptPosition {
                member: 4,
                offset: 512,
            },
        }],
    }
}

/// Open a transaction on the fixture's task and mint one claim into it.
fn minted(journal: &mut Journal, git: Option<body::GitBoundary>, source: body::SourceBoundary) {
    let task = handle("BOARD-a1");
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
            task: &task,
            actor: &actor,
            workflow: &workflow(),
            project: "board.allocate",
            lane: uuid(20),
            occupant: uuid(21),
            epoch: uuid(22),
            at: instant(5),
            expiry: instant(20),
            maximum: instant(35),
            git,
            source,
        },
    )
    .expect("the claim mints");
    journal.apply(write).expect("the minted claim commits");
}

/// Everything a claim carries reaches the journal through the one minter.
///
/// This replaced two independent minting sites, and the cost of that split was
/// that a field added to the claim had to be copied into both by hand. So the
/// guard is not that the minter works, which the fixture above already shows,
/// but that nothing a claim carries can be added without landing here: the fact
/// body is named field by field, and both heads are destructured whole, so a
/// field added to either one stops compiling until someone says where the
/// minter gets it from.
#[test]
fn the_one_claim_minter_writes_every_lifecycle_field() {
    let mut journal = fixture();
    minted(
        &mut journal,
        None,
        body::SourceBoundary::Proven { heads: Vec::new() },
    );

    let facts = journal.fact_snapshot().expect("claim fact stream");
    let claim = facts
        .facts()
        .iter()
        .find(|envelope| envelope.fact.kind() == FactKind::ClaimWon)
        .expect("minting writes a claim fact");
    // Named one by one rather than counted, because a count admits a rename.
    // `git` is absent because this claim was minted without a boundary, and the
    // field is skipped when it is empty; the test below mints one with it.
    // `source` is present for the same claim, because an empty proven boundary
    // is a boundary that was read and found empty; only the value the legacy
    // migration supplies for facts stored before boundaries were recorded is
    // skipped, and no live minter can produce it.
    let written: BTreeSet<&str> = claim.fact.body().keys().map(String::as_str).collect();
    assert_eq!(
        written,
        BTreeSet::from([
            "task", "actor", "lane", "occupant", "epoch", "cell", "until", "source",
        ]),
        "the claim fact carries exactly the fields the minter writes"
    );

    let snapshot = Snapshot::read(&journal, &projects()).expect("claimed board");
    let entry = snapshot
        .entries()
        .iter()
        .find(|entry| entry.statement().handle == handle("BOARD-a1"))
        .expect("the claimed task is on the board");
    let ClaimHead {
        actor,
        fact,
        lane,
        occupant,
        epoch,
        disposition,
        claimed,
        cell,
    } = entry.claim().expect("minting writes a claim head");
    assert_eq!(actor, &Actor::Lane(uuid(20)));
    assert_eq!(*fact, claim.seq);
    assert_eq!(*claimed, claim.seq);
    assert_eq!(cell.project, "board.allocate");
    assert_eq!(cell.phase, phase("todo"));
    assert_eq!(*lane, uuid(20));
    assert_eq!(*occupant, uuid(21));
    assert_eq!(*epoch, uuid(22));
    assert_eq!(*disposition, ClaimDisposition::Active);

    let LeaseHead {
        start,
        renewal,
        expiry,
        maximum,
        epoch,
    } = entry.lease().expect("minting writes a lease head");
    assert_eq!(*start, instant(5));
    assert_eq!(*renewal, instant(5));
    assert_eq!(*expiry, instant(20));
    assert_eq!(*maximum, instant(35));
    assert_eq!(*epoch, uuid(22));
}

/// A boundary the caller supplies survives the minter it is handed to.
///
/// `git` is the field the two minting sites actually disagreed about: the
/// allocator hardcoded it empty while the resident threaded a real worktree
/// through, which is why it is the one worth proving reaches the journal from a
/// caller rather than from the writer's own defaults.
///
/// `source` is proven beside it for the same reason and one more: the minter is
/// handed an already-open transaction, so it cannot read a transcript boundary
/// itself and has no default to fall back to. Whatever the caller passes is the
/// only boundary the claim can ever open at.
#[test]
fn a_caller_supplied_boundary_reaches_the_claim() {
    let mut journal = fixture();
    let boundary = body::GitBoundary {
        head: "a-recorded-commit".to_owned(),
        integrated: "an-integrated-commit".to_owned(),
        upstream: Some("origin/main".to_owned()),
        upstream_head: Some("a-baseline-commit".to_owned()),
        local_commits: 2,
    };
    minted(&mut journal, Some(boundary), source_boundary(uuid(20)));

    let facts = journal.fact_snapshot().expect("claim fact stream");
    let claim = facts
        .facts()
        .iter()
        .find(|envelope| envelope.fact.kind() == FactKind::ClaimWon)
        .expect("minting writes a claim fact");
    let git = claim
        .fact
        .body()
        .get("git")
        .expect("the claim carries the boundary it was minted with");
    // Read back field by field rather than compared against the fixture struct,
    // so the assertion cannot pass by echoing its own input through the same
    // serializer that wrote it.
    assert_eq!(
        git.get("head").and_then(|value| value.as_str()),
        Some("a-recorded-commit")
    );
    assert_eq!(
        git.get("integrated").and_then(|value| value.as_str()),
        Some("an-integrated-commit")
    );
    assert_eq!(
        git.get("upstream").and_then(|value| value.as_str()),
        Some("origin/main")
    );
    assert_eq!(
        git.get("upstream_head").and_then(|value| value.as_str()),
        Some("a-baseline-commit")
    );
    assert_eq!(
        git.get("local_commits").and_then(|value| value.as_u64()),
        Some(2)
    );

    let head = claim
        .fact
        .body()
        .get("source")
        .and_then(|source| source.get("proven"))
        .and_then(|proven| proven.get("heads"))
        .and_then(|heads| heads.get(0))
        .expect("the claim carries the accounted head it was minted with");
    assert_eq!(
        head.get("lane").and_then(|value| value.as_str()),
        Some(uuid(20).to_string().as_str())
    );
    assert_eq!(
        head.get("stream").and_then(|value| value.as_str()),
        Some("session")
    );
    assert_eq!(
        head.get("next")
            .and_then(|next| next.get("offset"))
            .and_then(|value| value.as_u64()),
        Some(512)
    );
}
