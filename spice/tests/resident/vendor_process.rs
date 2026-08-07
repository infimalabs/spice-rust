use super::*;

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
        fs::canonicalize(fixture.root.join(".git/agent.gitconfig"))
            .expect("agent Git shadow canonicalizes")
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
        "agent shadow mutated repository configuration"
    );

    drain(&fixture, lane);
    let down = operator(&fixture, &["down"], true);
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

#[test]
fn staged_installed_spice_rehearses_clean_break_without_predecessor_tools() {
    let fixture = Fixture::new("installed-native-bootstrap");
    let rig = configure(&fixture);
    prepare_cutover_repository(&fixture, &rig);
    let (installed_binary, plan_digest, binary_digest) = stage_cutover_appliance(&fixture);
    assert!(
        !fixture.journal().exists(),
        "the clean-break fixture inherited a current authority"
    );

    let (resident_path, resident_tools) = native_resident_path(&fixture);
    let mut resident =
        RunningResident::spawn_with_path(&fixture, &installed_binary, &resident_path);
    resident.assert_ready(&fixture);
    let claude = add_lane_with(&fixture, &installed_binary, None, Dialect::Claude);
    let codex = add_lane_with(
        &fixture,
        &installed_binary,
        Some(&rig.codex_worktree),
        Dialect::Codex,
    );
    let lanes = [(Dialect::Claude, claude), (Dialect::Codex, codex)];
    let created = add_cutover_tasks(&fixture, &installed_binary, &lanes);
    assert_launches_with_executable(
        &fixture,
        &rig,
        &lanes,
        false,
        &installed_binary,
        Some(&resident_path),
    );

    let empty_path = fixture.cleanup.join("empty-path");
    fs::create_dir(&empty_path).expect("empty PATH directory");
    assert_cutover_allocation(&fixture, &installed_binary, &empty_path, &lanes, &created);
    let directive_keys =
        assert_cutover_steering(&fixture, &installed_binary, &empty_path, claude, codex);
    exercise_installed_surface(&fixture, &installed_binary, &empty_path, &lanes);
    stop_cutover(
        &fixture,
        &rig,
        &installed_binary,
        &empty_path,
        &lanes,
        resident,
    );
    eprintln!(
        "cutover-evidence plan={plan_digest} binary={binary_digest} journal={} tasks=[{},{}] \
         lanes=[{claude},{codex}] directives=[{},{}] vendor_logs=2 \
         resident_path=[git:{},sleep:{}] client_path_entries=0 shutdown=clean",
        fixture.journal().display(),
        created[0],
        created[1],
        directive_keys[0],
        directive_keys[1],
        resident_tools[0].display(),
        resident_tools[1].display(),
    );
}

fn native_resident_path(fixture: &Fixture) -> (PathBuf, [PathBuf; 2]) {
    let path = fixture.cleanup.join("native-path");
    fs::create_dir(&path).expect("native resident PATH creates");
    let tools = [ambient_executable("git"), ambient_executable("sleep")];
    for (name, source) in ["git", "sleep"].into_iter().zip(&tools) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(source, path.join(name)).expect("native resident tool links");
        #[cfg(not(unix))]
        fs::copy(source, path.join(name)).expect("native resident tool copies");
    }
    let mut entries = fs::read_dir(&path)
        .expect("native resident PATH reads")
        .map(|entry| {
            entry
                .expect("native resident PATH entry reads")
                .file_name()
                .into_string()
                .expect("native resident tool name is UTF-8")
        })
        .collect::<Vec<_>>();
    entries.sort();
    assert_eq!(entries, ["git", "sleep"]);
    assert!(
        entries
            .iter()
            .all(|name| !name.starts_with("python") && name != "task")
    );
    (path, tools)
}

#[allow(
    clippy::disallowed_methods,
    reason = "the process fixture resolves the two ambient native tools at its binary rim"
)]
fn ambient_executable(name: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").expect("fixture PATH exists"))
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("fixture requires native executable {name:?}"))
}

fn prepare_cutover_repository(fixture: &Fixture, rig: &Rig) {
    let mut document =
        fs::read_to_string(fixture.root.join("spice.toml")).expect("fixture config reads");
    document.push_str("\n[tasks]\nstems = [\"cutover\"]\n");
    fs::write(fixture.root.join("spice.toml"), &document).expect("Claude config extends");
    fs::write(
        fixture.root.join(".gitignore"),
        format!(
            concat!(
                ".agents/\n",
                ".spice/\n",
                "codex-worktree/\n",
                "fixture-claude\n",
                "fixture-codex\n",
                "launch-captures/\n",
                "launch-index\n",
                "native-path/\n",
                "{}",
            ),
            app_stage_ignore()
        ),
    )
    .expect("cutover fixture ignore policy writes");
    fixture_git(&fixture.root, &["init", "-b", "main"]);
    fixture_git(&fixture.root, &["config", "user.name", "Spice Cutover"]);
    fixture_git(
        &fixture.root,
        &["config", "user.email", "cutover@spice.invalid"],
    );
    fixture_git(&fixture.root, &["add", ".gitignore", "spice.toml"]);
    fixture_git(&fixture.root, &["commit", "-m", "empty cutover authority"]);
    assert_fixture_repository_clean(&fixture.root);
    fs::remove_dir_all(&rig.codex_worktree).expect("synthetic Codex worktree withdraws");
    fs::remove_dir_all(fixture.root.join(".git/worktrees/vendor-process-codex"))
        .expect("synthetic Codex Git directory withdraws");
    let codex_worktree = rig.codex_worktree.display().to_string();
    fixture_git(
        &fixture.root,
        &[
            "worktree",
            "add",
            "-b",
            "cutover-codex",
            &codex_worktree,
            "HEAD",
        ],
    );
}

