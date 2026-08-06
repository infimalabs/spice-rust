use super::*;

const RECALL_LIMIT: usize = 16;
const SESSION_ID: &str = "018f7777-7777-7444-8888-999999999999";
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
    process.env_remove("SPICE_LANE"); // env-policy: allow
    if let Some(lane) = lane_context {
        process.env("SPICE_LANE", lane.to_string()); // env-policy: allow
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

fn configure_successor_worktree(fixture: &Fixture) -> PathBuf {
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
    fs::copy(
        fixture.root.join("spice.toml"),
        successor.join("spice.toml"),
    )
    .expect("successor vendor configuration copies");
    successor
}

fn initialize_task_git_fixture(fixture: &Fixture) {
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
        format!("vendor-launches\norigin.git\n{}", app_stage_ignore()),
    )
    .expect("task fixture ignore writes");
    fixture_git(fixture, &["add", ".gitignore", "fake-claude", "spice.toml"]);
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

fn fixture_git(fixture: &Fixture, arguments: &[&str]) {
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
        fixture.root.join("spice.toml"),
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

fn exiting_vendor(fixture: &Fixture, code: i32) -> std::thread::JoinHandle<()> {
    let exit_pipe = fixture.root.join("vendor-exited");
    let made = Command::new("mkfifo")
        .arg(&exit_pipe)
        .output()
        .expect("mkfifo launches");
    assert!(made.status.success(), "mkfifo failed: {:?}", made.stderr);
    configure_vendor(
        fixture,
        &format!(
            "#!/bin/sh\nexec 3>{}\nprintf '%s\\n' {}\nexit {code}\n",
            shell_word(&exit_pipe.display().to_string()),
            shell_word(SESSION_RECORD),
        ),
    );
    std::thread::spawn(move || {
        let mut pipe = fs::File::open(exit_pipe).expect("exit observer opens");
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes)
            .expect("exit observer reaches EOF");
        assert!(bytes.is_empty(), "exit observer carries no payload");
    })
}

fn recording_vendor(
    fixture: &Fixture,
    complete: &str,
    terminal: &str,
) -> std::thread::JoinHandle<()> {
    let exit_pipe = fixture.root.join("recording-vendor-exited");
    let made = Command::new("mkfifo")
        .arg(&exit_pipe)
        .output()
        .expect("mkfifo launches");
    assert!(made.status.success(), "mkfifo failed: {:?}", made.stderr);
    configure_vendor(
        fixture,
        &format!(
            concat!(
                "#!/bin/sh\n",
                "exec 3>{}\n",
                "printf '%s\\n' {}\n",
                "printf '%s\\n' {}\n",
                "printf '%s' {}\n",
                "printf '%s' 'opaque diagnostic' >&2\n",
                "exit 0\n",
            ),
            shell_word(&exit_pipe.display().to_string()),
            shell_word(SESSION_RECORD),
            shell_word(complete),
            shell_word(terminal),
        ),
    );
    std::thread::spawn(move || {
        let mut pipe = fs::File::open(exit_pipe).expect("recording exit observer opens");
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes)
            .expect("recording exit observer reaches EOF");
        assert!(
            bytes.is_empty(),
            "recording exit observer carries no payload"
        );
    })
}

fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn assert_latest_launch_resumed(fixture: &Fixture) {
    let launches = fs::read_to_string(fixture.root.join("vendor-launches"))
        .expect("fake vendor launch modes read");
    assert_eq!(launches.lines().next_back(), Some("resume"));
}

fn refusal_reason(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) = outcome(output) else {
        panic!("fixture command must return a refusal");
    };
    refusal.reason
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
            .join(".git/.spice/tape")
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
            .join(".git/.spice/tape")
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
    let lanes =
        reply_body::<wire::intent::LaneAnswer>(&invoke_args(fixture, &["lane"], true, None));
    let row = &lanes.lanes[0];
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
    assert_eq!(refusal.reason, "lane::worktree-occupied");
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
    assert!(recalled.residency.lanes[0].bound);
    let safe = wire::encode(&recalled.residency).expect("safe recall residency encodes");
    assert!(!String::from_utf8_lossy(&safe).contains(&capability.to_string()));
    assert!(!String::from_utf8_lossy(&safe).contains(SESSION_ID));
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
    let vacant =
        reply_body::<wire::intent::LaneAnswer>(&invoke_args(fixture, &["lane"], true, None));
    assert_eq!(
        (vacant.lanes[0].occupant, vacant.lanes[0].lineage.len()),
        (None, 1)
    );
    let repeated = invoke_args(fixture, &["lane", "drain", &lane.to_string()], true, None);
    assert_eq!(repeated.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) = outcome(&repeated) else {
        panic!("draining a vacant lane must refuse");
    };
    assert_eq!(refusal.reason, "lane::already-vacant");
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
    assert_eq!(refusal.reason, "lane::already-occupied");
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

