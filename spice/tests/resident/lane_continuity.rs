use super::*;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

const CLAUDE_SESSION_ID: &str = "018f7777-7777-7444-8888-999999999999";
const CODEX_SESSION_ID: &str = "019fbc32-beb1-7ee0-b814-a7513f8f8ea5";
const CLAUDE_SESSION_RECORD: &str =
    r#"{"type":"system","subtype":"init","session_id":"018f7777-7777-7444-8888-999999999999"}"#;
const CODEX_SESSION_RECORD: &str =
    r#"{"type":"thread.started","thread_id":"019fbc32-beb1-7ee0-b814-a7513f8f8ea5"}"#;
const COMPACTION_RECORD: &str = r#"{"type":"system","subtype":"compact_boundary"}"#;
const CLAUDE_DIAGNOSTIC: &str = "opaque-claude-continuity-diagnostic";
const CODEX_DIAGNOSTIC: &str = "opaque-codex-continuity-diagnostic";
const EXPECTED_RECALL_LIMIT: usize = 16;

struct VendorFixture {
    codex_worktree: PathBuf,
    claude_control: PathBuf,
    codex_control: PathBuf,
    claude_pids: PathBuf,
    codex_pids: PathBuf,
    stopped: PathBuf,
    launches: PathBuf,
}

#[derive(Clone, Copy)]
struct LanePair {
    claude: wire::CanonicalUuid,
    codex: wire::CanonicalUuid,
}

struct Baseline {
    lanes: LanePair,
    occupants: [wire::CanonicalUuid; 2],
    loci: [wire::TapeLocus; 2],
    capabilities: [wire::CanonicalUuid; 2],
    pids: [u32; 2],
}

struct Directives {
    acked: wire::intent::DirectiveAnswer,
    declined: wire::intent::DirectiveAnswer,
    canceled: wire::intent::DirectiveAnswer,
    claude_pending: wire::intent::DirectiveAnswer,
    codex_pending: wire::intent::DirectiveAnswer,
}

impl Drop for VendorFixture {
    fn drop(&mut self) {
        let stopped = fs::read_to_string(&self.stopped).unwrap_or_default();
        for path in [&self.claude_pids, &self.codex_pids] {
            let Ok(pids) = fs::read_to_string(path) else {
                continue;
            };
            for pid in pids.lines() {
                if stopped.lines().any(|line| line == pid) {
                    continue;
                }
                let _ = Command::new("kill").args(["-TERM", pid]).output();
            }
        }
    }
}

impl RunningResident {
    fn crash(mut self) -> Output {
        let mut child = self.child.take().expect("resident child");
        child.kill().expect("resident crash signal");
        child
            .wait_with_output()
            .expect("crashed resident is reaped")
    }
}

#[test]
fn supervised_claude_and_codex_continuity_stays_lane_bound_across_restart() {
    let fixture = Fixture::linked("multi-lane-continuity");
    let vendors = configure_vendors(&fixture);
    let (resident, lanes) = launch_lanes(&fixture, &vendors);
    let baseline = capture_baseline(&fixture, &vendors, lanes);

    let directives = exercise_before_restart(&fixture, &vendors, lanes);

    let replacement = restart_resident(&fixture, &vendors, resident, &baseline);
    assert_restarted_rows(&fixture, &baseline);
    assert_resumed_tapes_and_launches(&fixture, &vendors, &baseline, &directives);
    assert_post_restart_surfaces(&fixture, &baseline, &directives);

    finish_and_verify(&fixture, &vendors, replacement, lanes);
}

fn launch_lanes(fixture: &Fixture, vendors: &VendorFixture) -> (RunningResident, LanePair) {
    let mut resident = RunningResident::spawn(fixture);
    resident.assert_ready(fixture);
    let claude = add_lane(fixture, &[]);
    let codex_worktree = vendors.codex_worktree.display().to_string();
    let codex = add_lane(fixture, &[codex_worktree.as_str(), "--vendor", "codex"]);
    assert_ne!(claude.lane, codex.lane);
    (
        resident,
        LanePair {
            claude: claude.lane,
            codex: codex.lane,
        },
    )
}

