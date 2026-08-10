use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    path: PathBuf,
    digest: String,
    length: u64,
    mode: Option<u32>,
}

impl FileIdentity {
    fn read(path: &Path) -> Self {
        let bytes = fs::read(path).unwrap_or_else(|error| {
            panic!("read predecessor identity {}: {error}", path.display())
        });
        let metadata = fs::metadata(path).unwrap_or_else(|error| {
            panic!("inspect predecessor identity {}: {error}", path.display())
        });
        #[cfg(unix)]
        let mode = Some(metadata.permissions().mode() & 0o777);
        #[cfg(not(unix))]
        let mode = None;
        Self {
            path: path.to_path_buf(),
            digest: wire::Digest::compute(wire::Domain::DistArtifact, &bytes).to_string(),
            length: metadata.len(),
            mode,
        }
    }
}

struct PredecessorSystem {
    executable: FileIdentity,
    invocations: PathBuf,
    python_state_root: PathBuf,
    state: Vec<FileIdentity>,
    hooks: Vec<FileIdentity>,
    hook_config: FileIdentity,
    worktree: PathBuf,
    process: Child,
    pid: u32,
}

impl PredecessorSystem {
    fn install(fixture: &Fixture) -> Self {
        let worktree = install_predecessor_worktree(fixture);
        let hook_paths = install_predecessor_hooks(&worktree);
        let (executable_path, invocations) = install_predecessor_executable(fixture);
        let (state_paths, hook_config) = install_predecessor_state(fixture, &worktree);
        let process = Command::new(ambient_executable("sleep"))
            .arg("600")
            .spawn()
            .expect("start predecessor process");
        let pid = process.id();
        let state = state_paths
            .iter()
            .map(|path| FileIdentity::read(path))
            .collect();
        Self {
            executable: FileIdentity::read(&executable_path),
            invocations,
            python_state_root: fixture.root.join(".git/.spice"),
            state,
            hooks: hook_paths
                .iter()
                .map(|path| FileIdentity::read(path))
                .collect(),
            hook_config: FileIdentity::read(&hook_config),
            worktree,
            process,
            pid,
        }
    }

    /// Give Python Spice the exact UUID Rust Spice assigned to a live lane.
    ///
    /// Python's task authority and filesystem inbox remain under `.spice`; the
    /// Rust lane and its directive stream remain under `.spice-rs`. The
    /// identifier is deliberately equal while every route remains distinct.
    fn seed_same_agent_uuid(&mut self, lane: wire::CanonicalUuid) -> String {
        let task_authority = self.python_state_root.join("data/taskchampion.sqlite3");
        fs::write(&task_authority, format!("predecessor-agent-uuid={lane}\n"))
            .expect("seed Python agent UUID");
        let identity = self
            .state
            .iter_mut()
            .find(|identity| identity.path == task_authority)
            .expect("Python task authority is tracked");
        *identity = FileIdentity::read(&task_authority);

        let key = lane.to_string();
        let inbox = self.worktree.join(format!(".spice/inbox/{key}.txt"));
        fs::write(
            &inbox,
            format!("Python-only steering for same agent UUID {lane}\n"),
        )
        .expect("seed Python same-UUID inbox item");
        self.state.push(FileIdentity::read(&inbox));
        key
    }

    fn assert_unchanged(&mut self, verb: &str) {
        assert_eq!(
            FileIdentity::read(&self.executable.path),
            self.executable,
            "{verb} changed the predecessor executable"
        );
        assert!(
            !self.invocations.exists(),
            "{verb} routed an agent-shell command through the predecessor executable"
        );
        assert_eq!(
            self.state
                .iter()
                .map(|identity| FileIdentity::read(&identity.path))
                .collect::<Vec<_>>(),
            self.state,
            "{verb} changed predecessor durable state or authority"
        );
        assert_eq!(
            self.hooks
                .iter()
                .map(|identity| FileIdentity::read(&identity.path))
                .collect::<Vec<_>>(),
            self.hooks,
            "{verb} changed predecessor hook authority"
        );
        assert_eq!(
            FileIdentity::read(&self.hook_config.path),
            self.hook_config,
            "{verb} changed the predecessor core.hooksPath authority"
        );
        assert_eq!(
            fixture_git_output(&self.worktree, &["config", "--get", "core.hooksPath"]),
            ".spice/hooks",
            "{verb} stopped resolving predecessor steering hooks"
        );
        // `Child::id` reports the pid recorded at spawn and keeps reporting it
        // after the process is killed and reaped, so comparing it with the pid
        // captured from that same `Child` cannot fail and proved nothing about
        // the predecessor. Asking the operating system whether the child has
        // exited is the check that can, and it is the only one here that does.
        assert!(
            self.process
                .try_wait()
                .expect("inspect predecessor process")
                .is_none(),
            "{verb} stopped the predecessor process (pid {})",
            self.pid
        );
    }
}

