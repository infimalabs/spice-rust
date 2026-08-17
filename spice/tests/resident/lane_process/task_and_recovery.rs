use super::*;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

struct RecoveryProcessFixture {
    fixture: Fixture,
    resident: RunningResident,
    lane: wire::CanonicalUuid,
    task: wire::intent::TaskAnswer,
}

impl RecoveryProcessFixture {
    fn new(name: &str) -> Self {
        let fixture = Fixture::new(name);
        configure_fake_vendor(&fixture);
        let mut config = fs::read_to_string(fixture.root.join("spice-rust.toml"))
            .expect("negative recovery fixture configuration reads");
        config.push_str("\n[tasks]\nstems = [\"spice\"]\n");
        fs::write(fixture.root.join("spice-rust.toml"), config)
            .expect("negative recovery fixture project policy writes");
        initialize_task_git_fixture(&fixture);
        fs::write(fixture.root.join("conflict.txt"), "baseline\n")
            .expect("conflict baseline writes");
        fixture_git(&fixture, &["add", "conflict.txt"]);
        fixture_git(&fixture, &["commit", "-m", "negative recovery baseline"]);
        establish_task_baseline(&fixture);
        let mut resident = RunningResident::spawn(&fixture);
        resident.assert_ready(&fixture);
        let seated = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
            &fixture,
            &["lane", "add", "--life", "Drain", "--vendor", "claude"],
            true,
            None,
        ));
        let task = add_next_action_task(&fixture, seated.lane, "Retain a refused dirty handoff");
        Self {
            fixture,
            resident,
            lane: seated.lane,
            task,
        }
    }

    fn recover(&self) -> Output {
        invoke_args(
            &self.fixture,
            &["task", "recover", &self.task.task.to_string()],
            true,
            Some(self.lane),
        )
    }

    fn finish(self) {
        let Self {
            fixture,
            resident,
            lane,
            task: _,
        } = self;
        let drained = invoke_args(&fixture, &["lane", "drain", &lane.to_string()], true, None);
        assert!(
            drained.status.success(),
            "drain failed: {:?}",
            drained.stderr
        );
        let down = invoke(&fixture, "down");
        assert!(down.status.success(), "down failed: {:?}", down.stderr);
        assert!(resident.wait().status.success(), "resident failed");
    }
}

fn assert_quarantine_contains(fixture: &Fixture, expected: &[u8]) {
    let objects = fixture.root.join(".git/spice-task-recovery/objects");
    assert!(
        fs::read_dir(&objects)
            .expect("retained recovery objects enumerate")
            .map(
                |entry| fs::read(entry.expect("recovery object entry reads").path())
                    .expect("recovery object reads")
            )
            .any(|bytes| bytes == expected),
        "the retained quarantine no longer contains the refused handoff bytes",
    );
}

fn assert_recovery_claim_was_committed(fixture: &Fixture) {
    let snapshot = journal::JournalReader::open(fixture.journal())
        .expect("refused recovery journal reads")
        .fact_snapshot()
        .expect("refused recovery facts read");
    assert!(snapshot.facts().iter().any(|fact| {
        fact.fact.kind() == FactKind::ClaimWon && fact.fact.body()["recovery"].as_str().is_some()
    }));
}

fn assert_actionable_quarantine_refusal(output: &Output, expected: &str) {
    assert_eq!(output.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) = outcome(output) else {
        panic!("retained quarantine must answer with a refusal");
    };
    assert_eq!(refusal.reason(), expected);
    assert!(
        refusal.explanation().contains("quarantine"),
        "retained recovery refusal has no actionable quarantine guidance: {refusal:?}",
    );
}

