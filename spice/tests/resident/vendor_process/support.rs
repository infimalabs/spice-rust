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

fn lane_with(
    fixture: &Fixture,
    executable: &Path,
    lane: wire::CanonicalUuid,
    arguments: &[&str],
    json: bool,
) -> Output {
    let mut process = Command::new(executable);
    process.env(spice::agent_launch::SPICE_LANE, lane.to_string());
    if json {
        process.arg("--json");
    }
    process
        .args(arguments)
        .current_dir(&fixture.root)
        .output()
        .expect("lane command launches")
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

/// How many durable facts of one kind name this lane.
///
/// Shared by the ending tests rather than copied into each. Every one of them
/// has to assert a presence and an absence for the same lane, so a copy that
/// drifted would not read as a gap; it would quietly weaken whichever side of
/// whichever test it was copied into.
fn lane_facts(fixture: &Fixture, lane: wire::CanonicalUuid, kind: FactKind) -> usize {
    journal::JournalReader::open(fixture.journal())
        .expect("lane journal reads")
        .fact_snapshot()
        .expect("lane facts read")
        .facts()
        .iter()
        .filter(|fact| {
            fact.fact.kind() == kind
                && fact.fact.body().get("lane")
                    == Some(&serde_json::Value::String(lane.to_string()))
        })
        .count()
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
    let repository = spice::repository::Repository::discover(worktree)
        .expect("expected launch worktree discovers");
    let prompt = format!(
        "[$spice](<{}>)",
        spice::install::installed_skill(&repository).display()
    );
    match dialect {
        Dialect::Claude => expected_claude_argv(rig, lane, resumed, executable, &prompt),
        Dialect::Codex => expected_codex_argv(rig, worktree, resumed, executable, &prompt),
    }
}

fn expected_claude_argv(
    rig: &Rig,
    lane: wire::CanonicalUuid,
    resumed: bool,
    executable: &Path,
    prompt: &str,
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
        "{}\n\n{prompt}\n\n{}",
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

fn expected_codex_argv(
    rig: &Rig,
    worktree: &str,
    resumed: bool,
    executable: &Path,
    prompt: &str,
) -> Vec<String> {
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
    argv.push(prompt.to_owned());
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
