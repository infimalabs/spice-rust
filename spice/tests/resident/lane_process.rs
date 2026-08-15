use super::*;

const RECALL_LIMIT: usize = 16;
pub(super) const SESSION_ID: &str = "018f7777-7777-7444-8888-999999999999";
const SESSION_RECORD: &str =
    r#"{"type":"system","subtype":"init","session_id":"018f7777-7777-7444-8888-999999999999"}"#;
const MISMATCHED_SESSION_RECORD: &str = concat!(
    "{\"type\":\"system\",\"subtype\":\"init\",",
    "\"session_id\":\"018f9999-9999-7444-8888-999999999999\"}",
);

fn invoke_args(
    fixture: &Fixture,
    arguments: &[&str],
    json: bool,
    lane_context: Option<wire::CanonicalUuid>,
) -> Output {
    let mut process = Command::new(env!("CARGO_BIN_EXE_spice"));
    process.env_remove(steer::hook::LANE);
    if let Some(lane) = lane_context {
        process.env(steer::hook::LANE, lane.to_string());
    }
    if json {
        process.arg("--json");
    }
    process
        .args(arguments)
        .current_dir(&fixture.root)
        .output()
        .expect("spice command launches")
}

fn reply_body<T: serde::de::DeserializeOwned>(output: &Output) -> T {
    assert!(
        output.status.success(),
        "command failed: stdout={:?} stderr={:?}",
        output.stdout,
        output.stderr
    );
    let outcome: Outcome<serde_json::Value> =
        wire::decode(&output.stdout).expect("JSON command outcome decodes");
    let Outcome::Reply(reply) = outcome else {
        panic!("successful command must reply");
    };
    let bytes = wire::encode(&reply.body).expect("reply value encodes");
    wire::decode(&bytes).expect("reply body decodes as its selected DTO")
}

fn invoke_printed_next(fixture: &Fixture, next: &str, lane: wire::CanonicalUuid) -> Output {
    let words = next.split_whitespace().collect::<Vec<_>>();
    let Some((binary, arguments)) = words.split_first() else {
        panic!("printed next action is empty");
    };
    assert_eq!(*binary, "spice", "printed next action names the CLI");
    invoke_args(fixture, arguments, true, Some(lane))
}

pub(super) fn configure_fake_vendor(fixture: &Fixture) {
    let launches = fixture.root.join("vendor-launches");
    configure_vendor(
        fixture,
        &format!(
            concat!(
                "#!/bin/sh\n",
                "trap 'exit 0' TERM INT\n",
                "case \" $* \" in *\" --resume 018f7777-7777-7444-8888-999999999999 \"*) ",
                "printf 'resume\\n' >> {} ;; *) printf 'fresh\\n' >> {} ;; esac\n",
                "printf '%s\\n' {}\n",
                "while :; do sleep 1; done\n",
            ),
            shell_word(&launches.display().to_string()),
            shell_word(&launches.display().to_string()),
            shell_word(SESSION_RECORD),
        ),
    );
}

pub(super) fn configure_successor_worktree(fixture: &Fixture) -> PathBuf {
    let common = fixture
        .runtime
        .parent()
        .expect("runtime has common Git parent");
    let successor = fixture.cleanup.join("successor-worktree");
    let git = common.join("worktrees/lane-import-successor");
    fs::create_dir_all(&successor).expect("successor worktree directory");
    fs::create_dir_all(&git).expect("successor Git directory");
    fs::write(
        successor.join(".git"),
        format!("gitdir: {}\n", git.display()),
    )
    .expect("successor Git pointer");
    fs::write(git.join("commondir"), "../..\n").expect("successor common pointer");
    copy_cargo_fixture(fixture, &successor);
    fs::copy(
        fixture.root.join("spice-rust.toml"),
        successor.join("spice-rust.toml"),
    )
    .expect("successor vendor configuration copies");
    successor
}

