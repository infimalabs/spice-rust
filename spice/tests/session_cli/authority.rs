use std::fs;
use std::path::{Path, PathBuf};

use board::allocate::{Mint, mint};
use journal::{FactCause, Journal, JournalWrite};
use rusqlite::{Connection, params};
use serde_json::json;
use wire::{
    Actor, CanonicalUuid, Disposition, Instant, LaneResidency, Origin, Phase, Producer,
    ResidencyHead, ResidentState, SteeringHead, TaskStamp, WorkflowHead, body,
};

use super::{Scratch, codex_rows, reply_forms_in, strings, write_jsonl};

const SESSION_PROJECT: &str = "spice.session";

fn fake_repository(root: &Path) {
    fs::create_dir(root.join(".git")).expect("fixture git directory is created");
    fs::write(
        root.join("spice-rust.toml"),
        "[tasks]\nstems = [\"spice\"]\nproject_min_depth = 2\nproject_max_depth = 3\n",
    )
    .expect("fixture tracked task policy is written");
}

fn authority_path(root: &Path) -> PathBuf {
    root.join(".git/.spice-rs/data/spicejournal.sqlite3")
}

fn uuid(tail: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{tail:02x}")
        .parse()
        .expect("fixture uuid")
}

fn instant(minute: u8) -> Instant {
    format!("2026-08-01T06:{minute:02}:00.000Z")
        .parse()
        .expect("fixture instant")
}

fn phase(name: &str) -> Phase {
    name.parse().expect("fixture phase")
}

fn stamp(text: &str) -> TaskStamp {
    let value = text.bytes().fold(0_i64, |value, byte| {
        (value * 131 + i64::from(byte)) % 10_000_000_000_000
    });
    TaskStamp::for_millis(value).expect("fixture name fits the task stamp codec")
}

fn workflow(name: &str) -> WorkflowHead {
    WorkflowHead {
        flow: vec![phase("todo"), phase("review")],
        phase: phase(name),
        status: "pending".to_owned(),
        review: "none".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    }
}