fn capture_baseline(fixture: &Fixture, vendors: &VendorFixture, lanes: LanePair) -> Baseline {
    let before = durable_head(fixture);
    let claude = row(&before, lanes.claude);
    let codex = row(&before, lanes.codex);
    assert_ne!(claude.capabilities, codex.capabilities);
    assert_session(
        fixture,
        claude,
        "claude",
        CLAUDE_SESSION_ID,
        CLAUDE_SESSION_RECORD,
    );
    assert_session(
        fixture,
        codex,
        "codex",
        CODEX_SESSION_ID,
        CODEX_SESSION_RECORD,
    );
    Baseline {
        lanes,
        occupants: [
            claude.occupant.expect("Claude occupant"),
            codex.occupant.expect("Codex occupant"),
        ],
        loci: [
            claude.session.as_ref().expect("Claude session").locus,
            codex.session.as_ref().expect("Codex session").locus,
        ],
        capabilities: [claude.capabilities[0], codex.capabilities[0]],
        pids: [
            latest_pid(&vendors.claude_pids),
            latest_pid(&vendors.codex_pids),
        ],
    }
}

fn exercise_before_restart(
    fixture: &Fixture,
    vendors: &VendorFixture,
    lanes: LanePair,
) -> Directives {
    let directives = Directives {
        acked: send(fixture, lanes.claude, "ack after compaction"),
        declined: send(fixture, lanes.claude, "decline after compaction"),
        canceled: send(fixture, lanes.claude, "cancel before restart"),
        claude_pending: send(fixture, lanes.claude, "survive restart on Claude"),
        codex_pending: send(fixture, lanes.codex, "survive restart on Codex"),
    };
    let compacted = emit_continuity_records(fixture, vendors, lanes, &directives);
    settle_directives(fixture, lanes, &directives);
    prove_pre_restart_isolation(fixture, lanes, &directives, &compacted);
    directives
}

fn emit_continuity_records(
    fixture: &Fixture,
    vendors: &VendorFixture,
    lanes: LanePair,
    directives: &Directives,
) -> wire::body::LaneCompacted {
    emit_record(
        fixture,
        lanes.claude,
        "claude",
        &vendors.claude_control,
        &claude_prose("before compact"),
    );
    let compacted = emit_compaction(fixture, lanes.claude, &vendors.claude_control);
    emit_record(
        fixture,
        lanes.claude,
        "claude",
        &vendors.claude_control,
        &claude_prose(&format!("ACK {}: complete", directives.acked.key)),
    );
    emit_record(
        fixture,
        lanes.claude,
        "claude",
        &vendors.claude_control,
        &claude_prose(&format!("NACK {}: cannot comply", directives.declined.key)),
    );
    emit_record(
        fixture,
        lanes.codex,
        "codex",
        &vendors.codex_control,
        &codex_prose("Codex continuity without a fabricated compaction"),
    );
    compacted
}

fn settle_directives(fixture: &Fixture, lanes: LanePair, directives: &Directives) {
    let crossed = invoke_lane(
        fixture,
        lanes.claude,
        &["ack", &directives.codex_pending.key],
        true,
    );
    assert_refusal(&crossed, "steer::transcript-evidence-absent");
    let acked = directive_reply(&invoke_lane(
        fixture,
        lanes.claude,
        &["ack", &directives.acked.key],
        true,
    ));
    assert_eq!(acked.disposition, wire::Disposition::Acked);
    let declined = directive_reply(&invoke_lane(
        fixture,
        lanes.claude,
        &["ack", &directives.declined.key, "--decline"],
        true,
    ));
    assert_eq!(declined.disposition, wire::Disposition::Declined);
    let canceled = directive_reply(&invoke_operator(
        fixture,
        &[
            "steer",
            "cancel",
            &directives.canceled.directive.to_string(),
        ],
        true,
    ));
    assert_eq!(canceled.disposition, wire::Disposition::Canceled);
}

fn prove_pre_restart_isolation(
    fixture: &Fixture,
    lanes: LanePair,
    directives: &Directives,
    compacted: &wire::body::LaneCompacted,
) {
    let bytes = tape_bytes(fixture, lanes.claude, "claude", compacted_locus(compacted));
    assert_eq!(bytes, format!("{COMPACTION_RECORD}\n").as_bytes());
    assert_no_codex_compaction(fixture, lanes.codex);
    assert_pending_isolated(
        fixture,
        lanes.claude,
        &directives.claude_pending.key,
        &directives.codex_pending.key,
    );
    assert_pending_isolated(
        fixture,
        lanes.codex,
        &directives.codex_pending.key,
        &directives.claude_pending.key,
    );
}

