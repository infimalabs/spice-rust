use super::*;

use std::collections::BTreeMap;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

const PROMPT: &str = "[$spice](.agents/skills/spice/SKILL.md)";
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
    fs::write(
        fixture.root.join(".gitignore"),
        "fixture-claude\nlaunch-captures\nlaunch-index\n",
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
    let created = add_cutover_tasks(&fixture, &installed_binary);
    let claude = add_lane_with(&fixture, &installed_binary, None, Dialect::Claude);
    let codex = add_lane_with(
        &fixture,
        &installed_binary,
        Some(&rig.codex_worktree),
        Dialect::Codex,
    );
    let lanes = [(Dialect::Claude, claude), (Dialect::Codex, codex)];
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
        concat!(
            ".agents/\n",
            ".spice/\n",
            "codex-worktree/\n",
            "fixture-claude\n",
            "fixture-codex\n",
            "launch-captures/\n",
            "launch-index\n",
            "native-path/\n",
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

    let installed_binary = fixture.root.join(
        plan.operations()
            .iter()
            .find(|operation| operation.asset() == spice::install::Asset::Binary)
            .expect("binary operation exists")
            .path(),
    );
    let installed_skill = fixture.root.join(
        plan.operations()
            .iter()
            .find(|operation| operation.asset() == spice::install::Asset::Skill)
            .expect("skill operation exists")
            .path(),
    );
    assert!(installed_binary.is_file(), "installed binary is visible");
    assert_eq!(
        fs::read(&installed_binary).expect("installed binary reads"),
        fs::read(source_binary).expect("source binary reads"),
        "the staged appliance binary drifted from the reviewed source binary"
    );
    assert_eq!(
        installed_skill.strip_prefix(&fixture.root).unwrap(),
        Path::new(".agents/skills/spice/SKILL.md")
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

fn add_cutover_tasks(fixture: &Fixture, installed_binary: &Path) -> [wire::TaskHandle; 2] {
    [
        reply_body::<wire::intent::TaskAnswer>(&operator_with(
            fixture,
            installed_binary,
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
        reply_body::<wire::intent::TaskAnswer>(&operator_with(
            fixture,
            installed_binary,
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
    let stop = wait_for_vendor_stop(&vendor_pid, &stopped);
    assert_eq!(stop, "stopped");
    assert!(durable_head(&fixture).lanes.is_empty());

    let down = operator(&fixture, &["down"], true);
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

fn configure(fixture: &Fixture) -> Rig {
    let common = fixture
        .runtime
        .parent()
        .expect("runtime has common Git parent");
    let codex_worktree = fixture.cleanup.join("codex-worktree");
    let codex_git = common.join("worktrees/vendor-process-codex");
    fs::create_dir_all(&codex_worktree).expect("Codex worktree directory");
    fs::create_dir_all(&codex_git).expect("Codex Git directory");
    fs::write(
        codex_worktree.join(".git"),
        format!("gitdir: {}\n", codex_git.display()),
    )
    .expect("Codex Git pointer");
    fs::write(codex_git.join("commondir"), "../..\n").expect("Codex common pointer");

    let captures = fixture.cleanup.join("launch-captures");
    fs::create_dir_all(&captures).expect("capture directory");
    let capture_index = fixture.cleanup.join("launch-index");
    let claude_binary = fixture.cleanup.join("fixture-claude");
    let codex_binary = fixture.cleanup.join("fixture-codex");
    write_vendor(&claude_binary, &captures, &capture_index, Dialect::Claude);
    write_vendor(&codex_binary, &captures, &capture_index, Dialect::Codex);

    let document = format!(
        concat!(
            "[policy.commit_message]\n",
            "blocked_trailers = [\"Co-Authored-By\"]\n\n",
            "[lane]\nlease = \"30s\"\n\n",
            "[vendor.claude]\n",
            "binary = {:?}\nmodel = {:?}\neffort = \"xhigh\"\n",
            "auto_compact_window_tokens = 424242\n\n",
            "[vendor.codex]\n",
            "binary = {:?}\nmodel = {:?}\neffort = \"max\"\n",
            "personality = {:?}\nfast_mode = true\n",
        ),
        claude_binary.display().to_string(),
        CLAUDE_MODEL,
        codex_binary.display().to_string(),
        CODEX_MODEL,
        CODEX_PERSONALITY,
    );
    fs::write(fixture.root.join("spice.toml"), &document).expect("Claude config");
    fs::write(codex_worktree.join("spice.toml"), document).expect("Codex config");
    Rig {
        codex_worktree,
        capture_index,
        claude_binary,
        codex_binary,
    }
}

fn write_vendor(binary: &Path, captures: &Path, index: &Path, dialect: Dialect) {
    fs::write(
        binary,
        format!(
            concat!(
                "#!/bin/sh\n",
                "base={}/{}-$$\n",
                "printf '%s\\0' \"$0\" \"$@\" > \"$base.argv\"\n",
                "/usr/bin/env -0 > \"$base.env\"\n",
                "pwd -P > \"$base.cwd\"\n",
                "printf '%s\\n' \"$base\" >> {}\n",
                "trap 'exit 0' TERM INT\n",
                "printf '%s\\n' {}\n",
                "sleep 600 &\n",
                "child=$!\n",
                "printf '%s\\n' \"$child\" > \"$base.child\"\n",
                "wait \"$child\"\n",
            ),
            shell_word(&captures.display().to_string()),
            dialect.name(),
            shell_word(&index.display().to_string()),
            shell_word(dialect.session_record()),
        ),
    )
    .expect("fixture vendor writes");
    #[cfg(unix)]
    fs::set_permissions(binary, fs::Permissions::from_mode(0o700))
        .expect("fixture vendor becomes executable");
}

fn add_lane(fixture: &Fixture, worktree: Option<&Path>, dialect: Dialect) -> wire::CanonicalUuid {
    add_lane_with(
        fixture,
        Path::new(env!("CARGO_BIN_EXE_spice")),
        worktree,
        dialect,
    )
}

fn add_lane_with(
    fixture: &Fixture,
    executable: &Path,
    worktree: Option<&Path>,
    dialect: Dialect,
) -> wire::CanonicalUuid {
    let worktree = worktree.map(|path| path.display().to_string());
    let mut arguments = vec!["lane", "add"];
    if let Some(worktree) = worktree.as_deref() {
        arguments.push(worktree);
    }
    arguments.extend(["--vendor", dialect.name()]);
    let answer = reply_body::<wire::intent::LaneChangeAnswer>(&operator_with(
        fixture, executable, &arguments, true,
    ));
    answer.lane
}

fn drain(fixture: &Fixture, lane: wire::CanonicalUuid) {
    drain_with(fixture, Path::new(env!("CARGO_BIN_EXE_spice")), lane);
}

fn drain_with(fixture: &Fixture, executable: &Path, lane: wire::CanonicalUuid) {
    let output = operator_with(
        fixture,
        executable,
        &["lane", "drain", &lane.to_string()],
        true,
    );
    assert!(output.status.success(), "drain failed: {:?}", output.stderr);
}

fn wake(fixture: &Fixture, lane: wire::CanonicalUuid) {
    let output = operator(fixture, &["lane", "wake", &lane.to_string()], true);
    assert!(output.status.success(), "wake failed: {:?}", output.stderr);
}

fn operator(fixture: &Fixture, arguments: &[&str], json: bool) -> Output {
    operator_with(
        fixture,
        Path::new(env!("CARGO_BIN_EXE_spice")),
        arguments,
        json,
    )
}

fn operator_with(fixture: &Fixture, executable: &Path, arguments: &[&str], json: bool) -> Output {
    let mut process = Command::new(executable);
    process.env_remove(spice::agent_launch::SPICE_LANE);
    if json {
        process.arg("--json");
    }
    process
        .args(arguments)
        .current_dir(&fixture.root)
        .output()
        .expect("operator command launches")
}

fn operator_with_path(
    fixture: &Fixture,
    executable: &Path,
    path: &Path,
    arguments: &[&str],
    json: bool,
) -> Output {
    let mut process = Command::new(executable);
    process
        .env_remove(spice::agent_launch::SPICE_LANE)
        .env("PATH", path);
    if json {
        process.arg("--json");
    }
    process
        .args(arguments)
        .current_dir(&fixture.root)
        .output()
        .expect("PATH-confined operator command launches")
}

fn lane_command(
    fixture: &Fixture,
    executable: &Path,
    lane: wire::CanonicalUuid,
    path: &Path,
    arguments: &[&str],
    json: bool,
) -> Output {
    let mut process = Command::new(executable);
    process
        .env(spice::agent_launch::SPICE_LANE, lane.to_string())
        .env("PATH", path);
    if json {
        process.arg("--json");
    }
    process
        .args(arguments)
        .current_dir(&fixture.root)
        .output()
        .expect("lane command launches")
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

fn assert_launches(
    fixture: &Fixture,
    rig: &Rig,
    lanes: &[(Dialect, wire::CanonicalUuid); 2],
    resumed: bool,
) {
    assert_launches_with_executable(
        fixture,
        rig,
        lanes,
        resumed,
        Path::new(env!("CARGO_BIN_EXE_spice")),
        None,
    );
}

fn assert_launches_with_executable(
    fixture: &Fixture,
    rig: &Rig,
    lanes: &[(Dialect, wire::CanonicalUuid); 2],
    resumed: bool,
    executable: &Path,
    expected_path: Option<&Path>,
) {
    let head = durable_head(fixture);
    for (dialect, lane) in lanes {
        let row = head
            .lanes
            .iter()
            .find(|row| row.lane == *lane)
            .expect("lane row exists");
        let session = row.session.as_ref().expect("session authority exists");
        assert_eq!(session.identity, dialect.session_id());
        assert_eq!(session.stream, "stdout");
        assert_eq!(session.vendor.as_str(), dialect.name());

        let captures = captures(&rig.capture_index, *dialect);
        let expected_count = if resumed { 2 } else { 1 };
        assert_eq!(captures.len(), expected_count, "capture index drifted");
        let capture = CapturedLaunch::read(
            captures
                .last()
                .expect("selected vendor launch was captured"),
        );
        assert_eq!(capture.cwd, row.worktree);
        assert_environment(&capture.environment, *dialect, *lane, expected_path);
        assert_eq!(
            capture.argv,
            expected_argv(rig, *dialect, *lane, &row.worktree, resumed, executable)
        );
    }
}

fn durable_head(fixture: &Fixture) -> wire::ResidencyHead {
    journal::JournalReader::open(fixture.journal())
        .expect("resident journal reads")
        .read(|read| read.residency_head())
        .expect("residency reads")
        .expect("residency exists")
        .into_body()
}

fn captures(index: &Path, dialect: Dialect) -> Vec<PathBuf> {
    fs::read_to_string(index)
        .expect("launch index reads")
        .lines()
        .map(PathBuf::from)
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&format!("{}-", dialect.name())))
        })
        .collect()
}

fn assert_processes_stopped(index: &Path, expected_per_vendor: usize) {
    for dialect in [Dialect::Claude, Dialect::Codex] {
        let launches = captures(index, dialect);
        assert_eq!(launches.len(), expected_per_vendor);
        for launch in launches {
            let wrapper = launch
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.rsplit_once('-'))
                .and_then(|(_, pid)| pid.parse::<u32>().ok())
                .expect("capture name carries wrapper pid");
            let child = fs::read_to_string(launch.with_extension("child"))
                .expect("child pid reads")
                .trim()
                .parse::<u32>()
                .expect("child pid parses");
            assert!(!process_alive(wrapper), "vendor wrapper survived drain");
            assert!(!process_alive(child), "vendor child group survived drain");
        }
    }
}

fn binary(rig: &Rig, dialect: Dialect) -> &Path {
    match dialect {
        Dialect::Claude => &rig.claude_binary,
        Dialect::Codex => &rig.codex_binary,
    }
}

impl CapturedLaunch {
    fn read(base: &Path) -> Self {
        let argv = nul_fields(&fs::read(base.with_extension("argv")).expect("argv capture reads"));
        let environment =
            nul_fields(&fs::read(base.with_extension("env")).expect("environment capture reads"))
                .into_iter()
                .map(|entry| {
                    let (name, value) = entry.split_once('=').unwrap_or_else(|| {
                        panic!("captured environment entry has no equals: {entry:?}")
                    });
                    (name.to_owned(), value.to_owned())
                })
                .collect();
        let cwd = fs::read_to_string(base.with_extension("cwd"))
            .expect("cwd capture reads")
            .trim_end()
            .to_owned();
        Self {
            argv,
            environment,
            cwd,
        }
    }
}

fn nul_fields(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .map(|field| String::from_utf8(field.to_vec()).expect("fixture capture is UTF-8"))
        .collect()
}

#[allow(
    clippy::disallowed_methods,
    reason = "the integration fixture observes the same five ambient names at the resident \
              binary rim"
)]
fn assert_environment(
    captured: &BTreeMap<String, String>,
    dialect: Dialect,
    lane: wire::CanonicalUuid,
    expected_path: Option<&Path>,
) {
    let mut observed = captured.clone();
    for shell_owned in ["PWD", "SHLVL", "_"] {
        observed.remove(shell_owned);
    }
    if let Some(count) = observed.remove("GIT_CONFIG_COUNT") {
        let count = count
            .parse::<usize>()
            .expect("Git command-scope entry count is numeric");
        let system = observed
            .remove("GIT_CONFIG_SYSTEM")
            .expect("Git shadow names its system config");
        assert!(
            system.ends_with("/agent.gitconfig"),
            "unexpected Git shadow path: {system}"
        );
        for index in 0..count {
            let key = observed
                .remove(&format!("GIT_CONFIG_KEY_{index}"))
                .expect("Git command-scope key exists");
            let value = observed
                .remove(&format!("GIT_CONFIG_VALUE_{index}"))
                .expect("Git command-scope value exists");
            assert!(key.starts_with("branch."));
            assert!(key.ends_with(".merge") || key.ends_with(".remote"));
            assert!(!value.is_empty());
        }
    } else {
        assert!(!observed.contains_key("GIT_CONFIG_SYSTEM"));
    }
    let mut expected = ["HOME", "PATH", "SHELL", "TMPDIR", "USER"]
        .into_iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| (name.to_owned(), value))
        })
        .collect::<BTreeMap<_, _>>();
    if let Some(path) = expected_path {
        expected.insert("PATH".to_owned(), path.display().to_string());
    }
    expected.insert(spice::agent_launch::SPICE_LANE.to_owned(), lane.to_string());
    if dialect == Dialect::Claude {
        expected.insert(
            spice::agent_launch::CLAUDE_AUTO_COMPACT_WINDOW.to_owned(),
            COMPACT_WINDOW.to_owned(),
        );
    }
    assert_eq!(observed, expected, "ambient child environment leaked");
}