#[test]
fn explicit_recovery_process_refuses_unsupported_trees_preclaim() {
    let fifo = RecoveryProcessFixture::new("task-recover-fifo");
    let fifo_path = fifo.fixture.root.join("handoff.fifo");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo_path)
            .status()
            .expect("mkfifo launches")
            .success(),
    );
    fs::write(fifo.fixture.root.join("ordinary.txt"), b"ordinary\n")
        .expect("ordinary handoff writes");
    assert_eq!(refusal_reason(&fifo.recover()), "git::recovery-file-kind");
    assert!(fifo_path.exists());
    assert_eq!(
        fs::read(fifo.fixture.root.join("ordinary.txt")).expect("ordinary bytes read"),
        b"ordinary\n",
    );
    assert!(!fifo.fixture.root.join(".git/spice-task-recovery").exists());
    fifo.finish();

    let nested = RecoveryProcessFixture::new("task-recover-nested");
    let nested_root = nested.fixture.root.join("nested");
    fixture_git(
        &nested.fixture,
        &["init", nested_root.to_str().expect("UTF-8 nested path")],
    );
    fs::write(nested_root.join("payload.txt"), b"nested bytes\n").expect("nested handoff writes");
    assert_eq!(
        refusal_reason(&nested.recover()),
        "git::recovery-nested-repository",
    );
    assert_eq!(
        fs::read(nested_root.join("payload.txt")).expect("nested bytes read"),
        b"nested bytes\n",
    );
    assert!(
        !nested
            .fixture
            .root
            .join(".git/spice-task-recovery")
            .exists()
    );
    nested.finish();
}

#[test]
fn explicit_recovery_process_retains_a_quarantine_when_another_client_wins_the_claim_race() {
    let raced = RecoveryProcessFixture::new("task-recover-claim-race");
    let control = raced.fixture.root.join(".git/recovery-race-control");
    let first = control.join("first-fetch");
    let once = control.join("first-fetch-owner");
    let release = control.join("release");
    let upload_pack = control.join("upload-pack");
    fs::create_dir(&control).expect("claim-race control directory creates");
    assert!(
        Command::new("mkfifo")
            .arg(&release)
            .status()
            .expect("claim-race release FIFO creates")
            .success(),
    );
    fs::write(
        &upload_pack,
        format!(
            concat!(
                "#!/bin/sh\n",
                "set -eu\n",
                "if [ ! -d {once} ]; then\n",
                "  mkdir {once}\n",
                "  printf first > {first}\n",
                "  IFS= read -r released < {release}\n",
                "fi\n",
                "exec git-upload-pack \"$@\"\n",
            ),
            once = shell_word(&once.display().to_string()),
            first = shell_word(&first.display().to_string()),
            release = shell_word(&release.display().to_string()),
        ),
    )
    .expect("claim-race upload-pack wrapper writes");
    fs::set_permissions(&upload_pack, fs::Permissions::from_mode(0o700))
        .expect("claim-race upload-pack wrapper becomes executable");
    fixture_git(
        &raced.fixture,
        &[
            "config",
            "remote.origin.uploadpack",
            upload_pack.to_str().expect("UTF-8 upload-pack path"),
        ],
    );
    fs::write(
        raced.fixture.root.join("raced-handoff.txt"),
        b"do not lose\n",
    )
    .expect("claim-race handoff writes");

    let mut interrupted = None;
    wait_for_event(
        &control,
        || {
            let child = Command::new(env!("CARGO_BIN_EXE_spice"))
                .arg("--json")
                .args(["task", "recover", &raced.task.task.to_string()])
                .current_dir(&raced.fixture.root)
                .env(steer::hook::LANE, raced.lane.to_string())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("interrupted recovery client launches");
            interrupted = Some(child);
        },
        || first.is_file().then_some(()),
    );
    let winner = invoke_args(&raced.fixture, &["task", "next"], true, Some(raced.lane));
    let winner = reply_body::<wire::intent::ClaimAnswer>(&winner);
    assert_eq!(winner.task, raced.task.task);
    fs::write(&release, b"release\n").expect("release interrupted recovery fetch");
    let interrupted = interrupted
        .expect("recovery client was retained")
        .wait_with_output()
        .expect("interrupted recovery client exits");

    assert_actionable_quarantine_refusal(&interrupted, "git::recovery-board-ambiguous");
    assert_quarantine_contains(&raced.fixture, b"do not lose\n");
    assert!(
        !raced.fixture.root.join("raced-handoff.txt").exists(),
        "an ambiguous client replayed bytes under the winning ordinary claim",
    );
    raced.finish();
}

