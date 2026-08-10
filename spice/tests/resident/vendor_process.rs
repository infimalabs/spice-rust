use super::*;

use std::cell::Cell;
use std::collections::BTreeMap;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

const CLAUDE_MODEL: &str = "claude-opus-fixture";
const CODEX_MODEL: &str = "gpt-fixture";
const CODEX_PERSONALITY: &str = "pragmatic";
const COMPACT_WINDOW: &str = "424242";
const CLAUDE_SESSION_ID: &str = "018f7777-7777-7444-8888-999999999999";
const CODEX_SESSION_ID: &str = "019fbc32-beb1-7ee0-b814-a7513f8f8ea5";
const CLAUDE_SESSION_RECORD: &str =
    r#"{"type":"system","subtype":"init","session_id":"018f7777-7777-7444-8888-999999999999"}"#;
const CODEX_SESSION_RECORD: &str =
    r#"{"type":"thread.started","thread_id":"019fbc32-beb1-7ee0-b814-a7513f8f8ea5"}"#;
/// A record the Codex decoder types, carrying everything but its session.
const CODEX_SESSIONLESS_RECORD: &str = r#"{"type":"thread.started"}"#;
/// Bytes no decoder types at all, so no record is even read to lack a session.
const CODEX_UNTYPED_RECORD: &str = "thread.started";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Dialect {
    Claude,
    Codex,
}

impl Dialect {
    const fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    const fn session_id(self) -> &'static str {
        match self {
            Self::Claude => CLAUDE_SESSION_ID,
            Self::Codex => CODEX_SESSION_ID,
        }
    }

    const fn session_record(self) -> &'static str {
        match self {
            Self::Claude => CLAUDE_SESSION_RECORD,
            Self::Codex => CODEX_SESSION_RECORD,
        }
    }
}

struct Rig {
    codex_worktree: PathBuf,
    capture_index: PathBuf,
    claude_binary: PathBuf,
    codex_binary: PathBuf,
}

struct CapturedLaunch {
    argv: Vec<String>,
    environment: BTreeMap<String, String>,
    cwd: String,
}

#[test]
fn composed_vendor_launches_cross_the_resident_and_sill_boundary_without_drift() {
    let fixture = Fixture::linked("vendor-process-composition");
    let rig = configure(&fixture);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);

    let claude = add_lane(&fixture, None, Dialect::Claude);
    let codex = add_lane(&fixture, Some(&rig.codex_worktree), Dialect::Codex);
    let lanes = [(Dialect::Claude, claude), (Dialect::Codex, codex)];
    assert_launches(&fixture, &rig, &lanes, false);

    for (_, lane) in lanes {
        drain(&fixture, lane);
    }
    assert_processes_stopped(&rig.capture_index, 1);
    for (_, lane) in lanes {
        wake(&fixture, lane);
    }
    assert_launches(&fixture, &rig, &lanes, true);
    assert_tapes(&fixture, &lanes);

    for (_, lane) in lanes {
        drain(&fixture, lane);
    }
    assert_processes_stopped(&rig.capture_index, 2);
    let down = operator(&fixture, &["down"], true);
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