fn restart_resident(
    fixture: &Fixture,
    vendors: &VendorFixture,
    resident: RunningResident,
    baseline: &Baseline,
) -> RunningResident {
    let crashed = resident.crash();
    assert!(
        !crashed.status.success(),
        "fixture resident must crash abruptly"
    );
    stop_vendor_processes(fixture, &vendors.stopped, &baseline.pids);
    fs::remove_file(fixture.canvas()).expect("stale crashed discovery is removed");
    let mut replacement = RunningResident::spawn(fixture);
    replacement.assert_ready(fixture);
    wait_for_event(
        &fixture.runtime.join("data"),
        || {},
        || {
            let renewals = journal::JournalReader::open(fixture.journal())
                .ok()?
                .fact_snapshot()
                .ok()?
                .facts()
                .iter()
                .filter(|fact| fact.fact.kind() == FactKind::LaneRenewed)
                .count();
            (renewals >= 2).then_some(())
        },
    );
    replacement
}

fn assert_restarted_rows(fixture: &Fixture, baseline: &Baseline) {
    let after = durable_head(fixture);
    let claude = row(&after, baseline.lanes.claude);
    let codex = row(&after, baseline.lanes.codex);
    assert_eq!(claude.capabilities, [baseline.capabilities[0]]);
    assert_eq!(codex.capabilities, [baseline.capabilities[1]]);
    assert_ne!(claude.occupant, Some(baseline.occupants[0]));
    assert_ne!(codex.occupant, Some(baseline.occupants[1]));
    assert_eq!(claude.lineage.len(), 2);
    assert_eq!(codex.lineage.len(), 2);
    assert_session(
        fixture,
        claude,
        "claude",
        CLAUDE_SESSION_ID,
        CLAUDE_SESSION_RECORD,
    );
    assert_session(
        fixture,
        codex,
        "codex",
        CODEX_SESSION_ID,
        CODEX_SESSION_RECORD,
    );
    assert_ne!(
        claude.session.as_ref().expect("Claude session").locus,
        baseline.loci[0]
    );
    assert_ne!(
        codex.session.as_ref().expect("Codex session").locus,
        baseline.loci[1]
    );
}

fn assert_resumed_tapes_and_launches(
    fixture: &Fixture,
    vendors: &VendorFixture,
    baseline: &Baseline,
    directives: &Directives,
) {
    let claude = String::from_utf8(tape_all(&tape_path(
        fixture,
        baseline.lanes.claude,
        "claude",
    )))
    .expect("Claude tape is UTF-8 JSONL");
    let codex = String::from_utf8(tape_all(&tape_path(fixture, baseline.lanes.codex, "codex")))
        .expect("Codex tape is UTF-8 JSONL");
    assert!(!claude.contains(CODEX_SESSION_ID));
    assert!(!claude.contains(&directives.codex_pending.key));
    assert!(!codex.contains(CLAUDE_SESSION_ID));
    assert!(!codex.contains(&directives.claude_pending.key));
    let launches = fs::read_to_string(&vendors.launches).expect("launch modes read");
    for expected in [
        "claude:fresh",
        "codex:fresh",
        "claude:resume",
        "codex:resume",
    ] {
        assert_eq!(
            launches.lines().filter(|line| *line == expected).count(),
            1,
            "unexpected launch modes: {launches:?}",
        );
    }
}

fn assert_post_restart_surfaces(fixture: &Fixture, baseline: &Baseline, directives: &Directives) {
    assert_pending_isolated(
        fixture,
        baseline.lanes.claude,
        &directives.claude_pending.key,
        &directives.codex_pending.key,
    );
    assert_pending_isolated(
        fixture,
        baseline.lanes.codex,
        &directives.codex_pending.key,
        &directives.claude_pending.key,
    );
    assert_recall(
        fixture,
        baseline.lanes.claude,
        CLAUDE_SESSION_ID,
        &directives.claude_pending.key,
        baseline.capabilities[0],
        Some((
            &directives.acked.key,
            &directives.declined.key,
            &directives.canceled.key,
        )),
    );
    assert_recall(
        fixture,
        baseline.lanes.codex,
        CODEX_SESSION_ID,
        &directives.codex_pending.key,
        baseline.capabilities[1],
        None,
    );
}

