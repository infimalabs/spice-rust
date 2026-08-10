//! Rust-resident automatic lifecycle effects with no Python process.

use super::lane_continuity::{add_lane, durable_head};
use super::lane_process::{configure_fake_vendor, initialize_task_git_fixture};
use super::*;

const SESSION_RECORD: &str =
    r#"{"type":"system","subtype":"init","session_id":"018f7777-7777-7444-8888-999999999999"}"#;

fn configure_task_fixture(fixture: &Fixture) {
    configure_fake_vendor(fixture);
    let vendor = fixture.root.join("fake-claude");
    let script = fs::read_to_string(&vendor).expect("automatic lifecycle vendor reads");
    let switch = format!(
        concat!(
            "#!/bin/sh\n",
            "if [ -f {} ]; then exit 17; fi\n",
            "if [ -f {} ]; then printf '%s\\n' {}; printf '%s\\n' \"$$\" >> {}; IFS= read -r _ < {}; exit 17; fi\n",
        ),
        shell_word(&startup_failure_marker(fixture).display().to_string()),
        shell_word(&rapid_exit_marker(fixture).display().to_string()),
        shell_word(SESSION_RECORD),
        shell_word(&rapid_exit_pids(fixture).display().to_string()),
        shell_word(&rapid_exit_gate(fixture).display().to_string()),
    );
    fs::write(&vendor, script.replacen("#!/bin/sh\n", &switch, 1))
        .expect("automatic lifecycle vendor switches write");
    let mut config = fs::read_to_string(fixture.root.join("spice.toml"))
        .expect("automatic lifecycle configuration reads");
    config.push_str("\n[tasks]\nstems = [\"spice\"]\n");
    fs::write(fixture.root.join("spice.toml"), config)
        .expect("automatic lifecycle task policy writes");
    initialize_task_git_fixture(fixture);
}

fn startup_failure_marker(fixture: &Fixture) -> PathBuf {
    fixture.root.join(".git/automatic-startup-failure")
}

fn rapid_exit_marker(fixture: &Fixture) -> PathBuf {
    fixture.root.join(".git/automatic-rapid-exit")
}

fn rapid_exit_pids(fixture: &Fixture) -> PathBuf {
    fixture.cleanup.join("automatic-rapid-exit-pids")
}

fn rapid_exit_gate(fixture: &Fixture) -> PathBuf {
    fixture.cleanup.join("automatic-rapid-exit-gate")
}

fn configure_lineage(
    fixture: &Fixture,
    lineage: wire::CanonicalUuid,
    life: wire::intent::LineageLife,
) {
    let discovery = fixture.discovery();
    let topology_revision = durable_head(fixture).topology_revision;
    let (mut socket, accepted) = authenticate(&discovery, discovery.capability);
    assert_eq!(accepted.actor, Actor::Operator);
    let id = match life {
        wire::intent::LineageLife::Steer => uuid("00000000-0000-4000-8000-000000000100"),
        wire::intent::LineageLife::Drive => uuid("00000000-0000-4000-8000-000000000101"),
        wire::intent::LineageLife::Drain => uuid("00000000-0000-4000-8000-000000000102"),
    };
    let command = wire::Command {
        v: wire::WireVersion::CURRENT,
        id,
        epoch: discovery.epoch,
        kind: wire::CommandKind::LineageConfigure,
        expected: Vec::new(),
        body: wire::intent::LineageConfigure {
            topology_revision,
            lineage_id: lineage,
            member: None,
            life,
            manual_projects: None,
        },
    };
    socket
        .send(Message::Binary(
            wire::encode(&command)
                .expect("lineage configuration encodes")
                .into(),
        ))
        .expect("lineage configuration sends");
    let frame = socket
        .read()
        .expect("lineage configuration response arrives")
        .into_data();
    let response: wire::Response<serde_json::Value> =
        wire::decode(&frame).expect("lineage configuration response decodes");
    assert_eq!((response.id, response.epoch), (id, discovery.epoch));
    assert!(
        matches!(response.outcome, Outcome::Reply(_)),
        "lineage configuration failed: {:?}",
        response.outcome,
    );
    assert_eq!(
        durable_head(fixture).topology_revision,
        topology_revision + 1,
        "lineage configuration advances the shared topology authority",
    );
}