#[test]
fn resident_agent_process_receives_both_git_shadow_scopes() {
    let fixture = Fixture::new("vendor-git-shadow");
    let capture_dir = fixture.root.join("launch-captures");
    fs::create_dir_all(&capture_dir).expect("capture directory");
    let capture_index = fixture.root.join("launch-index");
    let binary = fixture.root.join("fixture-claude");
    write_vendor(&binary, &capture_dir, &capture_index, Dialect::Claude);
    let document = format!(
        concat!(
            "[lane]\nlease = \"30s\"\n\n",
            "[vendor.claude]\n",
            "binary = {:?}\nmodel = {:?}\neffort = \"xhigh\"\n",
            "auto_compact_window_tokens = 424242\n",
        ),
        binary.display().to_string(),
        CLAUDE_MODEL,
    );
    fs::write(fixture.root.join("spice.toml"), document).expect("vendor configuration");
    // The seeded app stage is build output under the worktree root; without this
    // the fixture reads dirty and the clean-worktree assertions below fail on
    // `?? dist/` rather than on anything this test is about.
    fs::write(
        fixture.root.join(".gitignore"),
        format!(
            "fixture-claude\nlaunch-captures\nlaunch-index\n{}",
            app_stage_ignore()
        ),
    )
    .expect("fixture ignore policy");
    fixture_git(&fixture.root, &["init", "-b", "main-d"]);
    fixture_git(&fixture.root, &["config", "user.name", "Spice Tests"]);
    fixture_git(
        &fixture.root,
        &["config", "user.email", "spice@example.test"],
    );
    fixture_git(&fixture.root, &["add", ".gitignore", "spice.toml"]);
    fixture_git(&fixture.root, &["commit", "-m", "fixture baseline"]);
    assert_fixture_repository_clean(&fixture.root);
    fixture_git(&fixture.root, &["branch", "integration"]);
    fixture_git(
        &fixture.root,
        &["commit", "--allow-empty", "-m", "lane head"],
    );
    fixture_git(&fixture.root, &["config", "branch.main-d.remote", "origin"]);
    fixture_git(
        &fixture.root,
        &["config", "branch.main-d.merge", "refs/heads/integration"],
    );
    let native_config = fixture.root.join(".git/config");
    let native_before = fs::read(&native_config).expect("native config reads");
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);

    let lane = add_lane(&fixture, None, Dialect::Claude);
    let capture = CapturedLaunch::read(
        captures(&capture_index, Dialect::Claude)
            .last()
            .expect("agent launch captured"),
    );

    assert_eq!(capture.environment["GIT_CONFIG_COUNT"], "2");
    assert_eq!(
        capture.environment["GIT_CONFIG_KEY_0"],
        "branch.main-d.merge"
    );
    assert_eq!(
        capture.environment["GIT_CONFIG_VALUE_0"],
        "refs/heads/integration"
    );
    assert_eq!(
        capture.environment["GIT_CONFIG_KEY_1"],
        "branch.main-d.remote"
    );
    assert_eq!(capture.environment["GIT_CONFIG_VALUE_1"], ".");
    assert_eq!(
        Path::new(&capture.environment["GIT_CONFIG_SYSTEM"]),
        fs::canonicalize(fixture.root.join(".git/spice-rs-agent.gitconfig"))
            .expect("the spice-rs-agent.gitconfig path canonicalizes")
    );
    assert_eq!(
        fixture_git_stdout_with_environment(
            &fixture.root,
            &["rev-parse", "--symbolic-full-name", "@{upstream}"],
            &capture.environment,
        ),
        "refs/heads/main-d"
    );
    assert_eq!(
        fixture_git_stdout_with_environment(
            &fixture.root,
            &["config", "--get", "branch.main-d.merge"],
            &capture.environment,
        ),
        "refs/heads/integration"
    );
    assert_eq!(
        fixture_git_stdout_with_environment(
            &fixture.root,
            &["status", "--short", "--branch"],
            &capture.environment,
        ),
        "## main-d...main-d"
    );
    assert_eq!(
        fs::read(&native_config).expect("native config rereads"),
        native_before,
        "Git shadow mutated repository configuration"
    );

    drain(&fixture, lane);
    let down = operator(&fixture, &["down"], true);
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}
#[path = "vendor_process/coexistence.rs"]
mod coexistence;

/// Creating a PID file is not the same event as publishing a PID through it.
#[test]
fn a_created_pid_file_is_not_complete_until_its_bytes_land() {
    let fixture = Fixture::new("vendor-child-pid-publication");
    let pid_file = fixture.cleanup.join("child.pid");

    fs::write(&pid_file, []).expect("empty child pid file creates");
    assert!(pid_file.is_file(), "the old readiness predicate is true");
    assert_eq!(
        fs::read_to_string(&pid_file).expect("empty child pid file reads"),
        "",
        "the control did not expose the create-before-write window"
    );
    assert_eq!(
        recorded_pid(&pid_file),
        None,
        "an empty pid file was treated as a complete publication"
    );

    let expected = std::process::id();
    fs::write(&pid_file, format!("{expected}\n")).expect("complete child pid writes");
    assert_eq!(recorded_pid(&pid_file), Some(expected));
}