fn expected_argv(
    rig: &Rig,
    dialect: Dialect,
    lane: wire::CanonicalUuid,
    worktree: &str,
    resumed: bool,
    executable: &Path,
) -> Vec<String> {
    match dialect {
        Dialect::Claude => expected_claude_argv(rig, lane, resumed, executable),
        Dialect::Codex => expected_codex_argv(rig, worktree, resumed, executable),
    }
}

fn expected_claude_argv(
    rig: &Rig,
    lane: wire::CanonicalUuid,
    resumed: bool,
    executable: &Path,
) -> Vec<String> {
    let executable = executable.display().to_string();
    let hook = format!("{} inbox", shell_word(&executable));
    let mcp = format!(
        r#"{{"mcpServers":{{"spice":{{"command":{},"args":["mcp"]}}}}}}"#,
        json_string(&executable),
    );
    let settings = format!(
        concat!(
            r#"{{"attribution":{{"commit":"","sessionUrl":false}},"#,
            r#""hooks":{{"PostToolUse":[{{"hooks":[{{"command":{},"#,
            r#""statusMessage":"Checking spice steering","timeout":30,"type":"command"}}],"#,
            r#""matcher":"*"}}]}},"permissions":{{"deny":["Task","Agent","TaskCreate","#,
            r#""TaskGet","TaskList","TaskUpdate","TaskOutput","TaskStop","Monitor"]}}}}"#,
        ),
        json_string(&hook),
    );
    let key = vendor::SteeringKey {
        token: lane.to_string(),
    };
    let system = format!(
        "{}\n\n{PROMPT}\n\n{}",
        vendor::claude::SKILL_SYSTEM_PROMPT_PREAMBLE,
        key.prompt_line()
    );
    let mut argv = vec![
        rig.claude_binary.display().to_string(),
        "--print".to_owned(),
        "--output-format".to_owned(),
        "stream-json".to_owned(),
        "--verbose".to_owned(),
        "--include-partial-messages".to_owned(),
        "--model".to_owned(),
        CLAUDE_MODEL.to_owned(),
        "--permission-mode".to_owned(),
        "bypassPermissions".to_owned(),
        "--mcp-config".to_owned(),
        mcp,
        "--settings".to_owned(),
        settings,
        "--append-system-prompt".to_owned(),
        system.clone(),
        "--effort".to_owned(),
        "xhigh".to_owned(),
    ];
    if resumed {
        argv.extend(["--resume".to_owned(), CLAUDE_SESSION_ID.to_owned()]);
    }
    argv.push(system);
    argv
}