pub(super) fn initialize_task_git_fixture(fixture: &Fixture) {
    fixture_git(fixture, &["init", "-b", "main"]);
    fixture_git(fixture, &["config", "user.name", "Resident Task Fixture"]);
    fixture_git(fixture, &["config", "user.email", "resident@example.test"]);
    // The stage is build output that `install_app` seeds into the worktree root
    // before this repository exists, so a fixture that does not ignore it reads
    // dirty and every `task next` here refuses with `git::worktree-dirty` instead
    // of reaching the refusal under test. Derived from the constant rather than
    // spelled out, so renaming the stage cannot quietly reintroduce that.
    fs::write(
        fixture.root.join(".gitignore"),
        format!(
            "vendor-launches\norigin.git\npeer-worktree\n{}",
            app_stage_ignore()
        ),
    )
    .expect("task fixture ignore writes");
    fixture_git(
        fixture,
        &["add", ".gitignore", "fake-claude", "spice-rust.toml"],
    );
    fixture_git(fixture, &["commit", "-m", "resident fixture"]);
    assert_fixture_repository_clean(&fixture.root);
}

fn establish_task_baseline(fixture: &Fixture) {
    fixture_git(fixture, &["init", "--bare", "origin.git"]);
    fixture_git(
        fixture,
        &[
            "remote",
            "add",
            "origin",
            &fixture.root.join("origin.git").display().to_string(),
        ],
    );
    fixture_git(fixture, &["push", "--set-upstream", "origin", "main"]);
}

fn peer_commit(fixture: &Fixture, path: &str, contents: &str) -> String {
    let peer = fixture.root.join("peer-worktree");
    let clone = Command::new("git")
        .arg("clone")
        .arg(fixture.root.join("origin.git"))
        .arg(&peer)
        .output()
        .expect("peer clone launches");
    assert!(
        clone.status.success(),
        "peer clone failed: {}",
        String::from_utf8_lossy(&clone.stderr)
    );
    for arguments in [
        ["config", "user.name", "Resident Peer Fixture"].as_slice(),
        ["config", "user.email", "resident-peer@example.test"].as_slice(),
    ] {
        let output = Command::new("git")
            .arg("-C")
            .arg(&peer)
            .args(arguments)
            .output()
            .expect("peer Git launches");
        assert!(
            output.status.success(),
            "peer git {arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::write(peer.join(path), contents).expect("peer commit candidate writes");
    for arguments in [
        ["add", path].as_slice(),
        ["commit", "-m", "peer advance"].as_slice(),
        ["push"].as_slice(),
    ] {
        let output = Command::new("git")
            .arg("-C")
            .arg(&peer)
            .args(arguments)
            .output()
            .expect("peer Git launches");
        assert!(
            output.status.success(),
            "peer git {arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    git_stdout(&peer, &["rev-parse", "HEAD"])
}

fn git_stdout(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .expect("fixture Git launches");
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout)
        .expect("fixture Git output is UTF-8")
        .trim()
        .to_owned()
}

pub(super) fn fixture_git(fixture: &Fixture, arguments: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(&fixture.root)
        .args(arguments)
        .output()
        .expect("fixture Git launches");
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr),
    );
}

fn configure_vendor(fixture: &Fixture, script: &str) {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;

    let binary = fixture.root.join("fake-claude");
    fs::write(&binary, script).expect("fake vendor writes");
    #[cfg(unix)]
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))
        .expect("fake vendor becomes executable");
    fs::write(
        fixture.root.join("spice-rust.toml"),
        format!(
            concat!(
                "[lane]\n",
                "lease = \"30s\"\n",
                "\n[vendor.claude]\n",
                "binary = {:?}\n",
            ),
            binary.display().to_string()
        ),
    )
    .expect("fixture vendor configuration writes");
}

fn exiting_vendor(fixture: &Fixture, code: i32) -> PathBuf {
    let exit_marker = fixture.root.join("vendor-exited");
    configure_vendor(
        fixture,
        &format!(
            "#!/bin/sh\ntrap 'printf exited > {}' EXIT\nprintf '%s\\n' {}\nexit {code}\n",
            shell_word(&exit_marker.display().to_string()),
            shell_word(SESSION_RECORD),
        ),
    );
    exit_marker
}