/// The fixture wrapper owns its child all the way through signal teardown.
///
/// Resident termination reaches the whole process group, so the ordinary drain
/// battery can leave a broken wrapper looking correct until the child's orphaned
/// zombie survives long enough for the post-drain identity probe to see it. This
/// probe removes that timing variable: TERM reaches only the wrapper, and the
/// wrapper is collected before the child identity is inspected. The child can
/// be gone at that late probe only when the wrapper trap terminated and reaped
/// it before reporting its own exit.
#[test]
fn fixture_vendor_reaps_its_child_before_the_wrapper_exits() {
    let fixture = Fixture::new("vendor-child-reap");
    let capture_dir = fixture.cleanup.join("launch-captures");
    fs::create_dir_all(&capture_dir).expect("capture directory");
    let index = fixture.cleanup.join("launch-index");
    let binary = fixture.cleanup.join("fixture-claude");
    write_vendor(&binary, &capture_dir, &index, Dialect::Claude);

    let mut vendor = Command::new(&binary)
        .current_dir(&fixture.root)
        .stdout(Stdio::null())
        .spawn()
        .expect("fixture vendor launches");
    let child_pid_file =
        capture_dir.join(format!("{}-{}.child", Dialect::Claude.name(), vendor.id()));
    // Creation makes the path watchable but does not make its bytes complete.
    // The second wait arms that file before reading it, so either the PID is
    // already complete or its finishing write produces the next wakeup.
    match readiness::wait_until_ready(
        &mut vendor,
        &capture_dir,
        readiness::ARRIVAL_HORIZON,
        "fixture vendor child pid file",
        || child_pid_file.is_file(),
    ) {
        Readiness::Ready => {}
        Readiness::Exited => panic!("fixture vendor exited before creating its child pid file"),
    }

    let child_pid = Cell::new(None);
    match readiness::wait_until_ready(
        &mut vendor,
        &child_pid_file,
        readiness::ARRIVAL_HORIZON,
        "fixture vendor child pid write",
        || {
            if let Some(pid) = recorded_pid(&child_pid_file) {
                child_pid.set(Some(pid));
                true
            } else {
                false
            }
        },
    ) {
        Readiness::Ready => {}
        Readiness::Exited => panic!("fixture vendor exited before completing its child pid write"),
    }

    let child = child_pid
        .get()
        .expect("fixture child readiness carries a parsed pid");
    let terminated = Command::new("kill")
        .args(["-TERM", &vendor.id().to_string()])
        .output()
        .expect("fixture wrapper termination launches");
    assert!(
        terminated.status.success(),
        "fixture wrapper termination failed: {:?}",
        terminated.stderr
    );
    assert!(
        vendor.wait().expect("fixture wrapper reaps").success(),
        "fixture wrapper did not choose a successful exit"
    );

    let child_survived = process_alive(child);
    if child_survived {
        let cleanup = Command::new("kill")
            .args(["-KILL", &child.to_string()])
            .output()
            .expect("surviving fixture child cleanup launches");
        assert!(
            cleanup.status.success(),
            "surviving fixture child cleanup failed: {:?}",
            cleanup.stderr
        );
    }
    assert!(!child_survived, "fixture child survived wrapper exit");
}

