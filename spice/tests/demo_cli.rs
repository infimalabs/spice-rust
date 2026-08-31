#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant as Monotonic};

use wire::{DemoAnswer, Outcome, ReplyPosition};

#[path = "support/staged_app.rs"]
mod staged_app;

const SEED_COMMIT: &str = "a53ad58b1356a5f7abea7437975321850e04b217";

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
static BUILD_LANE_APP: Mutex<()> = Mutex::new(());

/// A serialized, temporary source generation for real-binary demo invocations.
struct SeededBuildLaneApp(Option<PathBuf>);

impl SeededBuildLaneApp {
    fn new() -> Self {
        Self::for_worktree(&build_lane_worktree()).unwrap_or_else(|message| panic!("{message}"))
    }

    fn for_worktree(worktree: &Path) -> Result<Self, String> {
        let stage = worktree.join(canvas::APP_STAGE);
        if stage.exists() {
            canvas::AppSource::resolve(worktree).map_err(|error| {
                format!("demo_cli precondition found a stale lane app generation: {error}")
            })?;
            return Ok(Self(None));
        }
        staged_app::seed(worktree);
        Ok(Self(Some(stage)))
    }
}

impl Drop for SeededBuildLaneApp {
    fn drop(&mut self) {
        if let Some(stage) = self.0.take() {
            fs::remove_dir_all(stage).expect("remove generated build lane app fixture");
        }
    }
}

struct Fixture {
    scratch: PathBuf,
    ambient_bin: PathBuf,
    ambient_spice: PathBuf,
    invocation_marker: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let scratch = std::env::temp_dir().join(format!(
            "spice-demo-cli-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&scratch).expect("create exclusive demo process fixture");
        let ambient_bin = scratch.join("ambient-bin");
        fs::create_dir(&ambient_bin).expect("create ambient command directory");
        let ambient_spice = ambient_bin.join("spice");
        let invocation_marker = scratch.join("ambient-spice-invoked");
        let marker = shell_quote(
            invocation_marker
                .to_str()
                .expect("fixture marker path is UTF-8"),
        );
        fs::write(
            &ambient_spice,
            format!("#!/bin/sh\nprintf invoked > {marker}\nexit 91\n"),
        )
        .expect("plant ambient executable sentinel");
        let mut permissions = fs::metadata(&ambient_spice)
            .expect("read ambient sentinel permissions")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&ambient_spice, permissions).expect("make ambient sentinel executable");
        Self {
            scratch,
            ambient_bin,
            ambient_spice,
            invocation_marker,
        }
    }

    fn root(&self, name: &str) -> PathBuf {
        self.scratch.join(name)
    }

    fn invoke(&self, arguments: &[&str]) -> std::process::Output {
        self.invoke_with(arguments, &[])
    }

    fn invoke_with(
        &self,
        arguments: &[&str],
        environment: &[(&str, &Path)],
    ) -> std::process::Output {
        let _app_lock = BUILD_LANE_APP.lock().expect("build lane app fixture lock");
        let _app = SeededBuildLaneApp::new();
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        command
            .args(arguments)
            .current_dir(&self.scratch)
            .env("PATH", &self.ambient_bin)
            .env_remove(steer::hook::INBOX)
            .env_remove(steer::hook::LANE);
        for (name, value) in environment {
            command.env(name, value);
        }
        command.output().expect("native demo process launches")
    }

    fn invoke_at(&self, root: &Path, arguments: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(arguments)
            .current_dir(root)
            .env("PATH", &self.ambient_bin)
            .env_remove(steer::hook::INBOX)
            .env_remove(steer::hook::LANE)
            .output()
            .expect("native demo follow-up launches")
    }

