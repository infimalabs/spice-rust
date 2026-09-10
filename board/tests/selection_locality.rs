use board::Entry;
use journal::{FactCause, Journal, JournalWrite};
use wire::{
    Actor, CanonicalUuid, ClaimDisposition, ClaimHead, Instant, LeaseHead, Origin, Phase, Producer,
    Seq, TaskHead, TaskStamp, WorkflowHead, body,
};

fn stamp(text: &str) -> TaskStamp {
    let value = text.bytes().fold(0_i64, |value, byte| {
        (value * 131 + i64::from(byte)) % 10_000_000_000_000
    });
    TaskStamp::for_micros(value).expect("fixture name fits the task stamp codec")
}

fn phase(text: &str) -> Phase {
    text.parse().expect("phase")
}

fn uuid(text: &str) -> CanonicalUuid {
    text.parse().expect("canonical UUID")
}

fn instant(text: &str) -> Instant {
    text.parse().expect("canonical instant")
}

fn seq(value: u64) -> Seq {
    Seq::new(value).expect("non-zero sequence")
}

fn environment() -> wire::Environment {
    wire::Environment::journal_schema(journal::SCHEMA_VERSION)
        .expect("the compiled journal schema is positive")
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

fn all_projects() -> wire::LineageSubscription {
    wire::LineageSubscription {
        lineage_id: uuid("019fbbde-5105-75a3-a76a-722afde1e408"),
        policy: wire::intent::LineageLife::Drain,
        manual_projects: Vec::new(),
        auto_projects: Vec::new(),
    }
}

fn workflow(current: &str) -> WorkflowHead {
    WorkflowHead {
        flow: vec![phase("todo"), phase("review")],
        phase: phase(current),
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    }
}

fn statement(name: &str) -> TaskHead {
    TaskHead {
        born: stamp(name),
        queue_origin: seq(1),
        title: format!("Implement {name}"),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: "board.kernel".to_owned(),
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: "task:BOARD-root".parse::<Origin>().expect("origin"),
        creation: wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::SpiceActor,
        ),
        revision: seq(1),
    }
}

fn lease() -> LeaseHead {
    LeaseHead {
        start: instant("2026-08-01T06:00:00.000Z"),
        renewal: instant("2026-08-01T06:00:00.000Z"),
        expiry: instant("2026-08-01T06:10:00.000Z"),
        maximum: instant("2026-08-01T06:30:00.000Z"),
        epoch: uuid("019fbbde-5105-75a3-a76a-722afde1e410"),
    }
}

fn placed_entry(name: &str, project: &str, current: &str, queue_origin: u64) -> Entry {
    let mut head = statement(name);
    head.project = project.to_owned();
    head.queue_origin = seq(queue_origin);
    Entry::new(head, workflow(current), None, None, &projects()).expect("placed candidate")
}

fn peer_entry(
    name: &str,
    actor: Actor,
    row_project: &str,
    cell_project: &str,
    current: &str,
) -> Entry {
    let mut head = statement(name);
    head.project = row_project.to_owned();
    let claim = ClaimHead {
        actor,
        fact: seq(2),
        claimed: seq(2),
        cell: wire::ClaimCell {
            project: cell_project.to_owned(),
            phase: phase(current),
        },
        lane: uuid("019fbbde-5105-75a3-a76a-722afde1e408"),
        occupant: uuid("019fbbde-5105-75a3-a76a-722afde1e409"),
        epoch: uuid("019fbbde-5105-75a3-a76a-722afde1e410"),
        recovery: None,
        disposition: ClaimDisposition::Active,
    };
    Entry::new(
        head,
        workflow(current),
        Some(claim),
        Some(lease()),
        &projects(),
    )
    .expect("active peer")
}

#[test]
fn peer_spread_reads_the_whole_board_cell_and_never_excludes_crowded_work() {
    let actor = Actor::Spice;
    let other = Actor::Lane(uuid("019fbbde-5105-75a3-a76a-722afde1e499"));
    let crowded = placed_entry("BOARD-a1", "board.alpha", "todo", 1);
    let open = placed_entry("BOARD-z1", "board.beta", "todo", 1);
    let peer = peer_entry(
        "BOARD-peer1",
        other.clone(),
        ".oops.correctness",
        "board.alpha",
        "todo",
    );
    let entries = [crowded.clone(), open.clone(), peer];

    let selected = board::select(&entries, &actor, &all_projects(), 2, None, &environment())
        .expect("an uncrowded cell is preferred");
    assert_eq!(selected.entry().born(), &stamp("BOARD-z1"));
    assert_eq!(
        selected.entry().handle(),
        &wire::TaskHandle::for_project(
            &selected.entry().statement().project,
            selected.entry().born(),
        ),
        "selection exposes the project-sensitive rendering of the durable stamp",
    );

    let second_peer = peer_entry("BOARD-peer2", other, ".oops.workflow", "board.beta", "todo");
    let all_crowded = [crowded, open, entries[2].clone(), second_peer];
    let selected = board::select(
        &all_crowded,
        &actor,
        &all_projects(),
        2,
        None,
        &environment(),
    )
    .expect("crowding remains a preference rather than an exclusion");
    assert_eq!(selected.entry().born(), &stamp("BOARD-a1"));
}