#[test]
fn cross_vendor_options_refuse_with_exact_paths_before_any_process_capture() {
    for (dialect, path) in [
        (Dialect::Claude, "vendor.claude.fast_mode"),
        (Dialect::Codex, "vendor.codex.auto_compact_window_tokens"),
    ] {
        let fixture = Fixture::linked(&format!("vendor-option-{}", dialect.name()));
        let rig = configure(&fixture);
        let valid = fs::read_to_string(fixture.root.join("spice.toml")).expect("valid config");
        let invalid = match dialect {
            Dialect::Claude => valid.replacen(
                "effort = \"xhigh\"\n",
                "effort = \"xhigh\"\nfast_mode = true\n",
                1,
            ),
            Dialect::Codex => valid.replacen(
                "fast_mode = true\n",
                "fast_mode = true\nauto_compact_window_tokens = 100\n",
                1,
            ),
        };
        let mut resident = RunningResident::spawn(&fixture);
        resident.assert_ready(&fixture);
        fs::write(fixture.root.join("spice.toml"), invalid).expect("invalid config installs");

        let output = operator(&fixture, &["lane", "add", "--vendor", dialect.name()], true);
        assert!(!output.status.success(), "unsupported option must refuse");
        let diagnostic = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            diagnostic.contains(path),
            "refusal omitted {path:?}: {diagnostic}"
        );
        assert!(
            !rig.capture_index.exists(),
            "vendor process was reached before configuration refusal"
        );
        assert!(
            !fixture.runtime.join("tape").exists(),
            "launch tape was created before configuration refusal"
        );

        fs::write(fixture.root.join("spice.toml"), valid).expect("valid config restores");
        let down = operator(&fixture, &["down"], true);
        assert!(down.status.success(), "down failed: {:?}", down.stderr);
        assert!(resident.wait().status.success(), "resident failed");
    }
}

#[test]
fn nonzero_vendor_exits_are_reaped_and_recorded_for_both_dialects() {
    for dialect in [Dialect::Claude, Dialect::Codex] {
        let fixture = Fixture::linked(&format!("vendor-exit-{}", dialect.name()));
        let rig = configure(&fixture);
        let exit_pipe = fixture.cleanup.join(format!("{}-exit", dialect.name()));
        let made = Command::new("mkfifo")
            .arg(&exit_pipe)
            .output()
            .expect("mkfifo launches");
        assert!(made.status.success(), "mkfifo failed: {:?}", made.stderr);
        let pid_file = fixture.cleanup.join(format!("{}-exit-pid", dialect.name()));
        fs::write(
            binary(&rig, dialect),
            format!(
                concat!(
                    "#!/bin/sh\n",
                    "exec 3>{}\n",
                    "printf '%s\\n' \"$$\" > {}\n",
                    "printf '%s\\n' {}\n",
                    "exit 7\n",
                ),
                shell_word(&exit_pipe.display().to_string()),
                shell_word(&pid_file.display().to_string()),
                shell_word(dialect.session_record()),
            ),
        )
        .expect("exiting vendor installs");
        let observer = std::thread::spawn(move || {
            let mut pipe = fs::File::open(exit_pipe).expect("exit observer opens");
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes)
                .expect("exit observer reaches EOF");
            assert!(bytes.is_empty(), "exit pipe carries no payload");
        });
        let mut resident = RunningResident::spawn(&fixture);
        resident.assert_ready(&fixture);

        let lane = add_lane(&fixture, None, dialect);
        observer.join().expect("vendor exit is observed");
        wait_for_lane_exit(&fixture, lane);
        let pid = fs::read_to_string(&pid_file)
            .expect("vendor pid reads")
            .trim()
            .parse::<u32>()
            .expect("vendor pid parses");
        assert!(!process_alive(pid), "exited vendor was not reaped");
        let facts = journal::JournalReader::open(fixture.journal())
            .expect("exit journal reads")
            .fact_snapshot()
            .expect("exit facts read");
        let exited = facts
            .facts()
            .iter()
            .find(|fact| {
                fact.fact.kind() == FactKind::LaneExited
                    && fact.fact.body().get("lane")
                        == Some(&serde_json::Value::String(lane.to_string()))
            })
            .expect("nonzero exit fact exists");
        assert_eq!(exited.fact.body()["code"], 7);

        let down = operator(&fixture, &["down"], true);
        assert!(down.status.success(), "down failed: {:?}", down.stderr);
        assert!(resident.wait().status.success(), "resident failed");
    }
}