fn recording_vendor(fixture: &Fixture, complete: &str, terminal: &str) -> PathBuf {
    let exit_marker = fixture.root.join("recording-vendor-exited");
    configure_vendor(
        fixture,
        &format!(
            concat!(
                "#!/bin/sh\n",
                "trap 'printf exited > {}' EXIT\n",
                "printf '%s\\n' {}\n",
                "printf '%s\\n' {}\n",
                "printf '%s' {}\n",
                "printf '%s' 'opaque diagnostic' >&2\n",
                "exit 0\n",
            ),
            shell_word(&exit_marker.display().to_string()),
            shell_word(SESSION_RECORD),
            shell_word(complete),
            shell_word(terminal),
        ),
    );
    exit_marker
}

fn assert_latest_launch_resumed(fixture: &Fixture) {
    let launches = fs::read_to_string(fixture.root.join("vendor-launches"))
        .expect("fake vendor launch modes read");
    assert_eq!(launches.lines().next_back(), Some("resume"));
}

fn refusal_reason(output: &Output) -> String {
    assert_eq!(
        output.status.code(),
        Some(REFUSAL_EXIT),
        "expected refusal: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let Outcome::Refusal(refusal) = outcome(output) else {
        panic!("fixture command must return a refusal");
    };
    refusal.reason().to_owned()
}

fn assert_session_binding(
    fixture: &Fixture,
    row: &wire::intent::LaneAnswerRow,
    epoch: wire::CanonicalUuid,
) {
    assert!(row.bound, "the safe row retains session presence");
    let journal = journal::Journal::open(fixture.journal()).expect("lane journal opens");
    let head = journal
        .read(|read| read.residency_head())
        .expect("lane residency reads")
        .expect("lane residency exists");
    let durable = head
        .body()
        .lanes
        .iter()
        .find(|durable| durable.lane == row.lane)
        .expect("safe lane remains durable");
    let session = durable
        .session
        .as_ref()
        .expect("lane session authority exists");
    let launch = session
        .launch
        .as_ref()
        .expect("current lane session records its launch identity");
    assert_eq!(
        (
            row.model.as_deref(),
            row.effort.as_deref(),
            row.git.as_deref(),
        ),
        (
            Some(launch.model.as_str()),
            Some(launch.effort.as_str()),
            launch.git.as_deref(),
        ),
        "the safe read publishes the durable launch identity"
    );
    assert_eq!(
        (
            session.lane,
            session.occupant,
            session.worktree.as_str(),
            session.vendor.as_str(),
            session.epoch,
            session.stream.as_str(),
            session.identity.as_str(),
        ),
        (
            row.lane,
            row.occupant.expect("session lane is occupied"),
            row.worktree.as_str(),
            row.vendor.as_str(),
            epoch,
            "stdout",
            SESSION_ID,
        )
    );
    let tape = tape::Tape::open_existing(
        fixture
            .root
            .join(".git/.spice-rs/tape")
            .join(row.lane.to_string())
            .join(row.vendor.as_str()),
    )
    .expect("session tape opens");
    let bytes = tape
        .read(tape::Locus::new(
            session.locus.member,
            session.locus.offset,
            session.locus.length,
        ))
        .expect("session locus resolves");
    assert_eq!(bytes, format!("{SESSION_RECORD}\n").as_bytes());
}