fn stage_cutover_appliance(fixture: &Fixture) -> (PathBuf, String, String) {
    let repository = spice::repository::Repository::discover(&fixture.root)
        .expect("linked repository discovers");
    let source_binary = Path::new(env!("CARGO_BIN_EXE_spice"));
    let plan = spice::install::plan(&repository, Some(source_binary)).expect("install plans");
    let digest = plan.digest().expect("install plan digests");
    let binary_digest = plan
        .operations()
        .iter()
        .find(|operation| operation.asset() == spice::install::Asset::Binary)
        .expect("binary operation exists")
        .generated()
        .digest()
        .as_str()
        .to_owned();
    let receipt =
        spice::install::apply(&repository, Some(source_binary), &digest).expect("install applies");
    assert_eq!(receipt.status(), spice::install::InstallStatus::Complete);

    let installed_binary = spice::install::installed_binary(&repository);
    let installed_skill = spice::install::installed_skill(&repository);
    assert!(installed_binary.is_file(), "installed binary is visible");
    assert_eq!(
        fs::read(&installed_binary).expect("installed binary reads"),
        fs::read(source_binary).expect("source binary reads"),
        "the staged appliance binary drifted from the reviewed source binary"
    );
    assert_eq!(
        installed_skill
            .strip_prefix(repository.common_dir())
            .unwrap(),
        Path::new(".spice-install/agents/skills/spice/SKILL.md")
    );
    let skill = fs::read_to_string(&installed_skill).expect("installed skill reads");
    for command in [
        "spice recall",
        "spice board",
        "spice inbox",
        "spice ack",
        "spice run --",
        "spice task next",
        "spice task done",
    ] {
        assert!(
            skill.contains(command),
            "installed skill omitted {command:?}"
        );
    }
    for unavailable in ["spice agent", "spice task status"] {
        assert!(
            !skill.contains(unavailable),
            "installed skill named unavailable command {unavailable:?}"
        );
    }
    (installed_binary, digest.as_str().to_owned(), binary_digest)
}

fn add_cutover_tasks(
    fixture: &Fixture,
    installed_binary: &Path,
    lanes: &[(Dialect, wire::CanonicalUuid); 2],
) -> [wire::TaskHandle; 2] {
    [
        reply_body::<wire::intent::TaskAnswer>(&lane_with(
            fixture,
            installed_binary,
            lanes[0].1,
            &[
                "task",
                "add",
                "Fresh Claude cutover work",
                "--project",
                "cutover.rehearsal",
            ],
            true,
        ))
        .task,
        reply_body::<wire::intent::TaskAnswer>(&lane_with(
            fixture,
            installed_binary,
            lanes[1].1,
            &[
                "task",
                "add",
                "Fresh Codex cutover work",
                "--project",
                "cutover.rehearsal",
            ],
            true,
        ))
        .task,
    ]
}

fn assert_cutover_allocation(
    fixture: &Fixture,
    installed_binary: &Path,
    empty_path: &Path,
    lanes: &[(Dialect, wire::CanonicalUuid); 2],
    created: &[wire::TaskHandle; 2],
) {
    let mut allocated = Vec::new();
    for (_, lane) in lanes {
        let claimed = reply_body::<wire::intent::ClaimAnswer>(&lane_command(
            fixture,
            installed_binary,
            *lane,
            empty_path,
            &["task", "next"],
            true,
        ));
        assert_eq!(claimed.holder, wire::Actor::Lane(*lane));
        allocated.push(claimed.task);
    }
    let mut created_handles = created.to_vec();
    created_handles.sort();
    allocated.sort();
    assert_eq!(allocated, created_handles);
    let board = reply_body::<wire::intent::BoardAnswer>(&operator_with_path(
        fixture,
        installed_binary,
        empty_path,
        &["board"],
        true,
    ));
    let mut board_handles = board
        .tasks
        .iter()
        .map(|task| task.statement.body.handle.clone())
        .collect::<Vec<_>>();
    board_handles.sort();
    assert_eq!(board_handles, created_handles);
}

