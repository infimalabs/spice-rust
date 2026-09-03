use std::fs;
use std::path::{Path, PathBuf};

use board::allocate::{Mint, mint};
use journal::{FactCause, Journal, JournalWrite};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
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

fn seed_codex_transcripts(root: &Path, threads: &[String]) -> PathBuf {
    let home = root.join("codex-home");
    let sessions = home.join("sessions");
    fs::create_dir_all(&sessions).expect("fixture Codex session root is created");
    let state = Connection::open(home.join("state_5.sqlite")).expect("fixture Codex state opens");
    state
        .execute("CREATE TABLE threads (id TEXT, rollout_path TEXT)", [])
        .expect("fixture Codex thread index is created");
    for thread in threads {
        let transcript = sessions.join(format!("rollout-{thread}.jsonl"));
        write_jsonl(&transcript, &codex_rows());
        state
            .execute(
                "INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)",
                params![thread, transcript.to_string_lossy()],
            )
            .expect("fixture Codex thread is indexed");
    }
    home
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
                creation: body::TaskCreationContext::unavailable(
                    body::TaskCreationUnavailable::SpiceActor,
                ),
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
        creation: body::TaskCreationContext::unavailable(body::TaskCreationUnavailable::SpiceActor),
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
        "review",
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
        "review",
        instant(1),
        instant(2),
        instant(2),
    );
    mint_claim(
        &mut journal,
        peer,
        Actor::Lane(peer_lane),
        peer_lane,
        peer_occupant,
        epoch,
        SESSION_PROJECT,
        "todo",
        instant(2),
        instant(40),
        instant(50),
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

fn renew_held_claim_for_meter(path: &Path, seconds: i64) {
    renew_held_claim_for_meter_with_extension(path, seconds, 3_600);
}

fn renew_maximum_bound_claim_for_meter(path: &Path, seconds: i64) {
    renew_held_claim_for_meter_with_extension(path, seconds, 0);
}

fn renew_held_claim_for_meter_with_extension(
    path: &Path,
    seconds: i64,
    maximum_extension_seconds: i64,
) {
    let task = stamp("SESSION-held");
    let journal = Journal::open(path).expect("fixture journal reopens");
    let lease = journal
        .read({
            let task = task.clone();
            move |read| Ok(read.lease_head(&task)?.expect("held lease exists"))
        })
        .expect("held lease reads");
    let now = Instant::new(jiff::Timestamp::now()).expect("meter clock is canonical");
    let deadline = now
        .get()
        .checked_add(jiff::SignedDuration::from_secs(seconds))
        .expect("meter deadline shifts");
    let until = Instant::new(deadline).expect("meter deadline is canonical");
    let maximum = Instant::new(
        deadline
            .checked_add(jiff::SignedDuration::from_secs(maximum_extension_seconds))
            .expect("meter maximum shifts"),
    )
    .expect("meter maximum is canonical");
    let mut write = JournalWrite::new(
        Actor::Spice,
        "task:SESSION-fixture"
            .parse()
            .expect("fixture claim origin"),
        now,
        Producer::BusTimer,
    );
    let renewed = write
        .fact(
            body::ClaimRenewed {
                task: task.clone(),
                epoch: lease.body().epoch,
                until,
                maximum,
            },
            FactCause::Root,
        )
        .expect("meter renewal builds");
    let mut next = lease.body().clone();
    next.renewal = now;
    next.expiry = until;
    next.maximum = maximum;
    write.lease_head(task, renewed, move |_| next);
    let mut journal = journal;
    journal.apply(write).expect("meter renewal commits");
}

fn move_meter_occupant(path: &Path) {
    let mut journal = Journal::open(path).expect("fixture journal reopens");
    let observed = journal
        .read(|read| Ok(read.residency_head()?.expect("residency exists")))
        .expect("residency reads");
    let mut next = observed.body().clone();
    let row = next
        .lanes
        .iter_mut()
        .find(|row| row.lane == uuid(0xaa))
        .expect("meter lane exists");
    let successor = uuid(0xf1);
    row.occupant = Some(successor);
    row.lineage.push(successor);
    row.session
        .as_mut()
        .expect("meter lane has a session")
        .identity = "successor-thread".to_owned();
    let epoch = next.epoch;
    let mut write = JournalWrite::new(
        Actor::Spice,
        format!("resident:{epoch}")
            .parse()
            .expect("resident origin"),
        Instant::new(jiff::Timestamp::now()).expect("meter clock is canonical"),
        Producer::CommandDoor,
    );
    let seated = write
        .fact(body::UpSeated { epoch }, FactCause::Root)
        .expect("successor seating builds");
    write.residency_head(seated, move |_| next);
    journal.apply(write).expect("successor seating commits");
}

fn free_held_claim(path: &Path) {
    let task = stamp("SESSION-held");
    let mut journal = Journal::open(path).expect("fixture journal reopens");
    let claim = journal
        .read({
            let task = task.clone();
            move |read| Ok(read.claim_head(&task)?.expect("held claim exists"))
        })
        .expect("held claim reads");
    let actor = Actor::Lane(uuid(0xaa));
    let mut write = JournalWrite::new(
        actor.clone(),
        "task:SESSION-fixture"
            .parse()
            .expect("fixture claim origin"),
        Instant::new(jiff::Timestamp::now()).expect("meter clock is canonical"),
        Producer::CommandDoor,
    );
    let freed = write
        .fact(
            body::ClaimFreed {
                task: task.clone(),
                actor,
            },
            FactCause::Root,
        )
        .expect("claim release builds");
    let mut next = claim.body().clone();
    next.disposition = wire::ClaimDisposition::Freed;
    write.claim_head(task, freed, move |fact| wire::ClaimHead { fact, ..next });
    journal.apply(write).expect("claim release commits");
}

fn invoke_meter_child(root: &Path, thread: &str) -> super::Spoken {
    let arguments = [
        "run".to_owned(),
        "--".to_owned(),
        "/bin/sh".to_owned(),
        "-c".to_owned(),
        "printf child-output; exit 7".to_owned(),
    ];
    super::invoke_in(
        Some(root),
        &arguments,
        &[("CODEX_THREAD_ID", thread.to_owned())], // env-policy: allow
    )
}

#[test]
#[cfg(unix)]
fn real_run_stream_reports_working_state_and_wakes_at_warning_boundary() {
    let scratch = Scratch::new("run-meter-boundary");
    fake_repository(scratch.path());
    let thread = uuid(0xf0).to_string();
    let authority = authority_path(scratch.path());
    seed_recall_authority(&authority, &thread);
    renew_maximum_bound_claim_for_meter(&authority, 604);
    let arguments = ["run", "--", "/bin/sh", "-c", "sleep 6; printf child-output"]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();

    let spoken = super::invoke_in(
        Some(scratch.path()),
        &arguments,
        &[("CODEX_THREAD_ID", thread)], // env-policy: allow
    );

    assert_eq!(spoken.code, Some(0));
    assert_eq!(spoken.out, b"child-output");
    let stderr = String::from_utf8(spoken.err).expect("run meter stderr is UTF-8");
    let lines = stderr.lines().collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        2,
        "initial state and one boundary warning speak: {stderr:?}"
    );
    assert!(lines[0].starts_with("🌶️ Working state: 1 pending inbox; claim "));
    assert!(!lines[0].contains("CLAIM LEASE"));
    assert!(lines[1].contains("CLAIM LEASE WARNING"));
    let handle = wire::TaskHandle::for_project(SESSION_PROJECT, &stamp("SESSION-held"));
    assert!(
        lines[1].contains(&format!("run spice task reclaim {handle}")),
        "warning recommends reclaiming the automatic fixture claim: {:?}",
        lines[1]
    );
    assert!(!lines[1].contains("task free"));

    scratch.remove();
}