impl Drop for PredecessorSystem {
    fn drop(&mut self) {
        if self.process.try_wait().is_ok_and(|status| status.is_none()) {
            self.process
                .kill()
                .expect("stop predecessor fixture process");
            self.process
                .wait()
                .expect("reap predecessor fixture process");
        }
    }
}

fn install_predecessor_worktree(fixture: &Fixture) -> PathBuf {
    let worktree = fixture.cleanup.join("predecessor-worktree");
    fixture_git(
        &fixture.root,
        &["config", "extensions.worktreeConfig", "true"],
    );
    fixture_git(
        &fixture.root,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "predecessor",
            worktree.to_str().expect("predecessor path is UTF-8"),
            "HEAD",
        ],
    );
    worktree
}

fn install_predecessor_hooks(worktree: &Path) -> Vec<PathBuf> {
    let hooks_root = worktree.join(".spice/hooks");
    let mut paths = Vec::new();
    for hook in ["pre-commit", "commit-msg", "reference-transaction"] {
        let path = hooks_root.join(hook);
        fs::create_dir_all(path.parent().expect("predecessor hook has a parent"))
            .expect("create predecessor hooks");
        fs::write(&path, format!("#!/bin/sh\nexec spice dev {hook} \"$@\"\n"))
            .expect("write predecessor hook");
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("make predecessor hook executable");
        paths.push(path);
    }
    fixture_git(
        worktree,
        &["config", "--worktree", "core.hooksPath", ".spice/hooks"],
    );
    paths
}

fn install_predecessor_executable(fixture: &Fixture) -> (PathBuf, PathBuf) {
    let executable = fixture.cleanup.join("predecessor-bin/spice");
    let invocations = fixture.cleanup.join("predecessor-bin/invocations");
    fs::create_dir_all(executable.parent().expect("predecessor bin has a parent"))
        .expect("create predecessor bin");
    fs::write(
        &executable,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\necho 'Spice 0.31.0'\n",
            shell_word(&invocations.display().to_string()),
        ),
    )
    .expect("write predecessor executable");
    #[cfg(unix)]
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
        .expect("make predecessor executable");
    (executable, invocations)
}

fn install_predecessor_state(fixture: &Fixture, worktree: &Path) -> (Vec<PathBuf>, PathBuf) {
    let data = fixture.root.join(".git/.spice/data");
    fs::create_dir_all(&data).expect("create Python state directory");
    let mut paths = [
        "spiceacks.sqlite3",
        "spicemaxims.sqlite3",
        "spiceprojections.sqlite3",
        "spiceteams.sqlite3",
        "taskchampion.sqlite3",
    ]
    .map(|name| data.join(name))
    .to_vec();
    for (index, path) in paths.iter().enumerate() {
        fs::write(path, format!("predecessor-authority-{index}\n"))
            .expect("write predecessor state");
    }

    let git_pointer =
        fs::read_to_string(worktree.join(".git")).expect("read predecessor worktree Git pointer");
    let hook_config = PathBuf::from(
        git_pointer
            .trim()
            .strip_prefix("gitdir: ")
            .expect("predecessor worktree names its Git directory"),
    )
    .join("config.worktree");
    let predecessor_git = hook_config
        .parent()
        .expect("predecessor worktree config has a Git directory");
    let context = [
        predecessor_git.join(".spice/agents/thread-id"),
        predecessor_git.join(".spice/agents/stderr.sock"),
        worktree.join(".spice/inbox/1jN54zJJ.txt"),
    ];
    for (path, bytes) in context.iter().zip([
        b"python-thread-identity\n".as_slice(),
        b"/tmp/spice-agent-side-python.sock\n".as_slice(),
        b"Python-only pending steering\n".as_slice(),
    ]) {
        fs::create_dir_all(path.parent().expect("predecessor context has a parent"))
            .expect("create predecessor context directory");
        fs::write(path, bytes).expect("write predecessor context state");
    }
    paths.extend(context);
    (paths, hook_config)
}