#[test]
fn lane_lifecycle_and_recall_cross_the_resident_in_both_forms() {
    let fixture = Fixture::new("lane-lifecycle");
    configure_fake_vendor(&fixture);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let (added, capability) = seat_and_recall(&fixture);
    resident = drain_and_restart(&fixture, resident, added.lane, capability);
    wake_and_finish(&fixture, resident, added.lane);
}

#[test]
fn predecessor_import_launches_the_successor_through_the_shared_occupant_path() {
    let fixture = Fixture::linked("lane-import-launch");
    configure_fake_vendor(&fixture);
    let successor = configure_successor_worktree(&fixture);
    let successor = fs::canonicalize(successor).expect("successor worktree canonicalizes");
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);

    let added = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &[
            "lane", "add", "--name", "Ava", "--life", "Drive", "--vendor", "claude",
        ],
        true,
        None,
    ));
    let drained = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &["lane", "drain", &added.lane.to_string()],
        true,
        None,
    ));
    assert_eq!(
        (drained.from.as_str(), drained.to.as_str()),
        ("occupied", "vacant")
    );

    let imported = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &[
            "lane",
            "import",
            &added.lane.to_string(),
            &successor.display().to_string(),
        ],
        true,
        None,
    ));
    assert_eq!(
        (imported.from.as_str(), imported.to.as_str()),
        (
            fs::canonicalize(&fixture.root)
                .expect("predecessor worktree canonicalizes")
                .display()
                .to_string()
                .as_str(),
            successor.display().to_string().as_str(),
        )
    );
    let lanes =
        reply_body::<wire::intent::LaneAnswer>(&invoke_args(&fixture, &["lane"], true, None));
    let row = &lanes.lanes[0];
    assert_eq!(row.name.as_deref(), Some("Ava"));
    assert_eq!(row.worktree, successor.display().to_string());
    assert_eq!(row.disposition, "occupied");
    assert_eq!(row.lineage.len(), 2);
    assert_eq!(row.occupant, row.lineage.last().copied());
    assert!(row.bound, "import preserves resumable session presence");
    let safe = wire::encode(&lanes).expect("imported safe lanes encode");
    assert!(!String::from_utf8_lossy(&safe).contains(SESSION_ID));
    assert_latest_launch_resumed(&fixture);
    let journal = journal::Journal::open(fixture.journal()).expect("import journal opens");
    let kinds = journal
        .fact_snapshot()
        .expect("import facts read")
        .facts()
        .iter()
        .map(|fact| fact.fact.kind())
        .collect::<Vec<_>>();
    for kind in [
        FactKind::LaneImported,
        FactKind::LaneSessionDeclared,
        FactKind::LaneStarted,
    ] {
        assert!(kinds.contains(&kind), "import recorded no {kind:?}");
    }
    drop(journal);

    let final_drain = invoke_args(
        &fixture,
        &["lane", "drain", &added.lane.to_string()],
        true,
        None,
    );
    assert!(
        final_drain.status.success(),
        "final drain failed: {:?}",
        final_drain.stderr
    );
    let down = invoke(&fixture, "down");
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