#[test]
#[cfg(unix)]
fn short_run_processes_share_suppression_and_tier_changes_speak() {
    let scratch = Scratch::new("run-meter-processes");
    fake_repository(scratch.path());
    let thread = uuid(0xf2).to_string();
    let authority = authority_path(scratch.path());
    seed_recall_authority(&authority, &thread);
    renew_maximum_bound_claim_for_meter(&authority, 599);
    let warning_generation_facts = fact_count(&authority);

    let first = invoke_meter_child(scratch.path(), &thread);
    let second = invoke_meter_child(scratch.path(), &thread);
    assert_eq!(fact_count(&authority), warning_generation_facts);
    renew_maximum_bound_claim_for_meter(&authority, 299);
    let urgent_generation_facts = fact_count(&authority);
    let urgent = invoke_meter_child(scratch.path(), &thread);

    for spoken in [&first, &second, &urgent] {
        assert_eq!(spoken.code, Some(7));
        assert_eq!(spoken.out, b"child-output");
    }
    let handle = wire::TaskHandle::for_project(SESSION_PROJECT, &stamp("SESSION-held"));
    let warning = String::from_utf8(first.err).expect("warning stderr");
    assert_eq!(warning.lines().count(), 1);
    assert!(warning.contains("CLAIM LEASE WARNING"));
    assert!(warning.contains("s remaining"));
    assert!(
        warning.contains(&format!("run spice task reclaim {handle}")),
        "warning recommends reclaiming the automatic fixture claim: {warning:?}"
    );
    assert!(!warning.contains("task free"));
    assert!(!warning.contains(" ACK"));
    assert!(!warning.contains(" NACK"));
    assert!(second.err.is_empty(), "same tier is durably suppressed");
    let urgent = String::from_utf8(urgent.err).expect("urgent stderr");
    assert_eq!(urgent.lines().count(), 1);
    assert!(urgent.contains("CLAIM LEASE URGENT"));
    assert!(urgent.contains("s remaining"));
    assert!(
        urgent.contains(&format!("run spice task reclaim {handle}")),
        "urgent warning recommends reclaiming the automatic fixture claim: {urgent:?}"
    );
    assert!(!urgent.contains("task free"));
    assert!(!urgent.contains(" ACK"));
    assert!(!urgent.contains(" NACK"));
    assert_eq!(fact_count(&authority), urgent_generation_facts);

    scratch.remove();
}