fn seed_occupied_lane(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    capability: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
) {
    let journal = journal::Journal::open(fixture.journal()).expect("lane fixture journal opens");
    let bus = Bus::start(journal).expect("lane fixture bus starts");
    let current = bus
        .residency_head()
        .expect("lane fixture head reads")
        .expect("lane fixture head exists");
    let epoch = current.body().epoch;
    let worktree = fs::canonicalize(&fixture.root)
        .expect("lane fixture worktree canonicalizes")
        .display()
        .to_string();
    let tape = tape::Tape::open(
        fixture
            .root
            .join(".git/.spice-rs/tape")
            .join(lane.to_string())
            .join("claude"),
    )
    .expect("session tape opens");
    let locus = tape
        .tee(SESSION_RECORD.as_bytes())
        .expect("session declaration is durable");
    let session = wire::LaneSession {
        lane,
        occupant,
        worktree: worktree.clone(),
        vendor: "claude".parse().expect("fixture vendor"),
        launch: Some(wire::body::LaneLaunchIdentity::new("sonnet", "high")),
        epoch,
        stream: "stdout".to_owned(),
        identity: SESSION_ID.to_owned(),
        locus: wire::TapeLocus {
            member: locus.member,
            offset: locus.offset,
            length: locus.length,
        },
    };
    let mut next = current.body().clone();
    next.lanes.push(wire::LaneResidency {
        lane,
        lineage_id: lane,
        name: None,
        worktree: worktree.clone(),
        life: "drive".to_owned(),
        vendor: "claude".parse().expect("fixture vendor"),
        occupant: Some(occupant),
        lineage: vec![occupant],
        capabilities: vec![capability],
        session: Some(session.clone()),
        disposition: "occupied".to_owned(),
    });
    next.lanes.sort_by_key(|row| row.lane);
    let origin = format!("resident:{epoch}")
        .parse()
        .expect("lane fixture origin");
    let mut write = JournalWrite::new(
        Actor::Spice,
        origin,
        instant("2030-04-12T18:47:09.271Z"),
        Producer::ResidentRecognition,
    );
    let seated = write
        .fact(
            wire::body::LaneSeated {
                lane,
                lineage_id: lane,
                capability,
                name: None,
                worktree: worktree.clone(),
                life: "drive".to_owned(),
                vendor: "claude".parse().expect("fixture vendor"),
            },
            FactCause::Existing(current.position()),
        )
        .expect("occupied lane seated fact");
    let declared = write
        .fact(
            wire::body::LaneSessionDeclared {
                lane,
                occupant,
                worktree: worktree.clone(),
                vendor: "claude".parse().expect("fixture vendor"),
                launch: Some(wire::body::LaneLaunchIdentity::new("sonnet", "high")),
                epoch,
                stream: "stdout".to_owned(),
                identity: SESSION_ID.to_owned(),
                member: locus.member,
                offset: locus.offset,
                length: locus.length,
            },
            FactCause::Pending(seated),
        )
        .expect("occupied lane session fact");
    let started = write
        .fact(
            wire::body::LaneStarted {
                lane,
                occupant,
                epoch,
                group: 42,
            },
            FactCause::Pending(declared),
        )
        .expect("occupied lane started fact");
    write.residency_head(started, move |_| next);
    committed(&bus, write);
    bus.shutdown().expect("lane fixture bus stops");
}

fn durable_capability(fixture: &Fixture) -> wire::CanonicalUuid {
    journal::Journal::open(fixture.journal())
        .expect("lane journal opens")
        .read(|read| read.residency_head())
        .expect("lane residency reads")
        .expect("lane residency exists")
        .into_body()
        .lanes[0]
        .capabilities[0]
}