#[test]
fn installed_spice_lifecycle_leaves_the_predecessor_system_untouched() {
    let fixture = Fixture::new("coexisting-native-lifecycle");
    let rig = configure(&fixture);
    prepare_coexistence_repository(&fixture, &rig);
    let mut predecessor = PredecessorSystem::install(&fixture);
    let (installed_binary, plan_digest, binary_digest) = install_coexisting_appliance(&fixture);
    predecessor.assert_unchanged("install");

    let (resident_path, resident_tools) =
        native_resident_path(&fixture, &predecessor.executable.path);
    let mut resident =
        RunningResident::spawn_with_path(&fixture, &installed_binary, &resident_path);
    resident.assert_ready(&fixture);
    predecessor.assert_unchanged("start");
    let claude = add_lane_with(&fixture, &installed_binary, None, Dialect::Claude);
    let codex = add_lane_with(
        &fixture,
        &installed_binary,
        Some(&rig.codex_worktree),
        Dialect::Codex,
    );
    let lanes = [(Dialect::Claude, claude), (Dialect::Codex, codex)];
    let python_same_uuid_key = predecessor.seed_same_agent_uuid(claude);
    let created = add_coexistence_tasks(&fixture, &installed_binary, &lanes);
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
    assert_coexistence_allocation(&fixture, &installed_binary, &empty_path, &lanes, &created);
    let directives = assert_coexistence_steering(
        &fixture,
        &rig,
        &installed_binary,
        &empty_path,
        &lanes,
        &python_same_uuid_key,
    );
    exercise_installed_surface(&fixture, &rig, &lanes);
    predecessor.assert_unchanged("agent shell routing");
    stop_coexisting_appliance(
        &fixture,
        &rig,
        &installed_binary,
        &empty_path,
        &lanes,
        &directives,
        resident,
    );
    predecessor.assert_unchanged("stop");
    let upgrade_digest = upgrade_coexisting_appliance(&fixture, &mut predecessor);
    predecessor.assert_unchanged("upgrade");
    eprintln!(
        "coexistence-evidence install={plan_digest} upgrade={upgrade_digest} \
         binary={binary_digest} journal={} tasks=[{},{}] \
         lanes=[{claude},{codex}] directives=[{},{}] vendor_logs=2 \
         same_uuid={python_same_uuid_key} predecessor_pid={} predecessor_files={} \
         predecessor_context=[thread,inbox,socket-marker,same-uuid-inbox] \
         predecessor_hooks=.spice/hooks resident_path=[git:{},sleep:{},spice:predecessor] \
         agent_path_head={} client_path_entries=0 shutdown=clean",
        fixture.journal().display(),
        created[0],
        created[1],
        directives[0].key,
        directives[1].key,
        predecessor.pid,
        predecessor.state.len() + 1,
        resident_tools[0].display(),
        resident_tools[1].display(),
        installed_binary
            .parent()
            .expect("installed appliance has a directory")
            .display(),
    );
}

fn prepare_coexistence_repository(fixture: &Fixture, rig: &Rig) {
    let mut document =
        fs::read_to_string(fixture.root.join("spice.toml")).expect("fixture config reads");
    document.push_str("\n[tasks]\nstems = [\"coexistence\"]\n");
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
                "predecessor-bin/\n",
                "predecessor-worktree/\n",
                "upgraded-spice\n",
                "{}",
            ),
            app_stage_ignore()
        ),
    )
    .expect("coexistence fixture ignore policy writes");
    fixture_git(&fixture.root, &["init", "-b", "main"]);
    fixture_git(&fixture.root, &["config", "user.name", "Spice Coexistence"]);
    fixture_git(
        &fixture.root,
        &["config", "user.email", "coexistence@spice.invalid"],
    );
    fixture_git(&fixture.root, &["add", ".gitignore", "spice.toml"]);
    fixture_git(&fixture.root, &["commit", "-m", "coexistence fixture"]);
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
            "coexistence-codex",
            &codex_worktree,
            "HEAD",
        ],
    );
}