#[test]
#[cfg(unix)]
fn ownership_loss_speaks_once_while_terminal_release_stays_silent() {
    let scratch = Scratch::new("run-meter-loss");
    fake_repository(scratch.path());
    let thread = uuid(0xf3).to_string();
    let authority = authority_path(scratch.path());
    seed_recall_authority(&authority, &thread);
    renew_held_claim_for_meter(&authority, 599);
    let _initial = invoke_meter_child(scratch.path(), &thread);
    move_meter_occupant(&authority);

    let lost = invoke_meter_child(scratch.path(), &thread);
    renew_held_claim_for_meter(&authority, 100);
    let deduplicated = invoke_meter_child(scratch.path(), &thread);

    let loss_stderr = String::from_utf8(lost.err).expect("loss stderr");
    assert_eq!(loss_stderr.lines().count(), 1);
    assert!(loss_stderr.contains("CLAIM LOST:"));
    assert!(loss_stderr.contains("run spice task status"));
    assert!(
        deduplicated.err.is_empty(),
        "successor lease heads do not repeat one ownership loss"
    );

    let terminal = Scratch::new("run-meter-terminal");
    fake_repository(terminal.path());
    let terminal_thread = uuid(0xf4).to_string();
    let terminal_authority = authority_path(terminal.path());
    seed_recall_authority(&terminal_authority, &terminal_thread);
    renew_held_claim_for_meter(&terminal_authority, 599);
    let _initial = invoke_meter_child(terminal.path(), &terminal_thread);
    free_held_claim(&terminal_authority);
    let freed = invoke_meter_child(terminal.path(), &terminal_thread);
    assert!(!String::from_utf8_lossy(&freed.err).contains("CLAIM LOST:"));

    terminal.remove();
    scratch.remove();
}

#[test]
#[cfg(unix)]
fn meter_persistence_failure_is_silent_and_preserves_child_outcome() {
    let scratch = Scratch::new("run-meter-persistence-failure");
    fake_repository(scratch.path());
    let thread = uuid(0xf5).to_string();
    let authority = authority_path(scratch.path());
    seed_recall_authority(&authority, &thread);
    renew_held_claim_for_meter(&authority, 599);
    fs::create_dir(
        scratch
            .path()
            .join(".git/.spice-rs/session-meter-owner.json"),
    )
    .expect("owner record is made unwritable");

    let spoken = invoke_meter_child(scratch.path(), &thread);

    assert_eq!(spoken.code, Some(7));
    assert_eq!(spoken.out, b"child-output");
    assert!(spoken.err.is_empty());
    scratch.remove();
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
    phase: &str,
    at: Instant,
    expiry: Instant,
    maximum: Instant,
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
            workflow: &workflow(phase),
            project,
            lane,
            occupant,
            epoch,
            at,
            expiry,
            maximum,
            git: None,
            recovery: None,
            source: body::SourceBoundary::Proven { heads: Vec::new() },
        },
    )
    .expect("claim mints");
    journal.apply(write).expect("claim commits");
}