fn expected_codex_argv(rig: &Rig, worktree: &str, resumed: bool, executable: &Path) -> Vec<String> {
    let executable = executable.display().to_string();
    let hook = format!("{} inbox", shell_word(&executable));
    let hook_override = format!(
        concat!(
            r#"hooks.PostToolUse=[{{matcher="^(Bash|apply_patch|Edit|Write|mcp__.*)$","#,
            r#"hooks=[{{type="command",command={},timeout=30,"#,
            r#"statusMessage="Checking spice steering"}}]}}]"#,
        ),
        json_string(&hook),
    );
    let mut argv = vec![
        rig.codex_binary.display().to_string(),
        "exec".to_owned(),
        "--cd".to_owned(),
        worktree.to_owned(),
        "--model".to_owned(),
        CODEX_MODEL.to_owned(),
        "--json".to_owned(),
        "--config".to_owned(),
        hook_override,
        "--config".to_owned(),
        "model_reasoning_effort=\"max\"".to_owned(),
        "--config".to_owned(),
        format!("mcp_servers.spice.command={}", json_string(&executable)),
        "--config".to_owned(),
        "mcp_servers.spice.args=[\"mcp\"]".to_owned(),
        "--config".to_owned(),
        format!("personality={}", json_string(CODEX_PERSONALITY)),
        "--enable".to_owned(),
        "fast_mode".to_owned(),
        "--sandbox".to_owned(),
        "danger-full-access".to_owned(),
        "--dangerously-bypass-approvals-and-sandbox".to_owned(),
        "--dangerously-bypass-hook-trust".to_owned(),
    ];
    if resumed {
        argv.extend(["resume".to_owned(), CODEX_SESSION_ID.to_owned()]);
    }
    argv.push(PROMPT.to_owned());
    argv
}

