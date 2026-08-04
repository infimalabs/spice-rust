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
    fs::write(
        &rig.codex_binary,
        format!(
            concat!(
                "#!/bin/sh\n",
                "trap 'printf stopped > {}; exit 0' TERM INT\n",
                "printf '%s\\n' '{{\"type\":\"thread.started\"}}'\n",
                "while :; do sleep 1; done\n",
            ),
            shell_word(&stopped.display().to_string()),
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
    assert_eq!(
        fs::read_to_string(&stopped).expect("malformed vendor stopped"),
        "stopped"
    );
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
    let worktree = worktree.map(|path| path.display().to_string());
    let mut arguments = vec!["lane", "add"];
    if let Some(worktree) = worktree.as_deref() {
        arguments.push(worktree);
    }
    arguments.extend(["--vendor", dialect.name()]);
    let answer = reply_body::<wire::intent::LaneChangeAnswer>(&operator(fixture, &arguments, true));
    answer.lane
}

fn drain(fixture: &Fixture, lane: wire::CanonicalUuid) {
    let output = operator(fixture, &["lane", "drain", &lane.to_string()], true);
    assert!(output.status.success(), "drain failed: {:?}", output.stderr);
}

fn wake(fixture: &Fixture, lane: wire::CanonicalUuid) {
    let output = operator(fixture, &["lane", "wake", &lane.to_string()], true);
    assert!(output.status.success(), "wake failed: {:?}", output.stderr);
}

fn operator(fixture: &Fixture, arguments: &[&str], json: bool) -> Output {
    let mut process = Command::new(env!("CARGO_BIN_EXE_spice"));
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
        assert_environment(&capture.environment, *dialect, *lane);
        assert_eq!(
            capture.argv,
            expected_argv(rig, *dialect, *lane, &row.worktree, resumed)
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

#[cfg(target_os = "linux")]
fn process_alive(pid: u32) -> bool {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat
            .rsplit_once(") ")
            .and_then(|(_, fields)| fields.as_bytes().first().copied())
            .is_none_or(|state| !matches!(state, b'Z' | b'X' | b'x')),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => process_identity_exists(pid),
    }
}

#[cfg(not(target_os = "linux"))]
fn process_alive(pid: u32) -> bool {
    process_identity_exists(pid)
}

fn process_identity_exists(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .expect("process probe launches")
        .status
        .success()
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
) {
    let mut observed = captured.clone();
    for shell_owned in ["PWD", "SHLVL", "_"] {
        observed.remove(shell_owned);
    }
    let mut expected = ["HOME", "PATH", "SHELL", "TMPDIR", "USER"]
        .into_iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| (name.to_owned(), value))
        })
        .collect::<BTreeMap<_, _>>();
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
) -> Vec<String> {
    match dialect {
        Dialect::Claude => expected_claude_argv(rig, lane, resumed),
        Dialect::Codex => expected_codex_argv(rig, worktree, resumed),
    }
}

fn expected_claude_argv(rig: &Rig, lane: wire::CanonicalUuid, resumed: bool) -> Vec<String> {
    let executable = env!("CARGO_BIN_EXE_spice");
    let hook = format!("{} inbox", shell_word(executable));
    let mcp = format!(
        r#"{{"mcpServers":{{"spice":{{"command":{},"args":["mcp"]}}}}}}"#,
        json_string(executable),
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

fn expected_codex_argv(rig: &Rig, worktree: &str, resumed: bool) -> Vec<String> {
    let executable = env!("CARGO_BIN_EXE_spice");
    let hook = format!("{} inbox", shell_word(executable));
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
        format!("mcp_servers.spice.command={}", json_string(executable)),
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