fn assert_cutover_steering(
    fixture: &Fixture,
    installed_binary: &Path,
    empty_path: &Path,
    claude: wire::CanonicalUuid,
    codex: wire::CanonicalUuid,
) -> [String; 2] {
    let claude_direction = reply_body::<wire::intent::DirectiveAnswer>(&operator_with_path(
        fixture,
        installed_binary,
        empty_path,
        &[
            "steer",
            &claude.to_string(),
            "Claude-only cutover direction",
        ],
        true,
    ));
    let codex_direction = reply_body::<wire::intent::DirectiveAnswer>(&operator_with_path(
        fixture,
        installed_binary,
        empty_path,
        &["steer", &codex.to_string(), "Codex-only cutover direction"],
        true,
    ));
    for (lane, own, foreign) in [
        (claude, &claude_direction.key, &codex_direction.key),
        (codex, &codex_direction.key, &claude_direction.key),
    ] {
        let inbox = reply_body::<wire::intent::InboxAnswer>(&lane_command(
            fixture,
            installed_binary,
            lane,
            empty_path,
            &["inbox"],
            true,
        ));
        assert_eq!(inbox.lanes.len(), 1);
        let keys = inbox.lanes[0]
            .pending
            .iter()
            .map(|pending| pending.key.as_str())
            .collect::<Vec<_>>();
        assert!(keys.contains(&own.as_str()));
        assert!(!keys.contains(&foreign.as_str()));
    }
    [claude_direction.key, codex_direction.key]
}

fn exercise_installed_surface(
    fixture: &Fixture,
    installed_binary: &Path,
    empty_path: &Path,
    lanes: &[(Dialect, wire::CanonicalUuid); 2],
) {
    let native_probe = fixture.cleanup.join("native-probe");
    fs::write(&native_probe, "#!/bin/sh\nexit 0\n").expect("native probe writes");
    #[cfg(unix)]
    fs::set_permissions(&native_probe, fs::Permissions::from_mode(0o700))
        .expect("native probe becomes executable");
    for (_, lane) in lanes {
        for command in ["recall", "board", "inbox"] {
            let output = lane_command(
                fixture,
                installed_binary,
                *lane,
                empty_path,
                &[command],
                true,
            );
            assert!(
                output.status.success(),
                "installed {command} failed: stdout={:?} stderr={:?}",
                output.stdout,
                output.stderr
            );
        }
        let output = lane_command(
            fixture,
            installed_binary,
            *lane,
            empty_path,
            &[
                "run",
                "--",
                native_probe.to_str().expect("native probe path is UTF-8"),
            ],
            false,
        );
        assert!(
            output.status.success(),
            "installed run failed: stdout={:?} stderr={:?}",
            output.stdout,
            output.stderr
        );
    }
}

fn stop_cutover(
    fixture: &Fixture,
    rig: &Rig,
    installed_binary: &Path,
    empty_path: &Path,
    lanes: &[(Dialect, wire::CanonicalUuid); 2],
    resident: RunningResident,
) {
    for (_, lane) in lanes {
        let drained = operator_with_path(
            fixture,
            installed_binary,
            empty_path,
            &["lane", "drain", &lane.to_string()],
            true,
        );
        assert!(
            drained.status.success(),
            "drain failed: {:?}",
            drained.stderr
        );
    }
    assert_processes_stopped(&rig.capture_index, 1);
    let down = operator_with_path(fixture, installed_binary, empty_path, &["down"], true);
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
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
        drain(&fixture, lane);
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

#[test]
fn malformed_codex_session_refuses_and_reaps_before_lane_publication() {
    let fixture = Fixture::linked("vendor-malformed-codex-session");
    let rig = configure(&fixture);
    let stopped = fixture.cleanup.join("malformed-codex-stopped");
    let vendor_pid = fixture.cleanup.join("malformed-codex-pid");
    fs::write(
        &rig.codex_binary,
        format!(
            concat!(
                "#!/bin/sh\n",
                "trap 'printf stopped > {}; exit 0' TERM INT\n",
                "printf '%s\\n' \"$$\" > {}\n",
                "printf '%s\\n' '{{\"type\":\"thread.started\"}}'\n",
                "while :; do sleep 1; done\n",
            ),
            shell_word(&stopped.display().to_string()),
            shell_word(&vendor_pid.display().to_string()),
        ),
    )
    .expect("malformed Codex vendor installs");
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);

    let output = operator(&fixture, &["lane", "add", "--vendor", "codex"], true);
    assert_eq!(output.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) = value_outcome(&output) else {
        panic!("malformed session must refuse")
    };
    assert_eq!(refusal.reason, "lane::session-missing");
    // The refusal returns to the operator without waiting for the vendor's TERM
    // trap, so the stop is observed rather than read at the return.
    let stop = wait_for_vendor_stop(&fixture.cleanup, &vendor_pid, &stopped);
    assert_eq!(stop, "stopped");
    assert!(durable_head(&fixture).lanes.is_empty());

    let down = operator(&fixture, &["down"], true);
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

include!("vendor_process/support.rs");