fn assert_tapes(fixture: &Fixture, lanes: &[(Dialect, wire::CanonicalUuid); 2]) {
    for (dialect, lane) in lanes {
        let path = fixture
            .runtime
            .join("tape")
            .join(lane.to_string())
            .join(dialect.name());
        let tape = tape::Tape::open_existing(path).expect("vendor tape opens");
        let extents = tape.extents().expect("vendor tape extents read");
        assert_eq!(extents.len(), 1, "one append-only tape member is preserved");
        let bytes = tape
            .read(tape::Locus::new(extents[0].member, 0, extents[0].length))
            .expect("vendor tape reads");
        assert_eq!(
            bytes,
            format!("{0}\n{0}\n", dialect.session_record()).as_bytes()
        );
        let text = String::from_utf8(bytes).expect("fixture tape is UTF-8");
        let other = if *dialect == Dialect::Claude {
            CODEX_SESSION_ID
        } else {
            CLAUDE_SESSION_ID
        };
        assert!(!text.contains(other));
        assert!(!text.contains(&lane.to_string()));
    }
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("fixture string encodes")
}

fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn fixture_git(root: &Path, arguments: &[&str]) {
    let output = native_git_command(root, arguments)
        .output()
        .expect("fixture Git launches");
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr),
    );
}

fn fixture_git_stdout_with_environment(
    root: &Path,
    arguments: &[&str],
    environment: &BTreeMap<String, String>,
) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env_clear()
        .envs(environment)
        .output()
        .expect("shadowed fixture Git launches");
    assert!(
        output.status.success(),
        "shadowed git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout)
        .expect("shadowed Git output is UTF-8")
        .trim()
        .to_owned()
}

fn native_git_command(root: &Path, arguments: &[&str]) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(arguments);
    command
        .env_remove("GIT_CONFIG_SYSTEM")
        .env_remove("GIT_CONFIG_COUNT");
    for index in 0..8 {
        command
            .env_remove(format!("GIT_CONFIG_KEY_{index}"))
            .env_remove(format!("GIT_CONFIG_VALUE_{index}"));
    }
    command
}
