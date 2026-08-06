#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use wire::{DemoAnswer, Outcome, ReplyPosition};

const SEED_COMMIT: &str = "a53ad58b1356a5f7abea7437975321850e04b217";

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

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
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        command
            .args(arguments)
            .current_dir(&self.scratch)
            .env("PATH", &self.ambient_bin)
            .env_remove(steer::hook::INBOX);
        for (name, value) in environment {
            command.env(name, value);
        }
        command.output().expect("native demo process launches")
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
        fs::read(root.join("spice.toml")).expect("read seeded config"),
        b"[tasks]\nstems = [\"rim\"]\n"
    );
    for asset in canvas::APP.assets() {
        assert!(
            root.join(canvas::DEV_STAGE).join(asset.file()).is_file(),
            "demo seed omitted {}",
            asset.file()
        );
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
    assert_eq!(refusal.reason, "demo::root-not-empty");
    assert_eq!(fs::read(sentinel).unwrap(), b"preserve me\n");
    fixture.assert_ambient_untouched(&ambient_before);
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
    assert_eq!(refusal.reason, "demo::commit-moved");
    assert_eq!(
        refusal.expected.as_deref(),
        Some(format!("refs/heads/main at {SEED_COMMIT}").as_str())
    );
    assert_eq!(
        refusal.observed.as_deref(),
        Some("refs/heads/main at 9c3f4bd0bd0b1e2ee59a1e0e9d2b47a3f7c4a5d6")
    );
    assert_eq!(
        refusal.remedy,
        "reset this demo root to its seeded commit or seed a new one"
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