/// A vendor the resident had to kill is recorded as killed, not as an exit.
///
/// This is the one ending the resident causes itself, and it used to be the one
/// it could not name. A signalled process reports no status, so the resident
/// substituted `-1` and published `lane::exited` — the same document a vendor
/// that returned `-1` on its own produces. The grace overrun was gone from the
/// record the moment it was written.
///
/// The vendor here ignores TERM outright rather than trapping it slowly, so the
/// escalation is the fixture's subject instead of a race it might win. Both
/// halves of the assertion carry weight: `lane::killed` must be present, and
/// `lane::exited` must be absent for the same lane, because a resident that
/// wrote both would still leave a reader unable to say which ending happened.
#[test]
fn a_vendor_that_ignores_termination_is_recorded_as_killed_rather_than_exited() {
    let fixture = Fixture::linked("vendor-killed");
    let rig = configure(&fixture);
    let pid_file = fixture.cleanup.join("ignoring-vendor-pid");
    fs::write(
        &rig.claude_binary,
        format!(
            concat!(
                "#!/bin/sh\n",
                "trap '' TERM INT\n",
                "printf '%s\\n' \"$$\" > {}\n",
                "printf '%s\\n' {}\n",
                "while :; do sleep 1; done\n",
            ),
            shell_word(&pid_file.display().to_string()),
            shell_word(Dialect::Claude.session_record()),
        ),
    )
    .expect("ignoring vendor installs");
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);

    let lane = add_lane(&fixture, None, Dialect::Claude);
    drain(&fixture, lane);

    let pid = fs::read_to_string(&pid_file)
        .expect("vendor pid reads")
        .trim()
        .parse::<u32>()
        .expect("vendor pid parses");
    assert!(!process_alive(pid), "killed vendor was not reaped");
    assert_eq!(
        lane_facts(&fixture, lane, FactKind::LaneKilled),
        1,
        "the kill is recorded once"
    );
    assert_eq!(
        lane_facts(&fixture, lane, FactKind::LaneExited),
        0,
        "a kill is not also published as an exit the occupant chose"
    );

    let down = operator(&fixture, &["down"], true);
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