fn claim_won_facts(path: &Path) -> i64 {
    Connection::open(path)
        .expect("fixture authority opens for claim fact count")
        .query_row(
            "SELECT COUNT(*) FROM fact_envelopes WHERE kind = 'claim::won'",
            [],
            |row| row.get(0),
        )
        .expect("claim fact count reads")
}

fn fact_count(path: &Path) -> i64 {
    Connection::open(path)
        .expect("fixture authority opens for total fact count")
        .query_row("SELECT COUNT(*) FROM fact_envelopes", [], |row| row.get(0))
        .expect("total fact count reads")
}

fn active_claims_for(path: &Path, actor: &Actor) -> i64 {
    Connection::open(path)
        .expect("fixture authority opens for active claim count")
        .query_row(
            "SELECT COUNT(*) FROM claim_index WHERE actor = ?1 AND disposition = 'active'",
            params![actor.to_string()],
            |row| row.get(0),
        )
        .expect("active claim count reads")
}

#[test]
fn ambient_and_explicit_matching_threads_bind_authority_to_the_live_identity_read_only() {
    let scratch = Scratch::new("authority-live-match");
    fake_repository(scratch.path());
    let live = uuid(0xaa).to_string().replace('-', "");
    let authority = authority_path(scratch.path());
    seed_recall_authority(&authority, &live);
    let codex_home = seed_codex_transcripts(scratch.path(), std::slice::from_ref(&live));
    let environment = [
        ("CODEX_THREAD_ID", live.clone()), // env-policy: allow
        ("CODEX_HOME", codex_home.to_string_lossy().into_owned()),
    ];
    let before = fact_count(&authority);

    let ambient = reply_forms_in(
        Some(scratch.path()),
        &strings(&["session", "briefing", "--deadline-seconds", "5"]),
        &environment,
        "recall",
    );
    let explicit = reply_forms_in(
        Some(scratch.path()),
        &strings(&["session", "briefing", &live, "--deadline-seconds", "5"]),
        &environment,
        "recall",
    );

    for recall in [&ambient, &explicit] {
        assert_eq!(recall["sources"][0]["thread"], live);
        assert_eq!(recall["authorities"]["held_work"]["thread"], live);
        assert_eq!(recall["authorities"]["asks"][0]["key"], "1kAsk001");
        assert_eq!(
            recall["authorities"]["held_work"]["own_claim"]["title"],
            "Compose Rust briefings"
        );
        assert!(
            recall["authorities"].get("unknowns").is_none(),
            "matching live and evidence identities are fully known"
        );
    }
    assert_eq!(fact_count(&authority), before, "briefings append no facts");

    scratch.remove();
}

#[test]
fn preferred_live_vendor_identity_wins_over_another_populated_vendor() {
    let scratch = Scratch::new("authority-preferred-live-vendor");
    fake_repository(scratch.path());
    let live = uuid(0xaa).to_string().replace('-', "");
    let stale = uuid(0xbb).to_string().replace('-', "");
    let authority = authority_path(scratch.path());
    seed_recall_authority(&authority, &live);
    let codex_home = seed_codex_transcripts(scratch.path(), std::slice::from_ref(&live));
    let environment = [
        ("CODEX_THREAD_ID", stale),                  // env-policy: allow
        ("CLAUDE_CODE_SESSION_ID", live.clone()),    // env-policy: allow
        ("SPICE_AGENT_VENDOR", "claude".to_owned()), // env-policy: allow
        ("CODEX_HOME", codex_home.to_string_lossy().into_owned()),
    ];
    let before = fact_count(&authority);

    let recall = reply_forms_in(
        Some(scratch.path()),
        &strings(&["session", "briefing", &live, "--deadline-seconds", "5"]),
        &environment,
        "recall",
    );

    assert_eq!(recall["sources"][0]["thread"], live);
    assert_eq!(recall["authorities"]["held_work"]["thread"], live);
    assert_eq!(recall["authorities"]["asks"][0]["key"], "1kAsk001");
    assert_eq!(
        recall["authorities"]["held_work"]["own_claim"]["title"],
        "Compose Rust briefings"
    );
    assert_eq!(fact_count(&authority), before, "briefing appends no facts");

    scratch.remove();
}