#[test]
fn manual_hidden_task_creation_refuses_for_operator_and_lane_processes() {
    let fixture = Fixture::new("hidden-task-creation");
    configure_fake_vendor(&fixture);
    let mut config = fs::read_to_string(fixture.root.join("spice.toml"))
        .expect("task fixture configuration reads");
    config.push_str("\n[tasks]\nhidden_stems = [\"scratch\"]\n");
    fs::write(fixture.root.join("spice.toml"), config).expect("task fixture project policy writes");
    initialize_task_git_fixture(&fixture);
    establish_task_baseline(&fixture);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);

    let added_lane = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &["lane", "add", "--life", "Drive", "--vendor", "claude"],
        true,
        None,
    ));
    fs::write(fixture.root.join("capture.txt"), "one loose task commit\n")
        .expect("capture candidate writes");
    fixture_git(&fixture, &["add", "capture.txt"]);
    fixture_git(&fixture, &["commit", "-m", "capture candidate"]);
    for (arguments, actor) in [
        (
            vec!["task", "add", "Manual oops", "--project", ".oops"],
            None,
        ),
        (
            vec![
                "task",
                "add",
                "Manual configured hidden",
                "--project",
                ".scratch.audit",
            ],
            Some(added_lane.lane),
        ),
        (
            vec![
                "task",
                "capture",
                "--title",
                "Captured oops",
                "--project",
                ".oops.correctness",
            ],
            None,
        ),
        (
            vec![
                "task",
                "capture",
                "--title",
                "Captured configured hidden",
                "--project",
                ".scratch.audit",
            ],
            Some(added_lane.lane),
        ),
    ] {
        let before = journal_cursor(&fixture);
        let refused = invoke_args(&fixture, &arguments, true, actor);
        assert_eq!(refusal_reason(&refused), "task::project-system-reserved",);
        assert_eq!(
            journal_cursor(&fixture),
            before,
            "{arguments:?} wrote facts"
        );
    }

    let system_task = reply_body::<wire::intent::TaskAnswer>(&invoke_args(
        &fixture,
        &["task", "oops", "System-owned oops"],
        true,
        None,
    ));
    let visible =
        reply_body::<wire::intent::BoardAnswer>(&invoke_args(&fixture, &["board"], true, None));
    let hidden = visible
        .tasks
        .iter()
        .find(|task| task.statement.body.handle == system_task.task)
        .expect("system-created hidden task remains typed board evidence");
    assert_eq!(hidden.statement.body.project, ".oops");
    assert_eq!(hidden.visibility.hidden_stem(), Some("oops"));

    let drained = invoke_args(
        &fixture,
        &["lane", "drain", &added_lane.lane.to_string()],
        true,
        None,
    );
    assert!(
        drained.status.success(),
        "drain failed: {:?}",
        drained.stderr
    );
    let down = invoke(&fixture, "down");
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

#[test]
fn typed_task_commands_retain_the_ambient_lane_principal() {
    let fixture = Fixture::new("lane-task-principal");
    configure_fake_vendor(&fixture);
    let mut config = fs::read_to_string(fixture.root.join("spice.toml"))
        .expect("task fixture configuration reads");
    config.push_str("\n[tasks]\nstems = [\"spice\"]\n");
    fs::write(fixture.root.join("spice.toml"), config).expect("task fixture project policy writes");
    initialize_task_git_fixture(&fixture);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);

    let added_lane = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &["lane", "add", "--life", "Drive", "--vendor", "claude"],
        true,
        None,
    ));

    let added_task = reply_body::<wire::intent::TaskAnswer>(&invoke_args(
        &fixture,
        &[
            "task",
            "add",
            "Preserve the typed command lane principal",
            "--project",
            "spice.client",
        ],
        true,
        Some(added_lane.lane),
    ));

    let implementation = reply_body::<wire::intent::ClaimAnswer>(&invoke_args(
        &fixture,
        &["task", "next"],
        true,
        Some(added_lane.lane),
    ));
    assert_eq!(
        (implementation.task, implementation.holder),
        (added_task.task.clone(), Actor::Lane(added_lane.lane))
    );
    let completed = reply_body::<wire::intent::TaskAnswer>(&invoke_args(
        &fixture,
        &[
            "task",
            "done",
            "--validation",
            "lane principal survived typed completion",
        ],
        true,
        Some(added_lane.lane),
    ));
    assert_eq!(completed.task, added_task.task);

    let review = reply_body::<wire::intent::ClaimAnswer>(&invoke_args(
        &fixture,
        &["task", "next"],
        true,
        Some(added_lane.lane),
    ));
    assert_eq!(
        (review.task, review.holder),
        (added_task.task.clone(), Actor::Lane(added_lane.lane))
    );
    let reviewed = reply_body::<wire::intent::TaskAnswer>(&invoke_args(
        &fixture,
        &["task", "review", &added_task.task.to_string()],
        true,
        Some(added_lane.lane),
    ));
    assert_eq!(
        (reviewed.task, reviewed.status.as_str()),
        (added_task.task, "completed")
    );

    let drained = invoke_args(
        &fixture,
        &["lane", "drain", &added_lane.lane.to_string()],
        true,
        None,
    );
    assert!(
        drained.status.success(),
        "drain failed: {:?}",
        drained.stderr
    );
    let down = invoke(&fixture, "down");
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