fn invoke_lane(fixture: &Fixture, lane: wire::CanonicalUuid, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spice"))
        .arg("--json")
        .args(arguments)
        .env(steer::hook::LANE, lane.to_string())
        .current_dir(&fixture.root)
        .output()
        .expect("lane command launches")
}

fn add_task(
    fixture: &Fixture,
    lane: Option<wire::CanonicalUuid>,
    title: &str,
) -> wire::intent::TaskAnswer {
    let arguments = [
        "task",
        "add",
        title,
        "--project",
        "spice.lifecycle",
        "--acceptance",
        "automatic lifecycle fixture reaches its asserted terminal state",
        "--origin",
        "fixture:automatic-lifecycle",
    ];
    let output = match lane {
        Some(lane) => invoke_lane(fixture, lane, &arguments),
        None => invoke_args(fixture, &arguments, true),
    };
    assert!(
        output.status.success(),
        "task add failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let Outcome::Reply(reply) = value_outcome(&output) else {
        panic!("successful task add must reply");
    };
    wire::decode(&wire::encode(&reply.body).expect("task answer re-encodes"))
        .expect("task answer decodes")
}

fn wait_for_successor(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    ancestor: wire::CanonicalUuid,
) -> wire::CanonicalUuid {
    wait_for_event(
        &fixture.runtime.join("data"),
        || {},
        || {
            durable_head(fixture)
                .lanes
                .into_iter()
                .find(|row| row.lane == lane)
                .and_then(|row| row.occupant)
                .filter(|occupant| *occupant != ancestor)
        },
    )
}