#[test]
fn multiple_live_vendor_identities_without_preference_leave_authority_unknown() {
    let scratch = Scratch::new("authority-ambiguous-live-vendors");
    fake_repository(scratch.path());
    let selected = uuid(0xaa).to_string().replace('-', "");
    let other = uuid(0xbb).to_string().replace('-', "");
    let authority = authority_path(scratch.path());
    seed_recall_authority(&authority, &selected);
    let codex_home = seed_codex_transcripts(scratch.path(), std::slice::from_ref(&selected));
    let environment = [
        ("CODEX_THREAD_ID", other),                   // env-policy: allow
        ("CLAUDE_CODE_SESSION_ID", selected.clone()), // env-policy: allow
        ("CODEX_HOME", codex_home.to_string_lossy().into_owned()),
    ];
    let before = fact_count(&authority);

    let recall = reply_forms_in(
        Some(scratch.path()),
        &strings(&["session", "briefing", &selected, "--deadline-seconds", "5"]),
        &environment,
        "recall",
    );

    assert_eq!(recall["sources"][0]["thread"], selected);
    assert_eq!(recall["authorities"]["held_work"]["thread"], Value::Null);
    assert_eq!(recall["authorities"]["held_work"]["lane"], Value::Null);
    assert_eq!(recall["authorities"]["held_work"]["own_claim"], Value::Null);
    assert!(recall["authorities"].get("asks").is_none());
    let unknown = recall["authorities"]["unknowns"]
        .as_array()
        .expect("ambiguous live environment is explicit")
        .iter()
        .find(|row| row["probe"] == "live-thread-identity")
        .expect("live identity ambiguity is reported");
    assert!(
        unknown["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("multiple live vendor identities"))
    );
    assert_eq!(fact_count(&authority), before, "briefing appends no facts");

    scratch.remove();
}

#[test]
fn preferred_vendor_without_identity_does_not_fall_back_to_another_vendor() {
    let scratch = Scratch::new("authority-missing-preferred-live-vendor");
    fake_repository(scratch.path());
    let selected = uuid(0xaa).to_string().replace('-', "");
    let authority = authority_path(scratch.path());
    seed_recall_authority(&authority, &selected);
    let codex_home = seed_codex_transcripts(scratch.path(), std::slice::from_ref(&selected));
    let environment = [
        ("CODEX_THREAD_ID", selected.clone()), // env-policy: allow
        ("SPICE_AGENT_VENDOR", "claude".to_owned()), // env-policy: allow
        ("CODEX_HOME", codex_home.to_string_lossy().into_owned()),
    ];

    let recall = reply_forms_in(
        Some(scratch.path()),
        &strings(&["session", "briefing", &selected, "--deadline-seconds", "5"]),
        &environment,
        "recall",
    );

    assert_eq!(recall["sources"][0]["thread"], selected);
    assert_eq!(recall["authorities"]["held_work"]["thread"], Value::Null);
    assert!(recall["authorities"].get("asks").is_none());
    assert!(
        recall["authorities"]["unknowns"]
            .as_array()
            .expect("missing preferred identity is explicit")
            .iter()
            .any(|row| {
                row["probe"] == "live-thread-identity"
                    && row["detail"]
                        .as_str()
                        .is_some_and(|detail| detail.contains("CLAUDE_CODE_SESSION_ID is absent"))
            })
    );

    scratch.remove();
}

#[test]
fn explicit_foreign_transcript_preserves_evidence_without_impersonating_its_actor() {
    let scratch = Scratch::new("authority-foreign-transcript");
    fake_repository(scratch.path());
    let live = uuid(0xaa).to_string().replace('-', "");
    let foreign = uuid(0xbb).to_string().replace('-', "");
    let authority = authority_path(scratch.path());
    seed_recall_authority(&authority, &live);
    let codex_home = seed_codex_transcripts(scratch.path(), std::slice::from_ref(&foreign));
    let environment = [
        ("CODEX_THREAD_ID", live.clone()), // env-policy: allow
        ("CODEX_HOME", codex_home.to_string_lossy().into_owned()),
    ];
    let before = fact_count(&authority);

    let recall = reply_forms_in(
        Some(scratch.path()),
        &strings(&["session", "briefing", &foreign, "--deadline-seconds", "5"]),
        &environment,
        "recall",
    );

    assert_eq!(recall["sources"][0]["thread"], foreign);
    assert!(
        recall["window"]["turns"]
            .as_array()
            .is_some_and(|turns| !turns.is_empty()),
        "foreign transcript evidence remains in the bounded window"
    );
    assert_eq!(recall["authorities"]["held_work"]["thread"], live);
    assert_eq!(recall["authorities"]["asks"][0]["key"], "1kAsk001");
    assert_eq!(
        recall["authorities"]["held_work"]["own_claim"]["title"],
        "Compose Rust briefings"
    );
    let mismatch = recall["authorities"]["unknowns"]
        .as_array()
        .expect("foreign evidence mismatch is explicit")
        .iter()
        .find(|row| row["probe"] == "live-thread-mismatch")
        .expect("foreign evidence mismatch is reported");
    let detail = mismatch["detail"].as_str().expect("mismatch has detail");
    assert!(detail.contains(&foreign), "{detail}");
    assert!(detail.contains(&live), "{detail}");
    assert_eq!(fact_count(&authority), before, "briefing appends no facts");

    scratch.remove();
}

#[test]
fn ambiguous_multi_thread_evidence_does_not_make_current_authority_ambiguous() {
    let scratch = Scratch::new("authority-multi-transcript");
    fake_repository(scratch.path());
    let live = uuid(0xaa).to_string().replace('-', "");
    let foreign = uuid(0xbb).to_string().replace('-', "");
    let authority = authority_path(scratch.path());
    seed_recall_authority(&authority, &live);
    let codex_home = seed_codex_transcripts(scratch.path(), &[live.clone(), foreign.clone()]);
    let environment = [
        ("CODEX_THREAD_ID", live.clone()), // env-policy: allow
        ("CODEX_HOME", codex_home.to_string_lossy().into_owned()),
    ];
    let before = fact_count(&authority);

    let recall = reply_forms_in(
        Some(scratch.path()),
        &strings(&[
            "session",
            "briefing",
            &live,
            &foreign,
            "--deadline-seconds",
            "5",
        ]),
        &environment,
        "recall",
    );

    assert_eq!(recall["sources"].as_array().map(Vec::len), Some(2));
    assert_eq!(recall["authorities"]["held_work"]["thread"], live);
    assert_eq!(recall["authorities"]["asks"][0]["key"], "1kAsk001");
    let unknowns = recall["authorities"]["unknowns"]
        .as_array()
        .expect("multi-thread evidence ambiguity is explicit");
    assert!(
        unknowns
            .iter()
            .any(|row| row["probe"] == "selected-thread-identity")
    );
    assert!(
        unknowns
            .iter()
            .any(|row| row["probe"] == "live-thread-mismatch")
    );
    assert_eq!(fact_count(&authority), before, "briefing appends no facts");

    scratch.remove();
}

#[test]
fn malformed_live_identity_stays_unknown_instead_of_using_selected_transcript_identity() {
    let scratch = Scratch::new("authority-malformed-live");
    fake_repository(scratch.path());
    let selected = uuid(0xaa).to_string().replace('-', "");
    let authority = authority_path(scratch.path());
    seed_recall_authority(&authority, &selected);
    let codex_home = seed_codex_transcripts(scratch.path(), std::slice::from_ref(&selected));
    let environment = [
        ("CODEX_THREAD_ID", "malformed-live-thread".to_owned()), // env-policy: allow
        ("CODEX_HOME", codex_home.to_string_lossy().into_owned()),
    ];
    let before = fact_count(&authority);

    let recall = reply_forms_in(
        Some(scratch.path()),
        &strings(&["session", "briefing", &selected, "--deadline-seconds", "5"]),
        &environment,
        "recall",
    );

    assert_eq!(recall["sources"][0]["thread"], selected);
    assert_eq!(recall["authorities"]["held_work"]["thread"], Value::Null);
    assert_eq!(recall["authorities"]["held_work"]["lane"], Value::Null);
    assert_eq!(recall["authorities"]["held_work"]["own_claim"], Value::Null);
    assert_eq!(
        recall["authorities"]["held_work"]["peer_claims"],
        Value::Null
    );
    assert!(recall["authorities"].get("asks").is_none());
    assert!(
        recall["authorities"]["unknowns"]
            .as_array()
            .expect("malformed live identity is explicit")
            .iter()
            .any(|row| {
                row["authority"] == "environment" && row["probe"] == "live-thread-identity"
            })
    );
    assert_eq!(fact_count(&authority), before, "briefing appends no facts");

    scratch.remove();
}

#[test]
fn absent_live_identity_keeps_current_authority_unknown_during_historical_recovery() {
    let scratch = Scratch::new("authority-no-live");
    fake_repository(scratch.path());
    let selected = uuid(0xaa).to_string().replace('-', "");
    let authority = authority_path(scratch.path());
    seed_recall_authority(&authority, &selected);
    let codex_home = seed_codex_transcripts(scratch.path(), std::slice::from_ref(&selected));
    let environment = [("CODEX_HOME", codex_home.to_string_lossy().into_owned())];
    let before = fact_count(&authority);

    let recall = reply_forms_in(
        Some(scratch.path()),
        &strings(&["session", "briefing", &selected, "--deadline-seconds", "5"]),
        &environment,
        "recall",
    );

    assert_eq!(recall["sources"][0]["thread"], selected);
    assert!(
        recall["window"]["turns"]
            .as_array()
            .is_some_and(|turns| !turns.is_empty()),
        "historical transcript evidence remains in the bounded window"
    );
    assert_eq!(recall["authorities"]["held_work"]["thread"], Value::Null);
    assert_eq!(recall["authorities"]["held_work"]["lane"], Value::Null);
    assert_eq!(recall["authorities"]["held_work"]["own_claim"], Value::Null);
    assert_eq!(
        recall["authorities"]["held_work"]["peer_claims"],
        Value::Null
    );
    assert!(recall["authorities"].get("asks").is_none());
    let unknowns = recall["authorities"]["unknowns"]
        .as_array()
        .expect("missing live identity is explicit");
    assert!(unknowns.iter().any(|row| {
        row["authority"] == "environment" && row["probe"] == "live-thread-identity"
    }));
    assert!(
        unknowns
            .iter()
            .any(|row| { row["authority"] == "board" && row["probe"] == "exact-actor-claim" })
    );
    assert_eq!(fact_count(&authority), before, "briefing appends no facts");

    scratch.remove();
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
            .any(|row| {
                row["authority"] == "environment" && row["probe"] == "live-thread-identity"
            }),
        "missing live identity is reported before actor-bound journal probes"
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
fn creator_and_claim_briefings_do_not_append_claim_facts_or_duplicate_the_actor_claim() {
    let scratch = Scratch::new("authority-briefing-read-only");
    fake_repository(scratch.path());
    let creator_transcript = scratch.path().join("creator.jsonl");
    let claim_transcript = scratch.path().join("claim.jsonl");
    write_jsonl(&creator_transcript, &codex_rows());
    write_jsonl(&claim_transcript, &codex_rows());
    let thread = uuid(0xaa).to_string().replace('-', "");
    let authority = authority_path(scratch.path());
    seed_recall_authority(&authority, &thread);
    let actor = Actor::Lane(uuid(0xaa));
    let environment = [("CODEX_THREAD_ID", thread.clone())]; // env-policy: allow

    let before_claim_facts = claim_won_facts(&authority);
    assert_eq!(active_claims_for(&authority, &actor), 1);

    let creator = reply_forms_in(
        Some(scratch.path()),
        &strings(&[
            "session",
            "briefing",
            &creator_transcript.to_string_lossy(),
            "--deadline-seconds",
            "5",
        ]),
        &environment,
        "recall",
    );
    assert_eq!(
        creator["authorities"]["held_work"]["own_claim"]["title"],
        "Compose Rust briefings"
    );
    assert_eq!(
        claim_won_facts(&authority),
        before_claim_facts,
        "creator briefing appended a claim fact"
    );

    let claim = reply_forms_in(
        Some(scratch.path()),
        &strings(&[
            "session",
            "briefing",
            &claim_transcript.to_string_lossy(),
            "--deadline-seconds",
            "5",
        ]),
        &environment,
        "recall",
    );
    assert_eq!(
        claim["authorities"]["held_work"]["own_claim"]["title"],
        "Compose Rust briefings"
    );
    assert_eq!(
        claim_won_facts(&authority),
        before_claim_facts,
        "claim briefing appended a claim fact"
    );
    assert_eq!(
        active_claims_for(&authority, &actor),
        1,
        "the actor owns exactly one active claim after both briefings"
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