fn assert_recovered_lane(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    ancestor: wire::CanonicalUuid,
    capability: wire::CanonicalUuid,
) {
    let discovery = fixture.discovery();
    let snapshot =
        reply_body::<wire::intent::LaneAnswer>(&invoke_args(fixture, &["lane"], true, None));
    let row = &snapshot.lanes[0];
    assert_eq!((row.lane, row.disposition.as_str()), (lane, "occupied"));
    assert!(row.bound);
    assert_eq!(
        (row.lineage.first(), row.lineage.len()),
        (Some(&ancestor), 2)
    );
    assert_eq!(row.occupant, row.lineage.last().copied());
    let journal = journal::Journal::open(fixture.journal()).expect("restored journal opens");
    let head = journal
        .read(|read| read.residency_head())
        .expect("restored residency reads")
        .expect("restored residency exists")
        .into_body();
    assert_eq!(head.epoch, discovery.epoch);
    assert_eq!(head.lanes[0].capabilities, [capability]);
    assert_eq!(
        head.lanes[0]
            .session
            .as_ref()
            .map(|session| session.identity.as_str()),
        Some(SESSION_ID)
    );
    assert_latest_launch_resumed(fixture);
    let facts = journal.fact_snapshot().expect("restored facts read");
    let kinds = facts
        .facts()
        .iter()
        .map(|fact| fact.fact.kind())
        .collect::<Vec<_>>();
    assert!(kinds.contains(&FactKind::LaneDegraded));
    assert!(kinds.contains(&FactKind::LaneRenewed));
    drop(journal);
    let recall = reply_body::<wire::intent::RecallAnswer>(&invoke_args(
        fixture,
        &["recall"],
        true,
        Some(lane),
    ));
    assert_eq!(recall.lane, Some(lane));
    assert!(
        recall
            .recent
            .iter()
            .any(|entry| entry.kind == FactKind::LaneRenewed)
    );
}

#[test]
fn replacement_resident_reconstructs_durable_occupied_lane() {
    let fixture = Fixture::new("lane-restart-occupied");
    configure_fake_vendor(&fixture);
    let mut initial = RunningResident::spawn(&fixture);
    initial.assert_ready(&fixture);
    let down = invoke(&fixture, "down");
    assert!(
        down.status.success(),
        "initial down failed: {:?}",
        down.stderr
    );
    assert!(initial.wait().status.success(), "initial resident failed");
    let lane = uuid("00000000-0000-4000-8000-000000000025");
    let capability = uuid("00000000-0000-4000-8000-000000000026");
    let ancestor = uuid("00000000-0000-4000-8000-000000000027");
    seed_occupied_lane(&fixture, lane, capability, ancestor);
    let mut replacement = RunningResident::spawn(&fixture);
    replacement.assert_ready(&fixture);
    assert_recovered_lane(&fixture, lane, ancestor, capability);
    let drained = invoke_args(&fixture, &["lane", "drain", &lane.to_string()], true, None);
    assert!(
        drained.status.success(),
        "drain failed: {:?}",
        drained.stderr
    );
    let down = invoke(&fixture, "down");
    assert!(
        down.status.success(),
        "replacement down failed: {:?}",
        down.stderr
    );
    assert!(
        replacement.wait().status.success(),
        "replacement resident failed"
    );
}