    fn assert_ambient_untouched(&self, expected: &[u8]) {
        assert_eq!(
            fs::read(&self.ambient_spice).expect("read ambient executable sentinel"),
            expected
        );
        assert!(
            !self.invocation_marker.exists(),
            "native demo must not resolve or launch an ambient executable"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.scratch);
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[test]
fn stale_real_stage_precondition_names_the_missing_asset_without_mutating_it() {
    let fixture = Fixture::new();
    let root = fixture.root("stale-app-stage");
    staged_app::seed(&root);
    let stage = root.join(canvas::APP_STAGE);
    let identity = fs::read_to_string(stage.join(canvas::APP_STAGE_CURRENT))
        .expect("read fixture app generation pointer");
    let missing = stage
        .join(canvas::APP_STAGE_GENERATIONS)
        .join(identity.trim_ascii_end())
        .join("mosaic.css");
    fs::remove_file(&missing).expect("remove one current-generation asset");
    let before = stage_snapshot(&stage);

    let error = match SeededBuildLaneApp::for_worktree(&root) {
        Ok(_) => panic!("a stale real stage must fail its precondition"),
        Err(error) => error,
    };

    for required in [
        "demo_cli precondition",
        "stale lane app generation",
        "mosaic.css",
        "dev xtask -- app",
    ] {
        assert!(error.contains(required), "missing {required:?}: {error}");
    }
    assert_eq!(
        stage_snapshot(&stage),
        before,
        "refusing a stale existing stage must not seed over or remove any bytes"
    );
}

#[test]
fn build_lane_stage_is_byte_identical_or_still_absent_after_the_precondition_guard() {
    let _app_lock = BUILD_LANE_APP.lock().expect("build lane app fixture lock");
    let worktree = build_lane_worktree();
    let stage = worktree.join(canvas::APP_STAGE);
    let before = stage_snapshot(&stage);

    drop(SeededBuildLaneApp::new());

    assert_eq!(
        stage_snapshot(&stage),
        before,
        "the suite must preserve an existing real stage and restore an absent one to absence"
    );
}

/// Resolve the writable build lane compiled into selected-tree tests, or the source workspace.
fn build_lane_worktree() -> PathBuf {
    option_env!("SPICE_WORKSPACE_TEST_WRITABLE_ROOT") // env-policy: allow
        .map_or_else(
            || {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .expect("spice crate has a workspace parent")
                    .to_path_buf()
            },
            PathBuf::from,
        )
}

#[test]
fn build_lane_fixture_can_seed_a_read_only_selected_tree_root() {
    let fixture = Fixture::new();
    let root = fixture.root("read-only-selected-source");
    fs::create_dir(&root).expect("create selected-source fixture root");
    let dist = root.join("dist");
    fs::create_dir(&dist).expect("create selected-source dist crate directory");
    for directory in [&root, &dist] {
        let mut permissions = fs::metadata(directory)
            .expect("read selected-source fixture permissions")
            .permissions();
        permissions.set_mode(0o500);
        fs::set_permissions(directory, permissions)
            .expect("make selected-source fixture read-only");
    }

    staged_app::seed(&root);

    let app = canvas::AppSource::resolve(&root).expect("seeded selected source app resolves");
    let generation = app
        .current_generation()
        .expect("selected source generation resolves");
    for asset in canvas::APP.assets() {
        generation.served(*asset).unwrap_or_else(|error| {
            panic!("selected source seed omitted {}: {error}", asset.file())
        });
    }
}

#[test]
fn build_lane_fixture_refuses_a_symlink_escape_without_writing_the_external_tree() {
    let fixture = Fixture::new();
    let root = fixture.root("symlinked-selected-source");
    let external = fixture.root("external-selected-source");
    fs::create_dir(&root).expect("create symlinked selected-source root");
    fs::create_dir(&external).expect("create external selected-source root");
    std::os::unix::fs::symlink(&external, root.join("dist"))
        .expect("link selected-source dist directory outside its root");

    let refused = std::panic::catch_unwind(|| staged_app::seed(&root));

    assert!(
        refused.is_err(),
        "symlinked fixture root unexpectedly seeded"
    );
    assert!(
        !external.join(".spice-rs").exists(),
        "refused fixture seed wrote through its symlink boundary"
    );
}

#[test]
fn staged_app_seed_callers_are_fully_inventoried() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let marker = ["staged_app", "::seed("].concat();
    let mut callers = fs::read_dir(&tests)
        .expect("read Spice integration tests")
        .filter_map(|entry| {
            let path = entry.expect("read Spice integration test entry").path();
            (path.extension().is_some_and(|extension| extension == "rs")
                && fs::read_to_string(&path)
                    .expect("read Spice integration test source")
                    .contains(&marker))
            .then(|| path.file_name().expect("test file name").to_owned())
        })
        .collect::<Vec<_>>();
    callers.sort();

    // `demo_cli.rs` is the sole real-worktree consumer. Every other caller
    // seeds an owned temporary scratch root unconditionally and removes it.
    assert_eq!(
        callers,
        [
            "cli.rs",
            "consent_cli.rs",
            "demo_cli.rs",
            "journal_cli.rs",
            "steering.rs",
            "task_artifact_cli.rs",
            "taste_cli.rs",
        ]
        .map(std::ffi::OsString::from)
    );
}

fn stage_snapshot(stage: &Path) -> Option<Vec<(PathBuf, Vec<u8>)>> {
    if !stage.exists() {
        return None;
    }
    let mut snapshot = Vec::new();
    snapshot_directory(stage, stage, &mut snapshot);
    Some(snapshot)
}

fn snapshot_directory(root: &Path, directory: &Path, snapshot: &mut Vec<(PathBuf, Vec<u8>)>) {
    let mut entries = fs::read_dir(directory)
        .expect("read staged app directory")
        .map(|entry| entry.expect("read staged app entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            snapshot_directory(root, &path, snapshot);
        } else {
            snapshot.push((
                path.strip_prefix(root)
                    .expect("stage entry is beneath root")
                    .to_path_buf(),
                fs::read(&path).expect("read staged app bytes"),
            ));
        }
    }
}

fn wait_for_demo_discovery(launched: &mut Child, discovery: &Path) {
    let deadline = Monotonic::now() + Duration::from_secs(10);
    while !discovery.is_file() && Monotonic::now() < deadline {
        assert!(
            launched
                .try_wait()
                .expect("demo launch state reads")
                .is_none(),
            "printed launch command exited before publishing discovery"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        discovery.is_file(),
        "printed launch command did not publish discovery"
    );
    // The wrapper observes discovery through a polling watcher before opening
    // the canvas. Put external shutdown on the far side of that handoff.
    std::thread::sleep(Duration::from_millis(250));
}

fn lane_answer(output: &Output) -> wire::intent::LaneAnswer {
    let Outcome::Reply(reply) =
        wire::decode::<Outcome<serde_json::Value>>(&output.stdout).expect("lane reply decodes")
    else {
        panic!(
            "live demo lane read must reply: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    wire::decode(&wire::encode(&reply.body).expect("lane reply body encodes"))
        .expect("lane reply body is typed")
}

#[test]
fn seed_only_process_is_anchored_recoverable_and_ambient_inert() {
    let fixture = Fixture::new();
    let ambient_before = fs::read(&fixture.ambient_spice).expect("snapshot ambient authority");
    let root = fixture.root("demo root's");
    let output = fixture.invoke(&[
        "--json",
        "demo",
        "--seed-only",
        "--root",
        root.to_str().expect("fixture root is UTF-8"),
    ]);

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let Outcome::Reply(reply) =
        wire::decode::<Outcome<DemoAnswer>>(&output.stdout).expect("demo reply is typed")
    else {
        panic!("seed-only demo must reply");
    };
    assert_eq!(reply.position, ReplyPosition::Direct {});
    assert_eq!(reply.body.mode, "seeded");
    assert_eq!(reply.body.lifecycle, "preserved");
    assert_eq!(reply.body.endpoint, None);

    let root = fs::canonicalize(root).expect("seeded root is canonical");
    assert_eq!(reply.body.root, root.display().to_string());
    assert_eq!(
        reply.body.transcript,
        root.join("spice-demo-transcript.jsonl")
            .display()
            .to_string()
    );
    assert_eq!(
        fs::read_to_string(root.join(".git/HEAD")).expect("read seeded HEAD"),
        "ref: refs/heads/main\n"
    );
    assert_eq!(
        fs::read_to_string(root.join(".git/refs/heads/main")).expect("read seeded branch"),
        format!("{SEED_COMMIT}\n")
    );
    assert!(root.join(".spice-demo.json").is_file());
    assert_eq!(
        fs::read(root.join("spice-rust.toml")).expect("read seeded config"),
        b"[tasks]\nstems = [\"rim\"]\n"
    );
    let app = canvas::AppSource::resolve(&root).expect("demo lane app resolves");
    let generation = app
        .current_generation()
        .expect("demo app generation resolves");
    for asset in canvas::APP.assets() {
        generation
            .served(*asset)
            .unwrap_or_else(|error| panic!("demo seed omitted {}: {error}", asset.file()));
    }
    let transcript = fs::read_to_string(&reply.body.transcript).expect("read canned transcript");
    let turns = transcript
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("decode canned turn"))
        .collect::<Vec<_>>();
    assert_eq!(turns.len(), 4);
    assert_eq!(turns[0]["timestamp"], "2026-07-11T05:00:00.000000Z");
    assert_eq!(
        turns[0]["text"],
        concat!(
            "Picking up the task to add a `--porcelain` flag to `spice status`. ",
            "Reading the status command to find where the human-readable table is ",
            "assembled, so a machine-readable path can branch from the same data."
        )
    );
    assert_eq!(turns[3]["timestamp"], "2026-07-11T05:03:00.000000Z");
    assert_eq!(
        turns[3]["text"],
        concat!(
            "Done -- `spice status --porcelain` ships behind one flag, with a single ",
            "renderer feeding both views and a test pinning them together. Handing ",
            "the lane back."
        )
    );
    assert!(reply.body.launch_command.starts_with("cd '"));
    assert!(reply.body.launch_command.contains("'\"'\"'"));
    assert!(
        reply
            .body
            .launch_command
            .contains(env!("CARGO_BIN_EXE_spice"))
    );
    assert!(reply.body.launch_command.contains(" demo --reuse "));
    assert!(reply.body.remove_command.starts_with("rm -rf -- "));
    assert_shell_syntax(&reply.body.launch_command);
    assert_shell_syntax(&reply.body.remove_command);
    fixture.assert_ambient_untouched(&ambient_before);

    let sentinel = root.join("operator-bytes");
    fs::write(&sentinel, b"preserve me\n").expect("plant protected content");
    let refused = fixture.invoke(&[
        "--json",
        "demo",
        "--root",
        root.to_str().expect("fixture root is UTF-8"),
    ]);
    assert_eq!(refused.status.code(), Some(2));
    assert!(refused.stdout.is_empty());
    let Outcome::Refusal(refusal) =
        wire::decode::<Outcome<DemoAnswer>>(&refused.stderr).expect("demo refusal is typed")
    else {
        panic!("nonempty demo root must refuse");
    };
    assert_eq!(refusal.reason(), "demo::root-not-empty");
    assert_eq!(fs::read(sentinel).unwrap(), b"preserve me\n");
    fixture.assert_ambient_untouched(&ambient_before);
}

#[test]
fn printed_launch_command_opens_two_simulated_lanes_that_accept_direction() {
    let fixture = Fixture::new();
    let root = fixture.root("steerable-demo");
    let seeded = fixture.invoke(&[
        "--json",
        "demo",
        "--seed-only",
        "--root",
        root.to_str().expect("fixture root is UTF-8"),
    ]);
    assert_eq!(seeded.status.code(), Some(0));
    let Outcome::Reply(seeded) =
        wire::decode::<Outcome<DemoAnswer>>(&seeded.stdout).expect("seed receipt decodes")
    else {
        panic!("demo seed must reply");
    };
    let root = fs::canonicalize(root).expect("seeded root is canonical");
    let state = spice::repository::Repository::discover(&root)
        .expect("seeded repository resolves")
        .admit_state()
        .expect("seeded state is admitted");

    let mut launched = Command::new("/bin/sh")
        .arg("-c")
        .arg(&seeded.body.launch_command)
        .current_dir(&fixture.scratch)
        .env("PATH", &fixture.ambient_bin)
        .env_remove(steer::hook::INBOX)
        .env_remove(steer::hook::LANE)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("printed launch command starts");
    wait_for_demo_discovery(&mut launched, state.canvas());

    let roster = lane_answer(&fixture.invoke_at(&root, &["--json", "lane"]));
    let directions = roster
        .lanes
        .iter()
        .take(2)
        .map(|lane| {
            let lane = lane.lane.to_string();
            fixture.invoke_at(&root, &["--json", "steer", &lane, "inspect the demo"])
        })
        .collect::<Vec<_>>();

    let down = fixture.invoke_at(&root, &["--json", "down"]);
    let completion = launched
        .wait_with_output()
        .expect("printed launch command completes after down");
    let preserved = journal::JournalReader::open(state.journal())
        .expect("demo authority remains readable")
        .read(|read| read.residency_head())
        .expect("demo residency remains readable")
        .expect("demo residency remains present");

    assert!(
        !state.canvas().is_file(),
        "demo discovery is removed after down"
    );
    assert_eq!(
        down.status.code(),
        Some(0),
        "demo down failed: {}",
        String::from_utf8_lossy(&down.stderr)
    );
    assert!(
        completion.status.success(),
        "printed launch command failed: {}",
        String::from_utf8_lossy(&completion.stderr)
    );
    assert!(
        roster.lanes.len() >= 2,
        "fresh demo exposes fewer than two lanes"
    );
    for lane in roster.lanes.iter().take(2) {
        assert_eq!(lane.vendor.as_str(), "simulated");
        assert_eq!(lane.disposition, "simulated");
        assert!(
            lane.occupant.is_some(),
            "simulated lane must accept direction"
        );
    }
    assert_eq!(directions.len(), 2);
    for direction in directions {
        assert_eq!(
            direction.status.code(),
            Some(0),
            "demo steer failed: {}",
            String::from_utf8_lossy(&direction.stderr)
        );
    }
    assert_eq!(preserved.body().lanes.len(), 2);
    assert!(preserved.body().lanes.iter().all(|lane| {
        lane.vendor.as_str() == "simulated"
            && lane.disposition == "simulated"
            && lane.occupant.is_some()
    }));
}

/// The operator's own Git configuration must not reach the seeded branch.
///
/// `init.defaultBranch` is what a plain init consults to point a new `HEAD`,
/// so an operator who renamed their default branch would seed a root whose
/// commit sits on `main` while `HEAD` names an unborn branch beside it. That
/// root is untouched, and yet the reuse the seed just printed refuses it: the
/// unit fixture pins `ref: refs/heads/operator` to exactly the
/// `demo::commit-moved` observation `refs/heads/operator at no commit`, and
/// the remedy of seeding a new root would only reproduce it. Only a child
/// process can be handed a hostile ambient config without racing the rest of
/// this suite, so the seeded bytes are read back here.
#[test]
fn a_seeded_root_is_born_on_main_under_a_renamed_ambient_default_branch() {
    let fixture = Fixture::new();
    let ambient_before = fs::read(&fixture.ambient_spice).expect("snapshot ambient authority");
    let ambient_git = fixture.scratch.join("ambient-gitconfig");
    fs::write(&ambient_git, "[init]\n\tdefaultBranch = operator\n")
        .expect("plant ambient Git authority");
    let root = fixture.root("renamed default branch");
    let seeded = fixture.invoke_with(
        &[
            "--json",
            "demo",
            "--seed-only",
            "--root",
            root.to_str().expect("fixture root is UTF-8"),
        ],
        &[("GIT_CONFIG_GLOBAL", ambient_git.as_path())],
    );

    assert_eq!(seeded.status.code(), Some(0));
    let root = fs::canonicalize(root).expect("seeded root is canonical");
    assert_eq!(
        fs::read_to_string(root.join(".git/HEAD")).expect("read seeded HEAD"),
        "ref: refs/heads/main\n",
        "an ambient default branch moved the seeded head"
    );
    assert_eq!(
        fs::read_to_string(root.join(".git/refs/heads/main")).expect("read seeded branch"),
        format!("{SEED_COMMIT}\n")
    );
    fixture.assert_ambient_untouched(&ambient_before);
}

#[test]
fn reuse_refuses_a_moved_demo_root_over_the_wire_without_ambient_resolution() {
    let fixture = Fixture::new();
    let ambient_before = fs::read(&fixture.ambient_spice).expect("snapshot ambient authority");
    let root = fixture.root("moved root");
    let seeded = fixture.invoke(&[
        "--json",
        "demo",
        "--seed-only",
        "--root",
        root.to_str().expect("fixture root is UTF-8"),
    ]);
    assert_eq!(seeded.status.code(), Some(0));
    let root = fs::canonicalize(root).expect("seeded root is canonical");
    fs::write(
        root.join(".git/refs/heads/main"),
        "9c3f4bd0bd0b1e2ee59a1e0e9d2b47a3f7c4a5d6\n",
    )
    .expect("move the seeded branch off its commit");

    let refused = fixture.invoke(&[
        "--json",
        "demo",
        "--reuse",
        root.to_str().expect("fixture root is UTF-8"),
    ]);
    assert_eq!(refused.status.code(), Some(2));
    assert!(refused.stdout.is_empty());
    let Outcome::Refusal(refusal) =
        wire::decode::<Outcome<DemoAnswer>>(&refused.stderr).expect("demo refusal is typed")
    else {
        panic!("a moved demo root must refuse");
    };
    assert_eq!(refusal.reason(), "demo::commit-moved");
    let diagnostic = refusal.diagnostic().expect("demo refusal is diagnostic");
    assert_eq!(
        diagnostic.expected(),
        Some(format!("refs/heads/main at {SEED_COMMIT}").as_str())
    );
    assert_eq!(
        diagnostic.observed(),
        Some("refs/heads/main at 9c3f4bd0bd0b1e2ee59a1e0e9d2b47a3f7c4a5d6")
    );
    assert_eq!(
        diagnostic.detail(),
        Some("reset this demo root to its seeded commit or seed a new one")
    );
    fixture.assert_ambient_untouched(&ambient_before);
}

fn assert_shell_syntax(command: &str) {
    let status = Command::new("/bin/sh")
        .args(["-n", "-c", command])
        .status()
        .expect("POSIX shell checks recovery command syntax");
    assert!(status.success(), "invalid recovery command: {command}");
}