fn task_added(
    write: &mut JournalWrite,
    origin: Origin,
    task: TaskStamp,
    title: &str,
    project: &str,
    phase: &str,
) {
    let workflow = workflow(phase);
    let title = title.to_owned();
    let project = project.to_owned();
    let fact = write
        .fact(
            body::TaskAdded {
                task: task.clone(),
                title: title.clone(),
                description: None,
                acceptance: Vec::new(),
                project: project.clone(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: workflow.flow.clone(),
                phase: workflow.phase.clone(),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("task fact builds");
    let statement_task = task.clone();
    write.task_head(fact, move |position| wire::TaskHead {
        born: statement_task,
        queue_origin: position,
        title,
        description: None,
        acceptance: Vec::new(),
        project,
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: origin,
        revision: position,
    });
    write.workflow_head(task, fact, move |_| workflow);
}

fn seed_recall_authority(path: &Path, thread: &str) {
    fs::create_dir_all(path.parent().expect("authority path has a parent"))
        .expect("authority parent is created");
    let mut journal = Journal::open(path).expect("fixture journal opens");
    let lane = uuid(0xaa);
    let occupant = uuid(0xab);
    let epoch = uuid(0xac);
    let peer_lane = uuid(0xad);
    let peer_occupant = uuid(0xae);
    let origin = "task:SESSION-fixture"
        .parse::<Origin>()
        .expect("fixture origin");
    let held = stamp("SESSION-held");
    let peer = stamp("SESSION-peer");
    let mut write = JournalWrite::new(
        Actor::Spice,
        origin.clone(),
        instant(0),
        Producer::CommandDoor,
    );
    let seated = write
        .fact(body::UpSeated { epoch }, FactCause::Root)
        .expect("seating fact builds");
    let lanes = recall_lanes(lane, occupant, peer_lane, peer_occupant, epoch, thread);
    write.residency_head(seated, move |_| ResidencyHead {
        epoch,
        state: ResidentState::Seated,
        topology_revision: 0,
        lanes,
        subscriptions: Vec::new(),
    });
    task_added(
        &mut write,
        origin.clone(),
        held.clone(),
        "Compose Rust briefings",
        SESSION_PROJECT,
        "todo",
    );
    task_added(
        &mut write,
        origin.clone(),
        peer.clone(),
        "Peer active claim",
        SESSION_PROJECT,
        "todo",
    );
    let steering = body::SteerSent {
        directive: uuid(0xaf),
        fanout: None,
        key: "1kAsk001".to_owned(),
        lane,
        text: "Address this exact steering ask".to_owned(),
        control: None,
        attachments: vec![
            wire::Media::from_base64("operator-capture.png", "image/png", "iVBORw0KGgo=")
                .expect("fixture media admits"),
        ],
        expires: instant(30),
    };
    let steering_fact = write
        .fact(steering.clone(), FactCause::Root)
        .expect("steering fact builds");
    let steering_origin = origin.clone();
    write.steering_head(steering_fact, move |created| SteeringHead {
        directive: steering.directive,
        fanout: steering.fanout,
        key: steering.key,
        lane: steering.lane,
        text: steering.text,
        control: steering.control,
        attachments: steering.attachments,
        published: instant(0),
        expires: steering.expires,
        stage: body::SteeringStage::Published,
        escalations: Vec::new(),
        disposition: Disposition::Live,
        origin: steering_origin,
        created,
        minted_millis: None,
    });
    journal.apply(write).expect("fixture authority seeds");

    mint_claim(
        &mut journal,
        held,
        Actor::Lane(lane),
        lane,
        occupant,
        epoch,
        SESSION_PROJECT,
        instant(1),
    );
    mint_claim(
        &mut journal,
        peer,
        Actor::Lane(peer_lane),
        peer_lane,
        peer_occupant,
        epoch,
        SESSION_PROJECT,
        instant(2),
    );
}

fn seed_retired_peer_steering(path: &Path) {
    let mut journal = Journal::open(path).expect("fixture journal reopens");
    let directive = uuid(0xb0);
    let lane = uuid(0xad);
    let origin = "task:SESSION-fixture"
        .parse::<Origin>()
        .expect("fixture origin");
    let mut sent = JournalWrite::new(
        Actor::Spice,
        origin.clone(),
        instant(3),
        Producer::CommandDoor,
    );
    let body = body::SteerSent {
        directive,
        fanout: None,
        key: "1kRetire".to_owned(),
        lane,
        text: "unrelated retired steering".to_owned(),
        control: None,
        attachments: Vec::new(),
        expires: instant(30),
    };
    let fact = sent
        .fact(body.clone(), FactCause::Root)
        .expect("peer steering fact builds");
    let head_origin = origin.clone();
    sent.steering_head(fact, move |created| SteeringHead {
        directive: body.directive,
        fanout: body.fanout,
        key: body.key,
        lane: body.lane,
        text: body.text,
        control: body.control,
        attachments: body.attachments,
        published: instant(3),
        expires: body.expires,
        stage: body::SteeringStage::Published,
        escalations: Vec::new(),
        disposition: Disposition::Live,
        origin: head_origin,
        created,
        minted_millis: None,
    });
    journal
        .apply(sent)
        .expect("peer steering publication commits");
    let current = journal
        .read(|read| read.steering_head(directive))
        .expect("peer steering head reads")
        .expect("peer steering head exists");
    let mut canceled =
        JournalWrite::new(Actor::Operator, origin, instant(4), Producer::CommandDoor);
    let cancellation = canceled
        .fact(
            body::SteerCanceled {
                key: current.body().key.clone(),
                lane,
            },
            FactCause::Existing(current.body().created),
        )
        .expect("peer cancellation fact builds");
    let mut retired = current.into_body();
    retired.disposition = Disposition::Canceled;
    canceled.steering_head(cancellation, move |_| retired);
    journal
        .apply(canceled)
        .expect("peer steering retirement commits");
}

fn recall_lanes(
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    peer_lane: CanonicalUuid,
    peer_occupant: CanonicalUuid,
    epoch: CanonicalUuid,
    thread: &str,
) -> Vec<LaneResidency> {
    vec![
        LaneResidency {
            lane,
            lineage_id: lane,
            name: None,
            worktree: "/fixture".to_owned(),
            life: "drive".to_owned(),
            vendor: "codex".parse().expect("fixture vendor"),
            occupant: Some(occupant),
            lineage: vec![occupant],
            capabilities: Vec::new(),
            session: Some(wire::LaneSession {
                lane,
                occupant,
                worktree: "/fixture".to_owned(),
                vendor: "codex".parse().expect("fixture vendor"),
                launch: None,
                epoch,
                stream: "session".to_owned(),
                identity: thread.to_owned(),
                locus: wire::TapeLocus {
                    member: 0,
                    offset: 0,
                    length: thread.len() as u64,
                },
            }),
            disposition: "occupied".to_owned(),
        },
        LaneResidency {
            lane: peer_lane,
            lineage_id: peer_lane,
            name: None,
            worktree: "/fixture-peer".to_owned(),
            life: "drive".to_owned(),
            vendor: "codex".parse().expect("fixture vendor"),
            occupant: Some(peer_occupant),
            lineage: vec![peer_occupant],
            capabilities: Vec::new(),
            session: None,
            disposition: "occupied".to_owned(),
        },
    ]
}

#[allow(
    clippy::too_many_arguments,
    reason = "claim fixture states every durable identity and residency carrier"
)]
fn mint_claim(
    journal: &mut Journal,
    task: TaskStamp,
    actor: Actor,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    epoch: CanonicalUuid,
    project: &str,
    at: Instant,
) {
    let mut write = JournalWrite::new(
        actor.clone(),
        "task:SESSION-fixture"
            .parse()
            .expect("fixture claim origin"),
        at,
        Producer::CommandDoor,
    );
    mint(
        &mut write,
        Mint {
            task: &task,
            actor: &actor,
            workflow: &workflow("todo"),
            project,
            lane,
            occupant,
            epoch,
            at,
            expiry: instant(40),
            maximum: instant(50),
            git: None,
            source: body::SourceBoundary::Proven { heads: Vec::new() },
        },
    )
    .expect("claim mints");
    journal.apply(write).expect("claim commits");
}

#[test]
fn recall_authority_probe_is_read_only_when_repository_state_is_absent() {
    let scratch = Scratch::new("authority-absent");
    fake_repository(scratch.path());
    let transcript = scratch.path().join("codex.jsonl");
    write_jsonl(&transcript, &codex_rows());

    let recall = reply_forms_in(
        Some(scratch.path()),
        &strings(&[
            "session",
            "briefing",
            &transcript.to_string_lossy(),
            "--deadline-seconds",
            "5",
        ]),
        &[],
        "recall",
    );

    assert_eq!(recall["authorities"]["fold"]["mode"], "bounded-read-only");
    assert_eq!(recall["authorities"]["fold"]["writes"], json!([]));
    assert!(
        recall["authorities"].get("asks").is_none(),
        "empty outstanding asks are omitted"
    );
    assert!(
        recall["authorities"]["unknowns"]
            .as_array()
            .expect("unknowns are explicit")
            .iter()
            .any(|row| row["authority"] == "journal" && row["probe"] == "open-read-only"),
        "missing journal authority is reported as unknown"
    );
    assert!(
        !scratch.path().join(".git/.spice-rs").exists(),
        "read-only observation does not initialize repository state"
    );

    scratch.remove();
}

#[test]
fn recall_authority_sources_asks_and_held_work_from_exact_state_authorities() {
    let scratch = Scratch::new("authority-present");
    fake_repository(scratch.path());
    let transcript = scratch.path().join("codex.jsonl");
    write_jsonl(&transcript, &codex_rows());
    let thread = uuid(0xaa).to_string().replace('-', "");
    seed_recall_authority(&authority_path(scratch.path()), &thread);
    let environment = [("CODEX_THREAD_ID", thread.clone())]; // env-policy: allow

    let recall = reply_forms_in(
        Some(scratch.path()),
        &strings(&[
            "session",
            "briefing",
            &transcript.to_string_lossy(),
            "--start",
            "2026-01-01T00:00:00Z",
            "--max-lines",
            "80",
            "--max-bytes",
            "8000",
            "--deadline-seconds",
            "5",
        ]),
        &environment,
        "recall",
    );

    assert_eq!(recall["authorities"]["asks"][0]["key"], "1kAsk001");
    assert_eq!(
        recall["authorities"]["asks"][0]["text"],
        "Address this exact steering ask"
    );
    assert_eq!(
        recall["authorities"]["asks"][0]["attachments"][0]["name"],
        "operator-capture.png",
    );
    assert_eq!(
        recall["authorities"]["asks"][0]["attachments"][0]["data_url"],
        "data:image/png;base64,iVBORw0KGgo=",
    );
    assert_eq!(
        recall["authorities"]["held_work"]["own_claim"]["title"],
        "Compose Rust briefings"
    );
    assert_eq!(recall["authorities"]["held_work"]["peer_claims"], 1);
    assert_eq!(
        recall["authorities"]["priority"]["order"],
        json!([
            "outstanding-asks",
            "held-work",
            "transcript-deliveries",
            "transcript-working-set",
            "ambient-posture"
        ])
    );
    assert_eq!(
        recall["window"]["turns"][0]["touched_paths"],
        json!(["spice/src/session.rs"])
    );

    let pruned = reply_forms_in(
        Some(scratch.path()),
        &strings(&[
            "session",
            "briefing",
            &transcript.to_string_lossy(),
            "--start",
            "2026-01-01T00:00:00Z",
            "--max-lines",
            "1",
            "--max-bytes",
            "1",
            "--deadline-seconds",
            "5",
        ]),
        &environment,
        "recall",
    );
    assert!(
        pruned["pruning"]["turns_before"].as_u64() > pruned["pruning"]["turns_after"].as_u64(),
        "end pruning removed transcript turns"
    );
    assert_eq!(pruned["authorities"]["asks"][0]["key"], "1kAsk001");
    assert_eq!(
        pruned["authorities"]["asks"][0]["attachments"][0]["name"], "operator-capture.png",
        "end pruning preserves typed outstanding-ask media",
    );
    assert_eq!(
        pruned["authorities"]["held_work"]["own_claim"]["title"],
        "Compose Rust briefings"
    );
    assert_eq!(
        pruned["authorities"]["priority"]["end_pruning_preserves"],
        json!(["outstanding-asks", "held-work"])
    );

    scratch.remove();
}

#[test]
fn recall_ignores_corrupt_fact_and_retired_steering_outside_the_exact_locus() {
    let scratch = Scratch::new("authority-bounded-corruption");
    fake_repository(scratch.path());
    let transcript = scratch.path().join("codex.jsonl");
    write_jsonl(&transcript, &codex_rows());
    let thread = uuid(0xaa).to_string().replace('-', "");
    let authority = authority_path(scratch.path());
    seed_recall_authority(&authority, &thread);
    seed_retired_peer_steering(&authority);
    let connection = Connection::open(&authority).expect("fixture authority opens for corruption");
    connection
        .execute(
            "UPDATE fact_envelopes SET envelope = ?1 \
             WHERE seq = (SELECT max(seq) FROM fact_envelopes WHERE kind = 'task::added')",
            params![b"corrupt unrelated fact".to_vec()],
        )
        .expect("corrupt one unrelated fact body");
    connection
        .execute(
            "UPDATE steering_heads SET body = ?1 WHERE disposition = 'canceled'",
            params![b"corrupt retired steering".to_vec()],
        )
        .expect("corrupt retired steering body");
    drop(connection);

    let recall = reply_forms_in(
        Some(scratch.path()),
        &strings(&[
            "session",
            "briefing",
            &transcript.to_string_lossy(),
            "--deadline-seconds",
            "5",
        ]),
        &[("CODEX_THREAD_ID", thread)], // env-policy: allow
        "recall",
    );

    assert_eq!(recall["authorities"]["asks"][0]["key"], "1kAsk001");
    assert_eq!(
        recall["authorities"]["held_work"]["own_claim"]["title"],
        "Compose Rust briefings"
    );
    assert_eq!(recall["authorities"]["held_work"]["peer_claims"], 1);
    assert!(
        recall["authorities"].get("unknowns").is_none(),
        "corruption outside every selected keyspace is not decoded"
    );

    scratch.remove();
}

#[test]
fn a_failed_steering_probe_does_not_hide_independent_board_answers() {
    let scratch = Scratch::new("authority-probe-isolation");
    fake_repository(scratch.path());
    let transcript = scratch.path().join("codex.jsonl");
    write_jsonl(&transcript, &codex_rows());
    let thread = uuid(0xaa).to_string().replace('-', "");
    let authority = authority_path(scratch.path());
    seed_recall_authority(&authority, &thread);
    let connection = Connection::open(&authority).expect("fixture authority opens for corruption");
    connection
        .execute(
            "UPDATE steering_heads SET body = ?1 WHERE disposition = 'live'",
            params![b"corrupt selected steering".to_vec()],
        )
        .expect("corrupt selected steering body");
    drop(connection);

    let recall = reply_forms_in(
        Some(scratch.path()),
        &strings(&[
            "session",
            "briefing",
            &transcript.to_string_lossy(),
            "--deadline-seconds",
            "5",
        ]),
        &[("CODEX_THREAD_ID", thread)], // env-policy: allow
        "recall",
    );

    assert!(
        recall["authorities"].get("asks").is_none(),
        "failed selected steering is not rendered as an empty known inbox"
    );
    assert_eq!(
        recall["authorities"]["held_work"]["own_claim"]["title"],
        "Compose Rust briefings"
    );
    assert_eq!(recall["authorities"]["held_work"]["peer_claims"], 1);
    assert_eq!(
        recall["authorities"]["unknowns"]
            .as_array()
            .expect("failed probe is explicit")
            .iter()
            .filter(|row| row["authority"] == "steering")
            .count(),
        1
    );

    scratch.remove();
}