#[test]
fn drain_records_an_occupant_that_already_exited() {
    let fixture = Fixture::new("lane-exited");
    let exited = exiting_vendor(&fixture, 7);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let added = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &["lane", "add", "--vendor", "claude"],
        true,
        None,
    ));
    exited.join().expect("vendor exit is observed");
    let drained = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &["lane", "drain", &added.lane.to_string()],
        true,
        None,
    ));
    assert_eq!(
        (drained.from.as_str(), drained.to.as_str()),
        ("occupied", "vacant")
    );
    let journal = journal::Journal::open(fixture.journal()).expect("exited lane journal opens");
    let facts = journal.fact_snapshot().expect("exited lane facts read");
    let exited = facts
        .facts()
        .iter()
        .find(|fact| fact.fact.kind() == FactKind::LaneExited)
        .expect("self-exited occupant is recorded");
    assert_eq!(exited.fact.body()["code"], 7);
    drop(journal);
    let down = invoke(&fixture, "down");
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

#[test]
fn resident_capture_is_byte_exact_and_listenable_through_terminal_residue() {
    const COMPLETE: &str = concat!(
        r#"{"type":"assistant","message":{"type":"message","#,
        r#""role":"assistant","content":[{"type":"text","text":"done"}],"#,
        r#""usage":{"input_tokens":7,"cache_read_input_tokens":5,"#,
        r#""cache_creation_input_tokens":3,"output_tokens":2}}}"#,
    );
    const TERMINAL: &str = r#"{"type":"assistant","message":"#;

    let fixture = Fixture::new("lane-capture");
    let exited = recording_vendor(&fixture, COMPLETE, TERMINAL);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let added = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &["lane", "add", "--vendor", "claude"],
        true,
        None,
    ));
    exited.join().expect("recording vendor exit is observed");
    let drained = invoke_args(
        &fixture,
        &["lane", "drain", &added.lane.to_string()],
        true,
        None,
    );
    assert!(
        drained.status.success(),
        "drain failed: {:?}",
        drained.stderr
    );

    let tape_root = fixture
        .root
        .join(".git/.spice/tape")
        .join(added.lane.to_string())
        .join("claude");
    let tape = tape::Tape::open_existing(&tape_root).expect("resident transcript tape opens");
    let extents = tape.extents().expect("resident transcript extents read");
    assert_eq!(extents.len(), 1);
    let bytes = tape
        .read(tape::Locus::new(extents[0].member, 0, extents[0].length))
        .expect("resident transcript reads");
    assert_eq!(
        bytes,
        format!("{SESSION_RECORD}\n{COMPLETE}\n{TERMINAL}").as_bytes()
    );

    let replay = listen::retro(
        fixture.root.join(".git/.spice/tape"),
        listen::RetroSelector::Lane(added.lane.to_string()),
    )
    .expect("resident transcript replays");
    assert!(replay.observations.iter().any(|observation| matches!(
        observation,
        listen::Observation::Event {
            event: listen::SurfaceEvent::Prose { text, .. },
            ..
        } if text == "done"
    )));
    assert!(replay.observations.iter().any(|observation| matches!(
        observation,
        listen::Observation::Residue {
            residue: listen::SurfaceResidue::Truncated {},
            ..
        }
    )));

    let down = invoke(&fixture, "down");
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