#[test]
fn explicit_recovery_process_retains_postclaim_collision_and_conflict() {
    let collision = RecoveryProcessFixture::new("task-recover-collision");
    fs::write(
        collision.fixture.root.join("collision.txt"),
        b"handoff collision\n",
    )
    .expect("collision handoff writes");
    peer_commit(&collision.fixture, "collision.txt", "baseline collision\n");
    let refused = collision.recover();
    assert_actionable_quarantine_refusal(&refused, "git::recovery-untracked-collision");
    assert_eq!(
        fs::read(collision.fixture.root.join("collision.txt")).expect("baseline collision reads"),
        b"baseline collision\n",
    );
    assert_quarantine_contains(&collision.fixture, b"handoff collision\n");
    assert_recovery_claim_was_committed(&collision.fixture);
    let retried = collision.recover();
    assert_actionable_quarantine_refusal(&retried, "git::recovery-untracked-collision");
    assert_quarantine_contains(&collision.fixture, b"handoff collision\n");
    collision.finish();

    let conflict = RecoveryProcessFixture::new("task-recover-conflict");
    fs::write(
        conflict.fixture.root.join("conflict.txt"),
        b"handoff conflict\n",
    )
    .expect("conflict handoff writes");
    fixture_git(&conflict.fixture, &["add", "conflict.txt"]);
    peer_commit(&conflict.fixture, "conflict.txt", "baseline conflict\n");
    let refused = conflict.recover();
    assert_actionable_quarantine_refusal(&refused, "git::recovery-replay-conflict");
    assert!(
        git_stdout(&conflict.fixture.root, &["status", "--short"])
            .lines()
            .any(|line| line == "UU conflict.txt"),
        "three-way replay leaves complete conflict stages",
    );
    assert_eq!(
        git_stdout(
            &conflict.fixture.root,
            &["ls-files", "--unmerged", "conflict.txt"],
        )
        .lines()
        .count(),
        3,
    );
    assert_quarantine_contains(&conflict.fixture, b"handoff conflict\n");
    assert_recovery_claim_was_committed(&conflict.fixture);
    let retried = conflict.recover();
    assert_actionable_quarantine_refusal(&retried, "git::recovery-conflict-retained");
    assert_quarantine_contains(&conflict.fixture, b"handoff conflict\n");
    conflict.finish();
}

struct DirtyRecoveryFingerprint {
    status: String,
    index: String,
    staged: Vec<u8>,
}

fn initialize_dirty_recovery_fixture(fixture: &Fixture) {
    configure_fake_vendor(fixture);
    let mut config = fs::read_to_string(fixture.root.join("spice-rust.toml"))
        .expect("recovery fixture configuration reads");
    config.push_str("\n[tasks]\nstems = [\"spice\"]\n");
    fs::write(fixture.root.join("spice-rust.toml"), config)
        .expect("recovery fixture project policy writes");
    initialize_task_git_fixture(fixture);
    fs::write(fixture.root.join("layered.bin"), b"base\0layer\n").expect("layered baseline writes");
    fs::write(fixture.root.join("deleted.txt"), b"delete me\n").expect("deleted baseline writes");
    fs::write(fixture.root.join("script.sh"), b"#!/bin/sh\nexit 0\n")
        .expect("executable baseline writes");
    fixture_git(fixture, &["add", "layered.bin", "deleted.txt", "script.sh"]);
    fixture_git(fixture, &["commit", "-m", "recovery fixture baseline"]);
    establish_task_baseline(fixture);
}