fn seat_and_recall(fixture: &Fixture) -> (wire::intent::LaneChangeAnswer, wire::CanonicalUuid) {
    let text = invoke_args(fixture, &["lane"], false, None);
    assert!(text.status.success(), "lane text failed: {:?}", text.stderr);
    let initial: wire::intent::LaneAnswer =
        wire::decode(&text.stdout).expect("text lane body decodes");
    assert!(initial.lanes.is_empty());
    assert_eq!(initial.topology_revision, 0);
    let added = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        fixture,
        &[
            "lane", "add", "--name", "Ava", "--life", "Drive", "--vendor", "claude",
        ],
        true,
        None,
    ));
    assert_eq!(
        (added.from.as_str(), added.to.as_str()),
        ("absent", "occupied")
    );
    assert_eq!(added.topology_revision, 1);
    let lanes =
        reply_body::<wire::intent::LaneAnswer>(&invoke_args(fixture, &["lane"], true, None));
    let row = &lanes.lanes[0];
    assert_eq!(lanes.topology_revision, 1);
    assert_eq!(row.lane, added.lane);
    assert_eq!(row.name.as_deref(), Some("Ava"));
    assert_eq!(
        row.worktree,
        fs::canonicalize(&fixture.root)
            .expect("fixture root canonicalizes")
            .display()
            .to_string()
    );
    assert_eq!(
        (row.life.as_str(), row.vendor.as_str()),
        ("drive", "claude")
    );
    assert_eq!(
        (row.disposition.as_str(), row.lineage.len()),
        ("occupied", 1)
    );
    assert_session_binding(fixture, row, lanes.epoch);
    assert_eq!(row.liveness, wire::intent::LaneLiveness::Running);
    let contended = invoke_args(fixture, &["lane", "add"], true, None);
    assert_eq!(contended.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) = outcome(&contended) else {
        panic!("a second lane in one worktree must refuse");
    };
    assert_eq!(refusal.reason(), "lane::worktree-occupied");
    let capability = durable_capability(fixture);
    let journal = journal::Journal::open(fixture.journal()).expect("lane journal opens");
    let facts = journal.fact_snapshot().expect("lane facts read");
    let lifecycle = facts
        .facts()
        .iter()
        .filter(|fact| {
            matches!(
                fact.fact.kind(),
                FactKind::LaneSeated | FactKind::LaneSessionDeclared | FactKind::LaneStarted
            )
        })
        .map(|fact| fact.fact.kind())
        .collect::<Vec<_>>();
    assert_eq!(
        lifecycle,
        [
            FactKind::LaneSeated,
            FactKind::LaneSessionDeclared,
            FactKind::LaneStarted,
        ]
    );
    drop(journal);
    let recalled = reply_body::<wire::intent::RecallAnswer>(&invoke_args(
        fixture,
        &["recall"],
        true,
        Some(added.lane),
    ));
    assert_eq!(
        (recalled.lane, recalled.residency.lanes.len()),
        (Some(added.lane), 1)
    );
    let row = &recalled.residency.lanes[0];
    assert!(row.bound);
    assert_eq!(row.model.as_deref(), Some("sonnet"));
    assert_eq!(row.effort.as_deref(), Some("high"));
    assert_eq!(row.session.as_deref(), Some(SESSION_ID));
    let safe = wire::encode(&recalled.residency).expect("safe recall residency encodes");
    assert!(!String::from_utf8_lossy(&safe).contains(&capability.to_string()));
    assert!(recalled.recent.len() <= RECALL_LIMIT);
    assert!(
        recalled
            .recent
            .iter()
            .all(|entry| entry.seq <= recalled.barrier)
    );
    assert!(
        recalled
            .recent
            .iter()
            .any(|entry| entry.kind == FactKind::LaneSeated)
    );
    assert!(
        recalled
            .recent
            .iter()
            .any(|entry| entry.kind == FactKind::LaneSessionDeclared)
    );
    let forbidden = invoke_args(fixture, &["lane"], true, Some(added.lane));
    assert_eq!(forbidden.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&forbidden) else {
        panic!("lane principal must not cross an operator-only leaf");
    };
    assert_eq!(fault.reason, "command::principal-mismatch");
    (added, capability)
}