#[test]
fn missing_and_duplicate_declarations_refuse_and_fully_unwind_lane_add() {
    for (name, duplicate, expected) in [
        ("missing", false, "lane::session-missing"),
        ("duplicate", true, "lane::session-duplicate"),
    ] {
        let fixture = Fixture::new(&format!("lane-session-{name}"));
        let stopped = fixture.root.join("vendor-stopped");
        let vendor_pid = fixture.root.join("vendor-pid");
        let script = if !duplicate {
            format!(
                concat!(
                    "#!/bin/sh\n",
                    "trap 'printf stopped > {}; exit 0' TERM INT\n",
                    "printf '%s\\n' \"$$\" > {}\n",
                    "while :; do sleep 1; done\n",
                ),
                shell_word(&stopped.display().to_string()),
                shell_word(&vendor_pid.display().to_string()),
            )
        } else {
            format!(
                concat!(
                    "#!/bin/sh\n",
                    "trap 'printf stopped > {}; exit 0' TERM INT\n",
                    "printf '%s\\n' \"$$\" > {}\n",
                    "printf '%s\\n' {}\n",
                    "printf '%s\\n' {}\n",
                    "while :; do sleep 1; done\n",
                ),
                shell_word(&stopped.display().to_string()),
                shell_word(&vendor_pid.display().to_string()),
                shell_word(SESSION_RECORD),
                shell_word(SESSION_RECORD),
            )
        };
        configure_vendor(&fixture, &script);
        let mut resident = RunningResident::spawn(&fixture);
        resident.assert_ready(&fixture);

        let started = Monotonic::now();
        let added = invoke_args(&fixture, &["lane", "add", "--vendor", "claude"], true, None);
        assert_eq!(refusal_reason(&added), expected);
        assert!(started.elapsed() < LANE_DEADLINE_BOUND);
        let lanes =
            reply_body::<wire::intent::LaneAnswer>(&invoke_args(&fixture, &["lane"], true, None));
        assert!(lanes.lanes.is_empty());
        // The refusal returns without waiting for the vendor's TERM trap, so the
        // stop is observed rather than read at the return.
        let stop = wait_for_vendor_stop(&fixture.cleanup, &vendor_pid, &stopped);
        assert_eq!(
            stop, "stopped",
            "refused vendor records bounded termination"
        );

        let down = invoke(&fixture, "down");
        assert!(down.status.success(), "down failed: {:?}", down.stderr);
        assert!(resident.wait().status.success(), "resident failed");
    }
}

#[test]
fn mismatched_resume_and_moved_tape_refuse_without_advancing_the_vacant_lane() {
    let fixture = Fixture::new("lane-session-resume-refusals");
    configure_fake_vendor(&fixture);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let added = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &["lane", "add", "--vendor", "claude"],
        true,
        None,
    ));
    let drained = invoke_args(
        &fixture,
        &["lane", "drain", &added.lane.to_string()],
        true,
        None,
    );
    assert!(
        drained.status.success(),
        "drain failed: {:?}",
        drained.stderr
    );

    let stopped = fixture.root.join("mismatched-vendor-stopped");
    let vendor_pid = fixture.root.join("mismatched-vendor-pid");
    configure_vendor(
        &fixture,
        &format!(
            concat!(
                "#!/bin/sh\n",
                "trap 'printf stopped > {}; exit 0' TERM INT\n",
                "printf '%s\\n' \"$$\" > {}\n",
                "printf '%s\\n' {}\n",
                "while :; do sleep 1; done\n",
            ),
            shell_word(&stopped.display().to_string()),
            shell_word(&vendor_pid.display().to_string()),
            shell_word(MISMATCHED_SESSION_RECORD),
        ),
    );
    let mismatch = invoke_args(
        &fixture,
        &["lane", "wake", &added.lane.to_string()],
        true,
        None,
    );
    assert_eq!(refusal_reason(&mismatch), "lane::session-mismatched");
    // The refusal returns without waiting for the vendor's TERM trap, so the
    // stop is observed rather than read at the return.
    let stop = wait_for_vendor_stop(&fixture.cleanup, &vendor_pid, &stopped);
    assert_eq!(stop, "stopped", "mismatched vendor was terminated");
    let vacant =
        reply_body::<wire::intent::LaneAnswer>(&invoke_args(&fixture, &["lane"], true, None));
    assert_eq!(
        (
            vacant.lanes[0].occupant,
            vacant.lanes[0].lineage.len(),
            vacant.lanes[0].bound,
        ),
        (None, 1, true)
    );

    let tape = fixture
        .root
        .join(".git/.spice/tape")
        .join(added.lane.to_string())
        .join("claude");
    fs::rename(&tape, tape.with_extension("moved")).expect("fixture tape moves");
    let moved = invoke_args(
        &fixture,
        &["lane", "wake", &added.lane.to_string()],
        true,
        None,
    );
    assert_eq!(refusal_reason(&moved), "lane::session-tape-moved");
    let still_vacant =
        reply_body::<wire::intent::LaneAnswer>(&invoke_args(&fixture, &["lane"], true, None));
    assert_eq!(
        (
            still_vacant.lanes[0].occupant,
            still_vacant.lanes[0].lineage.len()
        ),
        (None, 1)
    );

    let down = invoke(&fixture, "down");
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}