fn write_dirty_recovery_generation(fixture: &Fixture) -> DirtyRecoveryFingerprint {
    fs::write(fixture.root.join("layered.bin"), b"staged\0generation\n")
        .expect("staged generation writes");
    fixture_git(fixture, &["add", "layered.bin"]);
    fs::write(fixture.root.join("layered.bin"), b"unstaged\0generation\n")
        .expect("unstaged generation writes");
    fs::remove_file(fixture.root.join("deleted.txt")).expect("tracked deletion writes");
    fs::set_permissions(
        fixture.root.join("script.sh"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("executable mode writes");
    fixture_git(fixture, &["add", "script.sh"]);
    fs::write(fixture.root.join("untracked.bin"), [0, 1, 2, 0xfe, 0xff])
        .expect("untracked bytes write");
    fs::set_permissions(
        fixture.root.join("untracked.bin"),
        fs::Permissions::from_mode(0o751),
    )
    .expect("untracked mode writes");
    let staged = Command::new("git")
        .args([
            "-C",
            fixture.root.to_str().expect("UTF-8 fixture root"),
            "show",
            ":layered.bin",
        ])
        .output()
        .expect("staged bytes read before recovery");
    assert!(staged.status.success(), "staged bytes are readable");
    DirtyRecoveryFingerprint {
        status: git_stdout(&fixture.root, &["status", "--porcelain=v1"]),
        index: dirty_recovery_index(fixture),
        staged: staged.stdout,
    }
}

fn dirty_recovery_index(fixture: &Fixture) -> String {
    git_stdout(
        &fixture.root,
        &[
            "ls-files",
            "--stage",
            "--",
            "layered.bin",
            "script.sh",
            "deleted.txt",
        ],
    )
}

fn assert_replayed_dirty_generation(fixture: &Fixture, fingerprint: &DirtyRecoveryFingerprint) {
    assert_eq!(
        git_stdout(&fixture.root, &["status", "--porcelain=v1"]),
        fingerprint.status,
        "replay restores the exact staged/unstaged/deleted/untracked footprint",
    );
    assert_eq!(
        dirty_recovery_index(fixture),
        fingerprint.index,
        "replay restores exact blob identities, executable bits, and index stages",
    );
    let replayed_stage = Command::new("git")
        .args([
            "-C",
            fixture.root.to_str().expect("UTF-8 fixture root"),
            "show",
            ":layered.bin",
        ])
        .output()
        .expect("replayed staged bytes read");
    assert!(
        replayed_stage.status.success(),
        "replayed stage is readable"
    );
    assert_eq!(replayed_stage.stdout, fingerprint.staged);
    assert_eq!(
        fs::read(fixture.root.join("layered.bin")).expect("replayed worktree bytes read"),
        b"unstaged\0generation\n",
    );
    assert!(!fixture.root.join("deleted.txt").exists());
    assert_eq!(
        fs::metadata(fixture.root.join("script.sh"))
            .expect("replayed executable metadata reads")
            .permissions()
            .mode()
            & 0o111,
        0o111,
    );
    assert_eq!(
        fs::read(fixture.root.join("untracked.bin")).expect("replayed untracked bytes read"),
        [0, 1, 2, 0xfe, 0xff],
    );
    assert_eq!(
        fs::metadata(fixture.root.join("untracked.bin"))
            .expect("replayed untracked metadata reads")
            .permissions()
            .mode()
            & 0o777,
        0o751,
    );
}

fn assert_recovery_claim_boundary(
    fixture: &Fixture,
    task: &wire::intent::TaskAnswer,
    synchronized: &str,
) {
    assert!(
        spice::task_git::pending_recovery(
            &spice::repository::Repository::discover(&fixture.root)
                .expect("replayed repository resolves"),
        )
        .expect("retired recovery inspects")
        .is_none(),
        "successful replay retires its private quarantine",
    );
    let facts = journal::JournalReader::open(fixture.journal())
        .expect("recovery journal reads")
        .fact_snapshot()
        .expect("recovery facts read");
    let won = facts
        .facts()
        .iter()
        .find(|fact| {
            fact.fact.kind() == FactKind::ClaimWon
                && fact.fact.body()["recovery"].as_str().is_some()
        })
        .expect("explicit recovery journals one recovery claim");
    assert_eq!(
        won.fact.body()["task"],
        serde_json::json!(task.task.born().unwrap())
    );
    assert_eq!(won.fact.body()["git"]["head"], synchronized);
    assert_eq!(won.fact.body()["git"]["integrated"], synchronized);
    assert_eq!(won.fact.body()["git"]["upstream"], "origin/main");
    assert_eq!(won.fact.body()["git"]["upstream_head"], synchronized);
    assert_eq!(won.fact.body()["git"]["local_commits"], 0);
    assert!(
        spice::task_git::Recovery::is_transaction(
            won.fact.body()["recovery"]
                .as_str()
                .expect("recovery claim names its transaction"),
        ),
        "claim journals the exact content-addressed recovery identity",
    );
    drop(facts);
}

#[test]
fn explicit_dirty_handoff_recovery_synchronizes_claim_and_replays_exactly() {
    let fixture = Fixture::new("task-recover-dirty-handoff");
    initialize_dirty_recovery_fixture(&fixture);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let seated = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &["lane", "add", "--life", "Drain", "--vendor", "claude"],
        true,
        None,
    ));
    let task = add_next_action_task(&fixture, seated.lane, "Recover a byte-exact dirty handoff");
    let synchronized = peer_commit(&fixture, "upstream.txt", "synchronized baseline\n");
    let fingerprint = write_dirty_recovery_generation(&fixture);

    let recovered = invoke_args(
        &fixture,
        &["task", "recover", &task.task.to_string()],
        true,
        Some(seated.lane),
    );
    let answer = reply_body::<wire::intent::ClaimAnswer>(&recovered);
    assert_eq!(
        (answer.task, answer.holder),
        (task.task.clone(), Actor::Lane(seated.lane)),
        "the explicit process command claims only its named handoff task",
    );
    assert_eq!(
        git_stdout(&fixture.root, &["rev-parse", "HEAD"]),
        synchronized,
    );
    assert_replayed_dirty_generation(&fixture, &fingerprint);
    assert_recovery_claim_boundary(&fixture, &task, &synchronized);

    let drained = invoke_args(
        &fixture,
        &["lane", "drain", &seated.lane.to_string()],
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
    let mut config = fs::read_to_string(fixture.root.join("spice-rust.toml"))
        .expect("task fixture configuration reads");
    config.push_str("\n[tasks]\nstems = [\"spice\"]\n");
    fs::write(fixture.root.join("spice-rust.toml"), config)
        .expect("task fixture project policy writes");
    initialize_task_git_fixture(&fixture);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);

    let added_lane = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &["lane", "add", "--life", "Drive", "--vendor", "claude"],
        true,
        None,
    ));
    let reviewed = finish_noted_task(&fixture, added_lane.lane);
    assert_oops_has_no_next(&fixture, added_lane.lane);
    finish_dependent_task(&fixture, added_lane.lane, &reviewed.task);

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
fn kernel_exit_callback_records_and_vacates_an_occupant() {
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
    wait_for_vendor_exit(&fixture.root, &exited);
    wait_for_lane_exit(&fixture, added.lane);
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
    wait_for_vendor_exit(&fixture.root, &exited);
    wait_for_lane_exit(&fixture, added.lane);

    let tape_root = fixture
        .root
        .join(".git/.spice-rs/tape")
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
        fixture.root.join(".git/.spice-rs/tape"),
        listen::RetroSelector::Lane(added.lane.to_string()),
        &[
            ("claude", listen::Dialect::Claude),
            ("codex", listen::Dialect::Codex),
        ],
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
        let announced = if duplicate {
            format!(
                concat!("printf '%s\\n' {}\n", "printf '%s\\n' {}\n"),
                shell_word(SESSION_RECORD),
                shell_word(SESSION_RECORD),
            )
        } else {
            String::new()
        };
        let script =
            vendor_prologue(&stopped, &vendor_pid) + &announced + "while :; do sleep 1; done\n";
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
        &(vendor_prologue(&stopped, &vendor_pid)
            + &format!(
                concat!("printf '%s\\n' {}\n", "while :; do sleep 1; done\n"),
                shell_word(MISMATCHED_SESSION_RECORD),
            )),
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
        .join(".git/.spice-rs/tape")
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