fn install_coexisting_appliance(fixture: &Fixture) -> (PathBuf, String, String) {
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

fn upgrade_coexisting_appliance(fixture: &Fixture, predecessor: &mut PredecessorSystem) -> String {
    let repository = spice::repository::Repository::discover(&fixture.root)
        .expect("coexistence repository discovers for upgrade");
    let source = Path::new(env!("CARGO_BIN_EXE_spice"));
    let upgraded = fixture.cleanup.join("upgraded-spice");
    fs::copy(source, &upgraded).expect("copy changed upgrade binary");
    fs::OpenOptions::new()
        .append(true)
        .open(&upgraded)
        .expect("open changed upgrade binary")
        .write_all(b"\n")
        .expect("change upgrade binary identity");
    let removed =
        spice::install::unapply(&repository).expect("reverse installed build for upgrade");
    assert_eq!(removed.status(), spice::install::InstallStatus::Uninstalled);
    predecessor.assert_unchanged("upgrade reversal");
    let plan = spice::install::plan(&repository, Some(&upgraded)).expect("upgrade plans");
    let digest = plan.digest().expect("upgrade plan digests");
    let receipt = spice::install::apply(&repository, Some(&upgraded), &digest)
        .expect("upgrade applies exactly");
    assert_eq!(receipt.status(), spice::install::InstallStatus::Complete);
    assert_eq!(
        fs::read(spice::install::installed_binary(&repository))
            .expect("upgraded installed binary reads"),
        fs::read(&upgraded).expect("upgrade source reads")
    );
    digest.to_string()
}

fn add_coexistence_tasks(
    fixture: &Fixture,
    installed_binary: &Path,
    lanes: &[(Dialect, wire::CanonicalUuid); 2],
) -> [wire::TaskHandle; 2] {
    // Both fields are deliberate: acceptance satisfies the task lifecycle,
    // while origin proves Rust never borrowed the predecessor's authority.
    [
        reply_body::<wire::intent::TaskAnswer>(&lane_with(
            fixture,
            installed_binary,
            lanes[0].1,
            &[
                "task",
                "add",
                "Fresh Claude coexistence work",
                "--project",
                "coexistence.rehearsal",
                "--acceptance",
                "coexistence fixture reaches clean resident shutdown",
                "--origin",
                "fixture:coexistence-rehearsal",
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
                "Fresh Codex coexistence work",
                "--project",
                "coexistence.rehearsal",
                "--acceptance",
                "coexistence fixture reaches clean resident shutdown",
                "--origin",
                "fixture:coexistence-rehearsal",
            ],
            true,
        ))
        .task,
    ]
}

fn assert_coexistence_allocation(
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

fn assert_coexistence_steering(
    fixture: &Fixture,
    rig: &Rig,
    installed_binary: &Path,
    empty_path: &Path,
    lanes: &[(Dialect, wire::CanonicalUuid); 2],
    python_same_uuid_key: &str,
) -> [wire::intent::DirectiveAnswer; 2] {
    let [(_, claude), (_, codex)] = *lanes;
    let claude_direction = reply_body::<wire::intent::DirectiveAnswer>(&operator_with_path(
        fixture,
        installed_binary,
        empty_path,
        &[
            "steer",
            &claude.to_string(),
            "Claude-only coexistence direction",
        ],
        true,
    ));
    let codex_direction = reply_body::<wire::intent::DirectiveAnswer>(&operator_with_path(
        fixture,
        installed_binary,
        empty_path,
        &[
            "steer",
            &codex.to_string(),
            "Codex-only coexistence direction",
        ],
        true,
    ));
    for (dialect, own, foreign) in [
        (Dialect::Claude, &claude_direction.key, &codex_direction.key),
        (Dialect::Codex, &codex_direction.key, &claude_direction.key),
    ] {
        let inbox = reply_body::<wire::intent::InboxAnswer>(&agent_shell_command(
            fixture,
            rig,
            dialect,
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
        assert!(
            !keys.contains(&python_same_uuid_key),
            "Rust inbox consumed the Python inbox item for the same agent UUID"
        );

        let crossed = agent_shell_command(fixture, rig, dialect, &["ack", foreign], true);
        assert_eq!(crossed.status.code(), Some(REFUSAL_EXIT));
        let Outcome::Refusal(refusal) = value_outcome(&crossed) else {
            panic!("foreign acknowledgment must refuse")
        };
        assert_eq!(refusal.reason(), "steer::transcript-evidence-absent");

        let after = reply_body::<wire::intent::InboxAnswer>(&agent_shell_command(
            fixture,
            rig,
            dialect,
            &["inbox"],
            true,
        ));
        let after_keys = after.lanes[0]
            .pending
            .iter()
            .map(|pending| pending.key.as_str())
            .collect::<Vec<_>>();
        assert!(after_keys.contains(&own.as_str()));
        assert!(!after_keys.contains(&foreign.as_str()));
        assert!(!after_keys.contains(&python_same_uuid_key));
    }

    let python_ack = agent_shell_command(
        fixture,
        rig,
        Dialect::Claude,
        &["ack", python_same_uuid_key],
        true,
    );
    assert_eq!(
        python_ack.status.code(),
        Some(REFUSAL_EXIT),
        "Rust ACK accepted a Python inbox key for the same agent UUID"
    );
    [claude_direction, codex_direction]
}

fn exercise_installed_surface(
    fixture: &Fixture,
    rig: &Rig,
    lanes: &[(Dialect, wire::CanonicalUuid); 2],
) {
    let native_probe = fixture.cleanup.join("native-probe");
    fs::write(&native_probe, "#!/bin/sh\nexit 0\n").expect("native probe writes");
    #[cfg(unix)]
    fs::set_permissions(&native_probe, fs::Permissions::from_mode(0o700))
        .expect("native probe becomes executable");
    for (dialect, _) in lanes {
        for command in ["recall", "board", "inbox"] {
            let output = agent_shell_command(fixture, rig, *dialect, &[command], true);
            assert!(
                output.status.success(),
                "installed {command} failed: stdout={:?} stderr={:?}",
                output.stdout,
                output.stderr
            );
        }
        let output = agent_shell_command(
            fixture,
            rig,
            *dialect,
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
    fs::remove_file(native_probe).expect("native probe cleanup");
}

/// Run one bare `spice` command in the environment the resident gave an agent.
///
/// The coexistence fixture deliberately puts the predecessor executable first
/// in the resident's ambient search path. This shell therefore reaches the
/// installed appliance only when lane launch inserted its own executable
/// directory ahead of that trap.
fn agent_shell_command(
    fixture: &Fixture,
    rig: &Rig,
    dialect: Dialect,
    arguments: &[&str],
    json: bool,
) -> Output {
    let capture = CapturedLaunch::read(
        captures(&rig.capture_index, dialect)
            .last()
            .expect("agent launch was captured"),
    );
    let mut process = Command::new("/bin/sh");
    process
        .args(["-c", "exec spice \"$@\"", "spice"])
        .env_clear()
        .envs(&capture.environment);
    if json {
        process.arg("--json");
    }
    process
        .args(arguments)
        .current_dir(&capture.cwd)
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "captured {} agent shell launches in {}: {error}",
                dialect.name(),
                fixture.root.display(),
            )
        })
}

fn stop_coexisting_appliance(
    fixture: &Fixture,
    rig: &Rig,
    installed_binary: &Path,
    empty_path: &Path,
    lanes: &[(Dialect, wire::CanonicalUuid); 2],
    directives: &[wire::intent::DirectiveAnswer; 2],
    resident: RunningResident,
) {
    for directive in directives {
        let canceled = operator_with_path(
            fixture,
            installed_binary,
            empty_path,
            &["steer", "cancel", &directive.directive.to_string()],
            true,
        );
        assert!(
            canceled.status.success(),
            "steering cancel failed: {:?}",
            canceled.stderr
        );
    }
    for (_, lane) in lanes {
        let completed = lane_command(
            fixture,
            installed_binary,
            *lane,
            empty_path,
            &[
                "task",
                "done",
                "--validation",
                "coexistence shutdown fixture",
            ],
            true,
        );
        assert!(
            completed.status.success(),
            "task completion failed: {:?}",
            completed.stderr
        );
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