/// A vendor that never traps TERM dies one stage earlier, and is still recorded
/// as ended by this resident rather than as an exit it chose.
///
/// The sibling above installs a trap that ignores TERM, so it survives to the
/// kill. This one installs no trap at all, so the default disposition ends it
/// inside the termination grace and the terminate stage is what observes it.
/// That stage is the one this resident used to misreport: a signalled process
/// carries no status code, so the classifier substituted -1 and published
/// `lane::exited`, a status no occupant ever returned. The two fixtures differ
/// only in that trap line, which is what makes the pair read as one comparison.
///
/// This drives the lane drain. The resident has a second route to the same
/// ending, the shutdown ladder, which classifies it in its own code and so was
/// its own defect; the test below drives that one and asserts the same record.
///
/// Both halves are load-bearing for the same reason as the sibling: writing both
/// endings would leave a reader unable to say which one happened.
#[test]
fn a_vendor_that_never_traps_termination_is_not_recorded_as_choosing_its_exit() {
    let fixture = Fixture::linked("vendor-terminated");
    let rig = configure(&fixture);
    let pid_file = fixture.cleanup.join("untrapped-vendor-pid");
    fs::write(
        &rig.claude_binary,
        format!(
            concat!(
                "#!/bin/sh\n",
                "printf '%s\\n' \"$$\" > {}\n",
                "printf '%s\\n' {}\n",
                "while :; do sleep 1; done\n",
            ),
            shell_word(&pid_file.display().to_string()),
            shell_word(Dialect::Claude.session_record()),
        ),
    )
    .expect("untrapped vendor installs");
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let lane = add_lane(&fixture, None, Dialect::Claude);
    drain(&fixture, lane);

    let pid = fs::read_to_string(&pid_file)
        .expect("vendor pid reads")
        .trim()
        .parse::<u32>()
        .expect("vendor pid parses");
    assert!(!process_alive(pid), "terminated vendor was not reaped");
    assert_eq!(
        lane_facts(&fixture, lane, FactKind::LaneKilled),
        1,
        "the terminate stage records the ending it caused"
    );
    assert_eq!(
        lane_facts(&fixture, lane, FactKind::LaneExited),
        0,
        "a signalled occupant is not credited with an exit status it never chose"
    );

    let down = operator(&fixture, &["down"], true);
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

/// The shutdown ladder records an untrapped vendor the way the lane drain does.
///
/// The same vendor and the same ending as the test above, reached by this
/// resident's other route to it. The two routes classify in separate code, so
/// agreement between them is a claim that has to be made rather than assumed;
/// the ladder was the site the fix was asked for and the drain was the one it
/// also needed.
///
/// Shortening the lease is what puts the ladder inside the operator's bound. The
/// ladder's drain stage waits out whatever remains of the occupant's lease before
/// it signals anything, and a vendor that loops forever never shortens that wait
/// by ending; at the fixture's usual 30s lease the operator gives up first.
#[test]
fn the_shutdown_ladder_records_an_untrapped_vendor_the_way_the_drain_does() {
    let fixture = Fixture::linked("ladder-terminated");
    let rig = configure(&fixture);
    let config = fixture.root.join("spice.toml");
    let document = fs::read_to_string(&config).expect("fixture config reads");
    let shortened = document.replace("lease = \"30s\"", "lease = \"1s\"");
    assert_ne!(
        shortened, document,
        "the shared fixture no longer spells the lease this test shortens"
    );
    fs::write(&config, shortened).expect("shortened lease installs");
    let pid_file = fixture.cleanup.join("ladder-vendor-pid");
    fs::write(
        &rig.claude_binary,
        format!(
            concat!(
                "#!/bin/sh\n",
                "printf '%s\\n' \"$$\" > {}\n",
                "printf '%s\\n' {}\n",
                "while :; do sleep 1; done\n",
            ),
            shell_word(&pid_file.display().to_string()),
            shell_word(Dialect::Claude.session_record()),
        ),
    )
    .expect("untrapped vendor installs");
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let lane = add_lane(&fixture, None, Dialect::Claude);

    let down = operator(&fixture, &["down"], true);
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");

    let pid = fs::read_to_string(&pid_file)
        .expect("vendor pid reads")
        .trim()
        .parse::<u32>()
        .expect("vendor pid parses");
    assert!(!process_alive(pid), "terminated vendor was not reaped");
    assert_eq!(
        lane_facts(&fixture, lane, FactKind::LaneKilled),
        1,
        "the ladder's terminate stage records the ending it caused"
    );
    assert_eq!(
        lane_facts(&fixture, lane, FactKind::LaneExited),
        0,
        "a signalled occupant is not credited with an exit status it never chose"
    );
}

/// A vendor killed by someone other than this resident keeps its own record.
///
/// The other half of the pair above. Both vendors die to a signal and neither
/// reports a status code, so the platform tells this resident the same thing in
/// both cases; only the resident knows which signal it sent. That is the whole
/// distinction the two kinds carry, and a test that asserted the killed side
/// alone would not show it: the ledger became more truthful only if this ending
/// still reads as an exit while the signalled one no longer does.
///
/// `-1` is the code because there is no code, and here that substitute says
/// something true — a signal this resident did not send. A Unix status code is 0
/// to 255, so it cannot be confused with a status a vendor chose to return.
///
/// The fifo is the process-side synchronization, not the pid: reaching EOF on
/// its write end proves the vendor is gone. The resident's journal event is the
/// independent synchronization proving the kernel callback reaped and durably
/// vacated that exact occupant without a later drain command.
#[test]
fn a_vendor_killed_by_another_hand_is_still_recorded_as_an_exit() {
    let fixture = Fixture::linked("vendor-foreign-signal");
    let rig = configure(&fixture);
    let exit_pipe = fixture.cleanup.join("foreign-signal-exit");
    let made = Command::new("mkfifo")
        .arg(&exit_pipe)
        .output()
        .expect("mkfifo launches");
    assert!(made.status.success(), "mkfifo failed: {:?}", made.stderr);
    let pid_file = fixture.cleanup.join("foreign-signal-pid");
    fs::write(
        &rig.claude_binary,
        format!(
            concat!(
                "#!/bin/sh\n",
                "exec 3>{}\n",
                "printf '%s\\n' \"$$\" > {}\n",
                "printf '%s\\n' {}\n",
                // Replaces the shell so the pid written above is the process the
                // kill reaches, with no sleep child outliving it.
                "exec sleep 1000\n",
            ),
            shell_word(&exit_pipe.display().to_string()),
            shell_word(&pid_file.display().to_string()),
            shell_word(Dialect::Claude.session_record()),
        ),
    )
    .expect("signalled vendor installs");
    let observer = std::thread::spawn(move || {
        let mut pipe = fs::File::open(exit_pipe).expect("exit observer opens");
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes)
            .expect("exit observer reaches EOF");
    });
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let lane = add_lane(&fixture, None, Dialect::Claude);

    let pid = fs::read_to_string(&pid_file)
        .expect("vendor pid reads")
        .trim()
        .parse::<u32>()
        .expect("vendor pid parses");
    let killed = Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .output()
        .expect("kill launches");
    assert!(killed.status.success(), "kill failed: {:?}", killed.stderr);
    observer.join().expect("foreign kill reaches the vendor");
    wait_for_lane_exit(&fixture, lane);

    let facts = journal::JournalReader::open(fixture.journal())
        .expect("signal journal reads")
        .fact_snapshot()
        .expect("signal facts read");
    let exited = facts
        .facts()
        .iter()
        .find(|fact| {
            fact.fact.kind() == FactKind::LaneExited
                && fact.fact.body().get("lane")
                    == Some(&serde_json::Value::String(lane.to_string()))
        })
        .expect("a death this resident did not cause is recorded as an exit");
    assert_eq!(
        exited.fact.body()["code"],
        -1,
        "the absent status is named, not invented"
    );
    assert_eq!(
        lane_facts(&fixture, lane, FactKind::LaneKilled),
        0,
        "this resident sent no signal, so it claims no ending"
    );

    let down = operator(&fixture, &["down"], true);
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

#[test]
fn malformed_codex_session_refuses_and_reaps_before_lane_publication() {
    for (name, announced, observed) in [
        (
            "sessionless",
            CODEX_SESSIONLESS_RECORD,
            "one record carrying no thread_id",
        ),
        (
            "untyped",
            CODEX_UNTYPED_RECORD,
            "one record whose $ is not JSON record",
        ),
    ] {
        let fixture = Fixture::linked(&format!("vendor-malformed-codex-{name}"));
        let rig = configure(&fixture);
        let stopped = fixture.cleanup.join("malformed-codex-stopped");
        let vendor_pid = fixture.cleanup.join("malformed-codex-pid");
        fs::write(
            &rig.codex_binary,
            vendor_prologue(&stopped, &vendor_pid)
                + &format!(
                    concat!("printf '%s\\n' {}\n", "while :; do sleep 1; done\n"),
                    shell_word(announced),
                ),
        )
        .expect("malformed Codex vendor installs");
        let mut resident = RunningResident::spawn(&fixture);
        resident.assert_ready(&fixture);

        let output = operator(&fixture, &["lane", "add", "--vendor", "codex"], true);
        assert_eq!(output.status.code(), Some(REFUSAL_EXIT));
        let Outcome::Refusal(refusal) = value_outcome(&output) else {
            panic!("{name} session must refuse")
        };
        assert_eq!(refusal.reason, "lane::session-missing");
        // Both cases refuse under the one reason the proof deadline raises, so
        // only the observation separates a record the decoder typed and found
        // sessionless from bytes it could not read as a record at all. It also
        // holds the two announce lines apart: either drifting into the other's
        // shape earns the other's observation and reds this case.
        assert_eq!(refusal.observed.as_deref(), Some(observed));
        // The refusal returns to the operator without waiting for the vendor's
        // TERM trap, so the stop is observed rather than read at the return.
        let stop = wait_for_vendor_stop(&fixture.cleanup, &vendor_pid, &stopped);
        assert_eq!(stop, "stopped");
        assert!(durable_head(&fixture).lanes.is_empty());

        let down = operator(&fixture, &["down"], true);
        assert!(down.status.success(), "down failed: {:?}", down.stderr);
        assert!(resident.wait().status.success(), "resident failed");
    }
}

#[path = "vendor_process/support.rs"]
mod support;
use support::*;