fn drain(fixture: &Fixture, lane: wire::CanonicalUuid) {
    let output = invoke_args(fixture, &["lane", "drain", &lane.to_string()], true);
    assert!(
        output.status.success(),
        "lane drain failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn finish_resident(fixture: &Fixture, resident: RunningResident) {
    let down = invoke(fixture, "down");
    assert!(
        down.status.success(),
        "resident shutdown command failed: stdout={} stderr={}",
        String::from_utf8_lossy(&down.stdout),
        String::from_utf8_lossy(&down.stderr),
    );
    assert!(resident.wait().status.success(), "resident failed");
}

fn end_rapid_successor(
    fixture: &Fixture,
    resident: &mut RunningResident,
    lane: wire::CanonicalUuid,
    ancestor: wire::CanonicalUuid,
    launch_count: usize,
) -> wire::CanonicalUuid {
    let pids = rapid_exit_pids(fixture);
    let observe = || {
        let successor = durable_head(fixture)
            .lanes
            .into_iter()
            .find(|row| row.lane == lane)
            .and_then(|row| row.occupant)
            .filter(|occupant| *occupant != ancestor)?;
        let pid = fs::read_to_string(&pids)
            .ok()?
            .lines()
            .nth(launch_count - 1)?
            .parse::<u32>()
            .ok()?;
        Some((successor, pid))
    };
    match readiness::wait_until_ready(
        resident
            .child
            .as_mut()
            .expect("resident child remains live"),
        &fixture.runtime.join("data"),
        LANE_EFFECT_HORIZON,
        "rapid automatic successor",
        || observe().is_some(),
    ) {
        Readiness::Ready => {}
        Readiness::Exited => {
            let output = resident
                .child
                .take()
                .expect("resident child")
                .wait_with_output()
                .expect("failed resident output collects");
            panic!(
                "resident exited before rapid successor: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
    }
    let (successor, pid) = observe().expect("ready rapid successor remains observable");
    let process_exit = readiness::process_exit::ProcessExitObserver::new(pid);
    let (exited, exit_event) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        process_exit.wait();
        let _sent = exited.send(());
    });
    let mut gate = fs::OpenOptions::new()
        .write(true)
        .open(rapid_exit_gate(fixture))
        .expect("rapid occupant exit gate opens");
    gate.write_all(format!("exit {pid}\n").as_bytes())
        .expect("rapid occupant exit gate writes");
    exit_event
        .recv_timeout(CLIENT_DEADLINE_BOUND)
        .expect("rapid occupant reaches its real kernel exit");
    successor
}

#[test]
fn rust_resident_pending_inbox_wakes_a_vacant_lane_without_python() {
    let fixture = Fixture::new("automatic-pending-inbox");
    configure_fake_vendor(&fixture);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let seated = add_lane(&fixture, &["--life", "Drive", "--vendor", "claude"]);
    let first = durable_head(&fixture)
        .lanes
        .into_iter()
        .find(|row| row.lane == seated.lane)
        .and_then(|row| row.occupant)
        .expect("new lane has its first occupant");
    let sent = invoke_args(
        &fixture,
        &["steer", &seated.lane.to_string(), "wake me after drain"],
        true,
    );
    assert!(
        sent.status.success(),
        "steering send failed: stdout={} stderr={}",
        String::from_utf8_lossy(&sent.stdout),
        String::from_utf8_lossy(&sent.stderr),
    );
    let Outcome::Reply(sent_reply) = value_outcome(&sent) else {
        panic!("successful steering send must reply");
    };
    let sent_answer: wire::intent::DirectiveAnswer =
        wire::decode(&wire::encode(&sent_reply.body).expect("steering answer re-encodes"))
            .expect("steering answer decodes");

    drain(&fixture, seated.lane);
    let successor = wait_for_successor(&fixture, seated.lane, first);
    assert_ne!(successor, first);
    let snapshot = journal::JournalReader::open(fixture.journal())
        .expect("automatic lifecycle journal reads")
        .fact_snapshot()
        .expect("automatic lifecycle facts read");
    assert!(
        !snapshot
            .facts()
            .iter()
            .any(|fact| fact.fact.kind() == FactKind::SteerDeadLettered),
        "a successful Rust wake must not park its pending directive",
    );

    let canceled = invoke_args(
        &fixture,
        &["steer", "cancel", &sent_answer.directive.to_string()],
        true,
    );
    assert!(
        canceled.status.success(),
        "fixture directive cancel failed: stdout={} stderr={}",
        String::from_utf8_lossy(&canceled.stdout),
        String::from_utf8_lossy(&canceled.stderr),
    );
    drain(&fixture, seated.lane);
    finish_resident(&fixture, resident);
}

#[test]
fn pending_start_failure_deadletters_and_exact_requeue_recovers_the_same_identity() {
    let fixture = Fixture::new("automatic-pending-dead-letter");
    configure_task_fixture(&fixture);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let seated = add_lane(&fixture, &["--life", "Drive", "--vendor", "claude"]);
    let first = durable_head(&fixture)
        .lanes
        .into_iter()
        .find(|row| row.lane == seated.lane)
        .and_then(|row| row.occupant)
        .expect("new lane has its first occupant");
    fs::write(startup_failure_marker(&fixture), b"fail\n")
        .expect("startup failure marker writes outside the worktree");
    let sent = invoke_args(
        &fixture,
        &[
            "steer",
            &seated.lane.to_string(),
            "park and recover this exact directive",
        ],
        true,
    );
    assert!(
        sent.status.success(),
        "steering send failed: stdout={} stderr={}",
        String::from_utf8_lossy(&sent.stdout),
        String::from_utf8_lossy(&sent.stderr),
    );
    let Outcome::Reply(sent_reply) = value_outcome(&sent) else {
        panic!("successful steering send must reply");
    };
    let sent_answer: wire::intent::DirectiveAnswer =
        wire::decode(&wire::encode(&sent_reply.body).expect("steering answer re-encodes"))
            .expect("steering answer decodes");

    drain(&fixture, seated.lane);
    wait_for_event(
        &fixture.runtime.join("data"),
        || {},
        || {
            let snapshot = journal::JournalReader::open(fixture.journal())
                .ok()?
                .fact_snapshot()
                .ok()?;
            snapshot
                .facts()
                .iter()
                .any(|fact| {
                    fact.fact.kind() == FactKind::SteerDeadLettered
                        && fact.fact.body()["directive"] == serde_json::json!(sent_answer.directive)
                })
                .then_some(())
        },
    );
    fs::remove_file(startup_failure_marker(&fixture)).expect("startup failure marker is repaired");
    let recovered = invoke_args(
        &fixture,
        &["steer", "recover", &sent_answer.directive.to_string()],
        true,
    );
    assert!(
        recovered.status.success(),
        "exact requeue failed: stdout={} stderr={}",
        String::from_utf8_lossy(&recovered.stdout),
        String::from_utf8_lossy(&recovered.stderr),
    );
    let Outcome::Reply(recovered_reply) = value_outcome(&recovered) else {
        panic!("successful steering recovery must reply");
    };
    let recovered_answer: wire::intent::DirectiveAnswer =
        wire::decode(&wire::encode(&recovered_reply.body).expect("recovery answer re-encodes"))
            .expect("recovery answer decodes");
    assert_eq!(
        (
            recovered_answer.directive,
            recovered_answer.key.as_str(),
            recovered_answer.disposition,
        ),
        (
            sent_answer.directive,
            sent_answer.key.as_str(),
            wire::Disposition::Live,
        ),
        "the printed requeue restores the same durable directive and key",
    );
    let successor = wait_for_successor(&fixture, seated.lane, first);
    assert_ne!(successor, first);

    let canceled = invoke_args(
        &fixture,
        &["steer", "cancel", &sent_answer.directive.to_string()],
        true,
    );
    assert!(
        canceled.status.success(),
        "recovered directive cancel failed: stdout={} stderr={}",
        String::from_utf8_lossy(&canceled.stdout),
        String::from_utf8_lossy(&canceled.stderr),
    );
    drain(&fixture, seated.lane);
    finish_resident(&fixture, resident);
}

#[test]
fn rust_resident_carries_a_held_claim_to_the_successor_without_python() {
    let fixture = Fixture::new("automatic-held-claim");
    configure_task_fixture(&fixture);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let seated = add_lane(&fixture, &["--life", "Drive", "--vendor", "claude"]);
    let first = durable_head(&fixture)
        .lanes
        .into_iter()
        .find(|row| row.lane == seated.lane)
        .and_then(|row| row.occupant)
        .expect("new lane has its first occupant");
    let task = add_task(
        &fixture,
        Some(seated.lane),
        "Carry held work into the automatic successor",
    );
    let claimed = invoke_lane(&fixture, seated.lane, &["task", "next"]);
    assert!(
        claimed.status.success(),
        "task claim failed: stdout={} stderr={}",
        String::from_utf8_lossy(&claimed.stdout),
        String::from_utf8_lossy(&claimed.stderr),
    );

    drain(&fixture, seated.lane);
    let successor = wait_for_successor(&fixture, seated.lane, first);
    let snapshot = journal::JournalReader::open(fixture.journal())
        .expect("held-claim lifecycle journal reads")
        .fact_snapshot()
        .expect("held-claim lifecycle facts read");
    let carried = snapshot
        .facts()
        .iter()
        .filter(|fact| fact.fact.kind() == FactKind::ClaimCarried)
        .collect::<Vec<_>>();
    assert_eq!(carried.len(), 1, "one held claim is carried exactly once");
    let carried: wire::body::ClaimCarried =
        wire::decode(&wire::encode(carried[0].fact.body()).expect("claim carry re-encodes"))
            .expect("claim carry decodes");
    assert_eq!(
        (
            carried.task,
            carried.lane,
            carried.occupant,
            carried.ancestor,
            carried.epoch,
        ),
        (
            task.task,
            seated.lane,
            successor,
            first,
            fixture.discovery().epoch,
        ),
        "the claim carry names the exact predecessor and successor",
    );
    drop(snapshot);

    let done = invoke_lane(
        &fixture,
        seated.lane,
        &[
            "task",
            "done",
            "--validation",
            "held-claim lifecycle fixture",
        ],
    );
    assert!(
        done.status.success(),
        "task completion failed: stdout={} stderr={}",
        String::from_utf8_lossy(&done.stdout),
        String::from_utf8_lossy(&done.stderr),
    );
    drain(&fixture, seated.lane);
    finish_resident(&fixture, resident);
}

#[test]
fn rust_resident_claims_one_of_two_settled_ready_tasks_without_python() {
    let fixture = Fixture::new("automatic-available-work");
    configure_task_fixture(&fixture);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let seated = add_lane(&fixture, &["--life", "Drive", "--vendor", "claude"]);
    configure_lineage(&fixture, seated.lane, wire::intent::LineageLife::Drain);
    let first = durable_head(&fixture)
        .lanes
        .into_iter()
        .find(|row| row.lane == seated.lane)
        .and_then(|row| row.occupant)
        .expect("new lane has its first occupant");
    let first_task = add_task(&fixture, None, "First settled automatic task");
    let second_task = add_task(&fixture, None, "Second settled automatic task");

    drain(&fixture, seated.lane);
    let successor = wait_for_successor(&fixture, seated.lane, first);
    let snapshot = journal::JournalReader::open(fixture.journal())
        .expect("available-work lifecycle journal reads")
        .fact_snapshot()
        .expect("available-work lifecycle facts read");
    let won = snapshot
        .facts()
        .iter()
        .filter(|fact| fact.fact.kind() == FactKind::ClaimWon)
        .collect::<Vec<_>>();
    assert_eq!(won.len(), 1, "one settled task is claimed exactly once");
    assert_eq!(won[0].fact.body()["lane"], serde_json::json!(seated.lane));
    assert_eq!(won[0].fact.body()["occupant"], serde_json::json!(successor));
    assert!(
        [first_task.task, second_task.task]
            .iter()
            .any(|task| won[0].fact.body()["task"] == serde_json::json!(task)),
        "the automatic claim selects one of the two READY tasks",
    );
    drop(snapshot);

    configure_lineage(&fixture, seated.lane, wire::intent::LineageLife::Drive);
    let done = invoke_lane(
        &fixture,
        seated.lane,
        &[
            "task",
            "done",
            "--validation",
            "available-work lifecycle fixture",
        ],
    );
    assert!(
        done.status.success(),
        "task completion failed: stdout={} stderr={}",
        String::from_utf8_lossy(&done.stdout),
        String::from_utf8_lossy(&done.stderr),
    );
    drain(&fixture, seated.lane);
    finish_resident(&fixture, resident);
}

#[test]
fn available_work_startup_failure_frees_only_its_automatic_claim() {
    let fixture = Fixture::new("automatic-claim-cleanup");
    configure_task_fixture(&fixture);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let seated = add_lane(&fixture, &["--life", "Drive", "--vendor", "claude"]);
    configure_lineage(&fixture, seated.lane, wire::intent::LineageLife::Drain);
    let first_task = add_task(&fixture, None, "First failed automatic task");
    let second_task = add_task(&fixture, None, "Second failed automatic task");
    fs::write(startup_failure_marker(&fixture), b"fail\n")
        .expect("startup failure marker writes outside the worktree");

    drain(&fixture, seated.lane);
    wait_for_event(
        &fixture.runtime.join("data"),
        || {},
        || {
            let snapshot = journal::JournalReader::open(fixture.journal())
                .ok()?
                .fact_snapshot()
                .ok()?;
            snapshot
                .facts()
                .iter()
                .any(|fact| fact.fact.kind() == FactKind::ClaimFreed)
                .then_some(())
        },
    );
    configure_lineage(&fixture, seated.lane, wire::intent::LineageLife::Drive);

    let snapshot = journal::JournalReader::open(fixture.journal())
        .expect("claim-cleanup journal reads")
        .fact_snapshot()
        .expect("claim-cleanup facts read");
    let won = snapshot
        .facts()
        .iter()
        .filter(|fact| fact.fact.kind() == FactKind::ClaimWon)
        .collect::<Vec<_>>();
    let freed = snapshot
        .facts()
        .iter()
        .filter(|fact| fact.fact.kind() == FactKind::ClaimFreed)
        .collect::<Vec<_>>();
    assert_eq!((won.len(), freed.len()), (1, 1));
    assert_eq!(won[0].fact.body()["task"], freed[0].fact.body()["task"]);
    assert!(
        [first_task.task, second_task.task]
            .iter()
            .any(|task| won[0].fact.body()["task"] == serde_json::json!(task)),
        "the released claim belongs to the selected READY task",
    );
    let row = durable_head(&fixture)
        .lanes
        .into_iter()
        .find(|row| row.lane == seated.lane)
        .expect("failed automatic lane remains durable");
    assert_eq!((row.occupant, row.disposition.as_str()), (None, "vacant"));
    drop(snapshot);
    finish_resident(&fixture, resident);
}

#[test]
fn repeated_rapid_deaths_hold_the_pending_inbox_lane_without_python() {
    let fixture = Fixture::new("automatic-rapid-death-hold");
    configure_task_fixture(&fixture);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let seated = add_lane(&fixture, &["--life", "Drive", "--vendor", "claude"]);
    let mut ancestor = durable_head(&fixture)
        .lanes
        .into_iter()
        .find(|row| row.lane == seated.lane)
        .and_then(|row| row.occupant)
        .expect("new lane has its first occupant");
    let gate = Command::new("mkfifo")
        .arg(rapid_exit_gate(&fixture))
        .output()
        .expect("rapid occupant exit gate creation launches");
    assert!(
        gate.status.success(),
        "rapid occupant exit gate creation failed: {}",
        String::from_utf8_lossy(&gate.stderr),
    );
    fs::write(rapid_exit_marker(&fixture), b"exit\n")
        .expect("rapid-exit marker writes outside the worktree");
    let sent = invoke_args(
        &fixture,
        &[
            "steer",
            &seated.lane.to_string(),
            "hold this lane after repeated rapid exits",
        ],
        true,
    );
    assert!(
        sent.status.success(),
        "steering send failed: stdout={} stderr={}",
        String::from_utf8_lossy(&sent.stdout),
        String::from_utf8_lossy(&sent.stderr),
    );
    let Outcome::Reply(sent_reply) = value_outcome(&sent) else {
        panic!("successful steering send must reply");
    };
    let sent_answer: wire::intent::DirectiveAnswer =
        wire::decode(&wire::encode(&sent_reply.body).expect("steering answer re-encodes"))
            .expect("steering answer decodes");

    drain(&fixture, seated.lane);
    for launch_count in 1..=3 {
        ancestor =
            end_rapid_successor(&fixture, &mut resident, seated.lane, ancestor, launch_count);
    }
    wait_for_event(
        &fixture.runtime.join("data"),
        || {},
        || {
            let snapshot = journal::JournalReader::open(fixture.journal())
                .ok()?
                .fact_snapshot()
                .ok()?;
            let held = snapshot
                .facts()
                .iter()
                .any(|fact| fact.fact.kind() == FactKind::LaneHeld);
            let vacant = durable_head(&fixture)
                .lanes
                .into_iter()
                .find(|row| row.lane == seated.lane)
                .is_some_and(|row| row.occupant.is_none());
            (held && vacant).then_some(())
        },
    );
    let snapshot = journal::JournalReader::open(fixture.journal())
        .expect("rapid-death journal reads")
        .fact_snapshot()
        .expect("rapid-death facts read");
    assert_eq!(
        snapshot
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == FactKind::LaneHeld)
            .count(),
        1,
        "the shared rapid-death threshold produces one hold",
    );
    assert_eq!(
        snapshot
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == FactKind::LaneExited)
            .count(),
        3,
        "the configured threshold is reached by three real child exits",
    );
    assert!(
        !snapshot
            .facts()
            .iter()
            .any(|fact| fact.fact.kind() == FactKind::SteerDeadLettered),
        "successful starts that later die are held, not deadlettered",
    );
    drop(snapshot);

    let canceled = invoke_args(
        &fixture,
        &["steer", "cancel", &sent_answer.directive.to_string()],
        true,
    );
    assert!(
        canceled.status.success(),
        "fixture directive cancel failed: stdout={} stderr={}",
        String::from_utf8_lossy(&canceled.stdout),
        String::from_utf8_lossy(&canceled.stderr),
    );
    finish_resident(&fixture, resident);
}