fn finish_and_verify(
    fixture: &Fixture,
    vendors: &VendorFixture,
    replacement: RunningResident,
    lanes: LanePair,
) {
    let resumed_pids = [
        latest_pid(&vendors.claude_pids),
        latest_pid(&vendors.codex_pids),
    ];
    let crashed = replacement.crash();
    assert!(
        !crashed.status.success(),
        "replacement fixture must crash abruptly"
    );
    stop_vendor_processes(fixture, &vendors.stopped, &resumed_pids);
    assert_no_codex_compaction(fixture, lanes.codex);
    journal::Journal::open(fixture.journal())
        .expect("continuity journal opens")
        .verify()
        .expect("continuity authority verifies");
}

fn configure_vendors(fixture: &Fixture) -> VendorFixture {
    let common = fixture
        .runtime
        .parent()
        .expect("runtime has common Git parent");
    let codex_worktree = fixture.cleanup.join("codex-worktree");
    let codex_git = common.join("worktrees/codex-continuity");
    fs::create_dir_all(&codex_worktree).expect("Codex worktree directory");
    fs::create_dir_all(&codex_git).expect("Codex Git directory");
    fs::write(
        codex_worktree.join(".git"),
        format!("gitdir: {}\n", codex_git.display()),
    )
    .expect("Codex Git pointer");
    fs::write(codex_git.join("commondir"), "../..\n").expect("Codex common pointer");

    let claude_control = fixture.cleanup.join("claude-control");
    let codex_control = fixture.cleanup.join("codex-control");
    for control in [&claude_control, &codex_control] {
        let made = Command::new("mkfifo")
            .arg(control)
            .output()
            .expect("mkfifo launches");
        assert!(made.status.success(), "mkfifo failed: {:?}", made.stderr);
    }
    let claude_pids = fixture.cleanup.join("claude-pids");
    let codex_pids = fixture.cleanup.join("codex-pids");
    let stopped = fixture.cleanup.join("vendor-stopped");
    let launches = fixture.cleanup.join("vendor-launches");
    let claude_binary = fixture.cleanup.join("fake-claude");
    let codex_binary = fixture.cleanup.join("fake-codex");
    write_vendor(
        &claude_binary,
        &claude_control,
        &claude_pids,
        &stopped,
        &launches,
        "claude",
        &format!(" --resume {CLAUDE_SESSION_ID} "),
        CLAUDE_SESSION_RECORD,
        CLAUDE_DIAGNOSTIC,
    );
    write_vendor(
        &codex_binary,
        &codex_control,
        &codex_pids,
        &stopped,
        &launches,
        "codex",
        &format!(" resume {CODEX_SESSION_ID} "),
        CODEX_SESSION_RECORD,
        CODEX_DIAGNOSTIC,
    );
    let config = format!(
        concat!(
            "[lane]\nlease = \"30s\"\n\n",
            "[vendor.claude]\nbinary = {:?}\n\n",
            "[vendor.codex]\nbinary = {:?}\n",
        ),
        claude_binary.display().to_string(),
        codex_binary.display().to_string(),
    );
    fs::write(fixture.root.join("spice.toml"), &config).expect("Claude worktree config");
    fs::write(codex_worktree.join("spice.toml"), config).expect("Codex worktree config");
    VendorFixture {
        codex_worktree,
        claude_control,
        codex_control,
        claude_pids,
        codex_pids,
        stopped,
        launches,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn write_vendor(
    binary: &Path,
    control: &Path,
    pids: &Path,
    stopped: &Path,
    launches: &Path,
    vendor: &str,
    resume_match: &str,
    session: &str,
    diagnostic: &str,
) {
    fs::write(
        binary,
        format!(
            concat!(
                "#!/bin/sh\n",
                "CONTROL={}\nPIDS={}\nSTOPPED={}\nLAUNCHES={}\n",
                "trap 'printf \"%s\\n\" \"$$\" >> \"$STOPPED\"; exit 0' TERM INT\n",
                "printf '%s\\n' \"$$\" >> \"$PIDS\"\n",
                "case \" $* \" in *{}*) printf '%s:resume\\n' {} >> \"$LAUNCHES\" ;; ",
                "*) printf '%s:fresh\\n' {} >> \"$LAUNCHES\" ;; esac\n",
                "exec 3<>\"$CONTROL\"\n",
                "printf '%s\\n' {}\n",
                "printf '%s\\n' {} >&2\n",
                "while IFS= read -r record <&3; do printf '%s\\n' \"$record\"; done\n",
            ),
            shell_word(&control.display().to_string()),
            shell_word(&pids.display().to_string()),
            shell_word(&stopped.display().to_string()),
            shell_word(&launches.display().to_string()),
            shell_word(resume_match),
            shell_word(vendor),
            shell_word(vendor),
            shell_word(session),
            shell_word(diagnostic),
        ),
    )
    .expect("fake vendor writes");
    #[cfg(unix)]
    fs::set_permissions(binary, fs::Permissions::from_mode(0o700))
        .expect("fake vendor becomes executable");
}

pub(super) fn add_lane(fixture: &Fixture, tail: &[&str]) -> wire::intent::LaneChangeAnswer {
    let mut args = vec!["lane", "add"];
    args.extend_from_slice(tail);
    reply_body(&invoke_operator(fixture, &args, true))
}

fn send(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    message: &str,
) -> wire::intent::DirectiveAnswer {
    reply_body(&invoke_operator(
        fixture,
        &["steer", &lane.to_string(), message],
        true,
    ))
}

fn invoke_operator(fixture: &Fixture, arguments: &[&str], json: bool) -> Output {
    let mut process = Command::new(env!("CARGO_BIN_EXE_spice"));
    process.env_remove(steer::hook::LANE);
    if json {
        process.arg("--json");
    }
    process
        .args(arguments)
        .current_dir(&fixture.root)
        .output()
        .expect("operator spice launches")
}

fn invoke_lane(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    arguments: &[&str],
    json: bool,
) -> Output {
    let mut process = Command::new(env!("CARGO_BIN_EXE_spice"));
    if json {
        process.arg("--json");
    }
    process
        .args(arguments)
        .env(steer::hook::LANE, lane.to_string())
        .current_dir(&fixture.root)
        .output()
        .expect("lane spice launches")
}

#[track_caller]
fn reply_body<T: serde::de::DeserializeOwned>(output: &Output) -> T {
    assert!(
        output.status.success(),
        "command failed: stdout={:?} stderr={:?}",
        output.stdout,
        output.stderr
    );
    let Outcome::Reply(reply) = value_outcome(output) else {
        panic!("successful command must reply")
    };
    let bytes = wire::encode(&reply.body).expect("reply value encodes");
    wire::decode(&bytes).expect("reply body decodes")
}

fn directive_reply(output: &Output) -> wire::intent::DirectiveAnswer {
    reply_body(output)
}

fn assert_refusal(output: &Output, expected: &str) {
    assert_eq!(output.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) = value_outcome(output) else {
        panic!("command must refuse")
    };
    assert_eq!(refusal.reason, expected);
}

pub(super) fn durable_head(fixture: &Fixture) -> wire::ResidencyHead {
    journal::JournalReader::open(fixture.journal())
        .expect("continuity journal reads")
        .read(|read| read.residency_head())
        .expect("residency reads")
        .expect("residency exists")
        .into_body()
}

fn row(head: &wire::ResidencyHead, lane: wire::CanonicalUuid) -> &wire::LaneResidency {
    head.lanes
        .iter()
        .find(|row| row.lane == lane)
        .expect("lane row exists")
}

fn assert_session(
    fixture: &Fixture,
    row: &wire::LaneResidency,
    vendor: &str,
    identity: &str,
    record: &str,
) {
    let session = row.session.as_ref().expect("session authority exists");
    assert_eq!(session.lane, row.lane);
    assert_eq!(session.occupant, row.occupant.expect("occupied lane"));
    assert_eq!(session.vendor.as_str(), vendor);
    assert_eq!(session.identity, identity);
    assert_eq!(session.stream, "stdout");
    assert_eq!(
        tape_bytes(fixture, row.lane, vendor, session.locus),
        format!("{record}\n").as_bytes()
    );
}

fn claude_prose(text: &str) -> String {
    serde_json::json!({
        "type": "assistant",
        "message": {
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "usage": {
                "input_tokens": 1,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
                "output_tokens": 1
            }
        }
    })
    .to_string()
}

fn codex_prose(text: &str) -> String {
    serde_json::json!({
        "type": "item.completed",
        "item": {"id": "continuity", "type": "agent_message", "text": text}
    })
    .to_string()
}

fn emit_record(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    vendor: &str,
    control: &Path,
    record: &str,
) {
    let tape = tape_path(fixture, lane, vendor);
    let expected = format!("{record}\n").into_bytes();
    wait_for_event(
        &tape,
        || {
            fs::write(control, &expected).expect("control record writes");
        },
        || {
            tape_all(&tape)
                .windows(expected.len())
                .any(|window| window == expected)
                .then_some(())
        },
    );
}

fn emit_compaction(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    control: &Path,
) -> wire::body::LaneCompacted {
    let data = fixture.runtime.join("data");
    wait_for_event(
        &data,
        || {
            fs::write(control, format!("{COMPACTION_RECORD}\n"))
                .expect("compaction control record writes");
        },
        || {
            journal::JournalReader::open(fixture.journal())
                .ok()?
                .fact_snapshot()
                .ok()?
                .facts()
                .iter()
                .find_map(|fact| fact.fact.read::<wire::body::LaneCompacted>())
                .filter(|body| body.lane == lane)
        },
    )
}

fn compacted_locus(body: &wire::body::LaneCompacted) -> wire::TapeLocus {
    wire::TapeLocus {
        member: body.member,
        offset: body.offset,
        length: body.length,
    }
}

fn tape_path(fixture: &Fixture, lane: wire::CanonicalUuid, vendor: &str) -> PathBuf {
    fixture
        .runtime
        .join("tape")
        .join(lane.to_string())
        .join(vendor)
}

fn tape_all(path: &Path) -> Vec<u8> {
    let tape = tape::Tape::open_existing(path).expect("continuity tape opens");
    let mut bytes = Vec::new();
    for extent in tape.extents().expect("continuity extents read") {
        bytes.extend(
            tape.read(tape::Locus::new(extent.member, 0, extent.length))
                .expect("continuity extent reads"),
        );
    }
    bytes
}

fn tape_bytes(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    vendor: &str,
    locus: wire::TapeLocus,
) -> Vec<u8> {
    tape::Tape::open_existing(tape_path(fixture, lane, vendor))
        .expect("lane tape opens")
        .read(tape::Locus::new(locus.member, locus.offset, locus.length))
        .expect("lane locus resolves")
}

fn assert_no_codex_compaction(fixture: &Fixture, lane: wire::CanonicalUuid) {
    let facts = journal::JournalReader::open(fixture.journal())
        .expect("continuity journal reads")
        .fact_snapshot()
        .expect("continuity facts read");
    assert!(facts.facts().iter().all(|fact| {
        fact.fact
            .read::<wire::body::LaneCompacted>()
            .is_none_or(|body| body.lane != lane)
    }));
    let replay = listen::retro(
        fixture.runtime.join("tape"),
        listen::RetroSelector::Lane(lane.to_string()),
    )
    .expect("Codex tape replays");
    assert!(replay.observations.iter().all(|observation| !matches!(
        observation,
        listen::Observation::Event {
            event: listen::SurfaceEvent::Compaction {},
            ..
        }
    )));
}

fn assert_pending_isolated(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    expected: &str,
    foreign: &str,
) {
    let inbox: wire::intent::InboxAnswer = reply_body(&invoke_operator(
        fixture,
        &["inbox", &lane.to_string()],
        true,
    ));
    let keys = inbox.lanes[0]
        .pending
        .iter()
        .map(|directive| directive.key.as_str())
        .collect::<Vec<_>>();
    assert!(keys.contains(&expected));
    assert!(!keys.contains(&foreign));
}

fn latest_pid(path: &Path) -> u32 {
    fs::read_to_string(path)
        .expect("vendor PID log reads")
        .lines()
        .next_back()
        .expect("vendor PID exists")
        .parse()
        .expect("vendor PID is numeric")
}

fn stop_vendor_processes(fixture: &Fixture, stopped: &Path, pids: &[u32]) {
    wait_for_event(
        &fixture.cleanup,
        || {
            for pid in pids {
                let output = Command::new("kill")
                    .args(["-TERM", &pid.to_string()])
                    .output()
                    .expect("vendor termination launches");
                assert!(
                    output.status.success(),
                    "vendor kill failed: {:?}",
                    output.stderr
                );
            }
        },
        || {
            let observed = fs::read_to_string(stopped).ok()?;
            pids.iter()
                .all(|pid| observed.lines().any(|line| line == pid.to_string()))
                .then_some(())
        },
    );
}

fn assert_recall(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    session_identity: &str,
    pending_key: &str,
    capability: wire::CanonicalUuid,
    dispositions: Option<(&str, &str, &str)>,
) {
    let lane_text = lane.to_string();
    let text_output = invoke_operator(fixture, &["recall", &lane_text], false);
    assert!(
        text_output.status.success(),
        "text recall failed: {:?}",
        text_output.stderr
    );
    let text: wire::intent::RecallAnswer =
        wire::decode(&text_output.stdout).expect("text recall decodes");
    let json: wire::intent::RecallAnswer =
        reply_body(&invoke_operator(fixture, &["recall", &lane_text], true));
    assert_eq!(text, json, "text and JSON recall must be form-invariant");
    assert!(json.recent.len() <= EXPECTED_RECALL_LIMIT);
    assert!(json.recent.windows(2).all(|pair| pair[0].seq > pair[1].seq));
    assert!(
        json.recent
            .iter()
            .all(|entry| { entry.seq <= json.barrier && entry.lane == Some(lane) })
    );
    let encoded = String::from_utf8(wire::encode(&json).expect("recall encodes"))
        .expect("recall is UTF-8 JSON");
    assert!(!encoded.contains(&capability.to_string()));
    assert!(!encoded.contains(CLAUDE_DIAGNOSTIC));
    assert!(!encoded.contains(CODEX_DIAGNOSTIC));
    assert!(json.recent.iter().any(|entry| matches!(
        &entry.provenance,
        Some(wire::intent::RecallProvenance::Session { identity, .. })
            if identity == session_identity
    )));
    let vendor = row(&durable_head(fixture), lane).vendor.to_string();
    for entry in &json.recent {
        match &entry.provenance {
            Some(wire::intent::RecallProvenance::Session {
                identity, locus, ..
            }) => {
                let bytes = tape_bytes(fixture, lane, &vendor, *locus);
                assert!(String::from_utf8_lossy(&bytes).contains(identity));
            }
            Some(wire::intent::RecallProvenance::Compaction { locus, .. }) => {
                assert_eq!(
                    tape_bytes(fixture, lane, &vendor, *locus),
                    format!("{COMPACTION_RECORD}\n").as_bytes()
                );
            }
            Some(wire::intent::RecallProvenance::Steering {
                key,
                locus: Some(locus),
                ..
            }) => {
                let bytes = tape_bytes(
                    fixture,
                    lane,
                    &vendor,
                    wire::TapeLocus {
                        member: locus.member,
                        offset: locus.offset,
                        length: locus.length,
                    },
                );
                assert!(String::from_utf8_lossy(&bytes).contains(key));
            }
            Some(wire::intent::RecallProvenance::Steering { locus: None, .. })
            | Some(wire::intent::RecallProvenance::Renewal { .. })
            | None => {}
        }
    }
    assert!(json.recent.iter().any(|entry| matches!(
        &entry.provenance,
        Some(wire::intent::RecallProvenance::Renewal { .. })
    )));
    assert!(json.recent.iter().any(|entry| matches!(
        &entry.provenance,
        Some(wire::intent::RecallProvenance::Steering {
            key,
            disposition: wire::Disposition::Live,
            ..
        }) if key == pending_key
    )));
    if let Some((acked, declined, canceled)) = dispositions {
        assert!(json.recent.iter().any(|entry| matches!(
            &entry.provenance,
            Some(wire::intent::RecallProvenance::Compaction { .. })
        )));
        for (key, disposition) in [
            (acked, wire::Disposition::Acked),
            (declined, wire::Disposition::Declined),
            (canceled, wire::Disposition::Canceled),
        ] {
            assert!(json.recent.iter().any(|entry| matches!(
                &entry.provenance,
                Some(wire::intent::RecallProvenance::Steering {
                    key: observed,
                    disposition: observed_disposition,
                    ..
                }) if observed == key && *observed_disposition == disposition
            )));
        }
    } else {
        assert!(json.recent.iter().all(|entry| !matches!(
            &entry.provenance,
            Some(wire::intent::RecallProvenance::Compaction { .. })
        )));
    }
}