fn drain_and_restart(
    fixture: &Fixture,
    resident: RunningResident,
    lane: wire::CanonicalUuid,
    capability: wire::CanonicalUuid,
) -> RunningResident {
    let started = Monotonic::now();
    let drained = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        fixture,
        &["lane", "drain", &lane.to_string()],
        true,
        None,
    ));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        (drained.from.as_str(), drained.to.as_str()),
        ("occupied", "vacant")
    );
    assert_eq!(drained.topology_revision, 1);
    let vacant =
        reply_body::<wire::intent::LaneAnswer>(&invoke_args(fixture, &["lane"], true, None));
    assert_eq!(
        (vacant.lanes[0].occupant, vacant.lanes[0].lineage.len()),
        (None, 1)
    );
    assert_eq!(vacant.topology_revision, 1);
    let repeated = invoke_args(fixture, &["lane", "drain", &lane.to_string()], true, None);
    assert_eq!(repeated.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) = outcome(&repeated) else {
        panic!("draining a vacant lane must refuse");
    };
    assert_eq!(refusal.reason(), "lane::already-vacant");
    let down = invoke(fixture, "down");
    assert!(
        down.status.success(),
        "first down failed: {:?}",
        down.stderr
    );
    assert!(resident.wait().status.success(), "first resident failed");
    let mut replacement = RunningResident::spawn(fixture);
    replacement.assert_ready(fixture);
    let snapshot =
        reply_body::<wire::intent::LaneAnswer>(&invoke_args(fixture, &["lane"], true, None));
    assert_eq!(snapshot.topology_revision, 1);
    assert_eq!(snapshot.lanes[0].occupant, None);
    assert_eq!(snapshot.lanes[0].name.as_deref(), Some("Ava"));
    assert!(snapshot.lanes[0].bound);
    assert_eq!(
        snapshot.lanes[0].liveness,
        wire::intent::LaneLiveness::Vacant
    );
    assert_eq!(durable_capability(fixture), capability);
    replacement
}

fn wake_and_finish(fixture: &Fixture, resident: RunningResident, lane: wire::CanonicalUuid) {
    let woke = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        fixture,
        &["lane", "wake", &lane.to_string()],
        true,
        None,
    ));
    assert_eq!(
        (woke.from.as_str(), woke.to.as_str()),
        ("vacant", "occupied")
    );
    let occupied =
        reply_body::<wire::intent::LaneAnswer>(&invoke_args(fixture, &["lane"], true, None));
    assert_eq!(occupied.lanes[0].name.as_deref(), Some("Ava"));
    assert_eq!(occupied.lanes[0].lineage.len(), 2);
    assert!(occupied.lanes[0].occupant.is_some());
    assert_latest_launch_resumed(fixture);
    let repeated = invoke_args(fixture, &["lane", "wake", &lane.to_string()], true, None);
    assert_eq!(repeated.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) = outcome(&repeated) else {
        panic!("waking an occupied lane must refuse");
    };
    assert_eq!(refusal.reason(), "lane::already-occupied");
    let text = invoke_args(fixture, &["recall", &lane.to_string()], false, None);
    assert!(
        text.status.success(),
        "recall text failed: {:?}",
        text.stderr
    );
    let recall: wire::intent::RecallAnswer =
        wire::decode(&text.stdout).expect("text recall decodes");
    assert_eq!(recall.lane, Some(lane));
    let final_drain = invoke_args(fixture, &["lane", "drain", &lane.to_string()], true, None);
    assert!(
        final_drain.status.success(),
        "final drain failed: {:?}",
        final_drain.stderr
    );
    let down = invoke(fixture, "down");
    assert!(
        down.status.success(),
        "final down failed: {:?}",
        down.stderr
    );
    assert!(resident.wait().status.success(), "final resident failed");
    let journal = journal::Journal::open(fixture.journal()).expect("lane journal reopens");
    journal.verify().expect("lane workflow authority verifies");
    let kinds = journal
        .fact_snapshot()
        .expect("lane facts read")
        .facts()
        .iter()
        .map(|fact| fact.fact.kind())
        .collect::<Vec<_>>();
    assert!(kinds.contains(&FactKind::LaneSeated));
    assert!(kinds.contains(&FactKind::LaneStarted));
    assert!(
        kinds
            .iter()
            .any(|kind| matches!(kind, FactKind::LaneDrained | FactKind::LaneExited))
    );
}

include!("lane_process/flow_tail.rs");
#[path = "lane_process/task_and_recovery.rs"]
mod task_and_recovery;