#[test]
fn peer_spread_precedes_movement_inside_the_shared_cell_vocabulary() {
    let actor = Actor::Spice;
    let recent = wire::ClaimCell {
        project: "board.alpha".to_owned(),
        phase: phase("todo"),
    };
    let local_but_crowded = placed_entry("BOARD-a1", "board.alpha", "todo", 1);
    let moved_but_open = placed_entry("BOARD-z1", "board.beta", "todo", 1);
    let peer = peer_entry(
        "BOARD-peer1",
        Actor::Lane(uuid("019fbbde-5105-75a3-a76a-722afde1e499")),
        ".oops.correctness",
        "board.alpha",
        "todo",
    );
    let entries = [local_but_crowded, moved_but_open, peer];

    let selected = board::select(
        &entries,
        &actor,
        &all_projects(),
        2,
        Some(&recent),
        &environment(),
    )
    .expect("peer spread speaks before affinity");
    assert_eq!(selected.entry().born(), &stamp("BOARD-z1"));
}

#[test]
fn movement_counts_project_and_phase_separately_from_only_the_recent_cell() {
    let recent = wire::ClaimCell {
        project: "board.alpha".to_owned(),
        phase: phase("todo"),
    };
    let same = placed_entry("BOARD-z0", "board.alpha", "todo", 1);
    let phase_only = placed_entry("BOARD-z1", "board.alpha", "review", 1);
    let project_only = placed_entry("BOARD-z2", "board.beta", "todo", 1);
    let both = placed_entry("BOARD-a1", "board.beta", "review", 1);

    let entries = [both.clone(), phase_only.clone(), project_only.clone(), same];
    let selected = board::select(
        &entries,
        &Actor::Spice,
        &all_projects(),
        2,
        Some(&recent),
        &environment(),
    )
    .expect("cost zero wins");
    assert_eq!(selected.entry().born(), &stamp("BOARD-z0"));

    let one_coordinate = [project_only, phase_only.clone()];
    let selected = board::select(
        &one_coordinate,
        &Actor::Spice,
        &all_projects(),
        2,
        Some(&recent),
        &environment(),
    )
    .expect("changing project and changing phase each cost one");
    assert_eq!(selected.entry().born(), &stamp("BOARD-z1"));

    let moved = [both, phase_only];
    let selected = board::select(
        &moved,
        &Actor::Spice,
        &all_projects(),
        2,
        Some(&recent),
        &environment(),
    )
    .expect("one-coordinate movement beats changing both");
    assert_eq!(selected.entry().born(), &stamp("BOARD-z1"));
}

#[test]
fn locality_is_neutral_outside_the_narrow_urgency_band() {
    let recent = wire::ClaimCell {
        project: "board.local".to_owned(),
        phase: phase("todo"),
    };
    let oldest_remote = placed_entry("BOARD-z1", "board.remote", "review", 1);
    let near_local = placed_entry("BOARD-y1", "board.local", "todo", 3);
    let far_local = placed_entry("BOARD-a1", "board.local", "todo", 4);
    let entries = [far_local.clone(), oldest_remote.clone(), near_local];

    let selected = board::select(
        &entries,
        &Actor::Spice,
        &all_projects(),
        2,
        Some(&recent),
        &environment(),
    )
    .expect("locality may speak only for the in-band rows");
    assert_eq!(selected.entry().born(), &stamp("BOARD-y1"));

    let outside = [far_local, oldest_remote];
    let selected = board::select(
        &outside,
        &Actor::Spice,
        &all_projects(),
        2,
        Some(&recent),
        &environment(),
    )
    .expect("the out-of-band local row remains workable but cannot jump age");
    assert_eq!(selected.entry().born(), &stamp("BOARD-z1"));
}

#[test]
fn affinity_history_keeps_only_the_most_recent_claimed_cell() {
    let mut journal = Journal::in_memory().expect("claim history journal");
    let mut write = JournalWrite::new(
        Actor::Spice,
        "task:BOARD-history".parse().expect("history origin"),
        instant("2026-08-01T06:00:00.000Z"),
        Producer::CommandDoor,
    );
    for (task, project, current) in [
        ("BOARD-h1", "board.first", "todo"),
        ("BOARD-h2", "board.second", "review"),
        ("BOARD-h3", "board.third", "todo"),
    ] {
        write
            .fact(
                body::ClaimWon {
                    task: stamp(task),
                    actor: Actor::Spice,
                    lane: uuid("019fbbde-5105-75a3-a76a-722afde1e408"),
                    occupant: uuid("019fbbde-5105-75a3-a76a-722afde1e409"),
                    epoch: uuid("019fbbde-5105-75a3-a76a-722afde1e410"),
                    cell: Some(wire::ClaimCell {
                        project: project.to_owned(),
                        phase: phase(current),
                    }),
                    until: instant("2026-08-01T06:10:00.000Z"),
                    git: None,
                    recovery: None,
                    source: body::SourceBoundary::Proven { heads: Vec::new() },
                },
                FactCause::Root,
            )
            .expect("claim history fact");
    }
    journal.apply(write).expect("claim history commits");
    let facts = journal.fact_snapshot().expect("claim history reads");

    assert_eq!(
        board::recent_claim_cell(&[], facts.facts(), &Actor::Spice),
        Some(wire::ClaimCell {
            project: "board.third".to_owned(),
            phase: phase("todo"),
        }),
    );
}
