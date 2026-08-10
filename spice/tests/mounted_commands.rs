use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

struct Repository {
    root: PathBuf,
}

impl Repository {
    fn new(configuration: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-mounted-commands-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("fixture root is exclusively created");
        fs::create_dir(root.join(".git")).expect("fixture Git directory");
        fs::write(root.join("spice-rust.toml"), configuration)
            .expect("fixture tracked configuration");
        Self { root }
    }

    fn invoke(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(arguments)
            .current_dir(&self.root)
            .env_remove("CLICOLOR")
            .env_remove("CLICOLOR_FORCE")
            .env_remove("FORCE_COLOR")
            .env("NO_COLOR", "1")
            .output()
            .expect("spice launches")
    }

    fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(unix)]
#[test]
fn declared_argv_runs_at_the_repository_root_with_verbatim_caller_arguments() {
    let repository = Repository::new(
        r#"
        [commands]
        "task.done.fixture" = [
            "sh",
            "-c",
            'printf "%s\n" "$0" "$PWD" "$@"',
            "declared-argument",
        ]
        "#,
    );

    let output = repository.invoke(&["task", "done", "fixture", "--limit", "40", "--json", ""]);

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).expect("mounted command emits UTF-8"),
        format!(
            "declared-argument\n{}\n--limit\n40\n--json\n\n",
            fs::canonicalize(repository.path())
                .expect("fixture root canonicalizes")
                .display()
        )
    );
}

#[test]
fn every_collision_refuses_before_builtin_dispatch_in_sorted_order() {
    let repository = Repository::new(
        r#"
        [commands]
        "task.done" = ["unused"]
        doctor = ["unused"]
        "#,
    );

    let output = repository.invoke(&["doctor"]);
    let stderr = String::from_utf8(output.stderr).expect("refusal emits UTF-8");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let doctor = stderr.find("doctor").expect("doctor collision is named");
    let task_done = stderr
        .find("task.done")
        .expect("registered action collision is named");
    assert!(
        doctor < task_done,
        "collisions are reported deterministically"
    );
    assert!(stderr.contains("shadows generated spice command path"));
}

#[test]
fn malformed_mount_configuration_refuses_loudly() {
    for (configuration, expected) in [
        ("[commands]\nBad = ['unused']\n", "dot-separated segments"),
        ("[commands]\nempty = []\n", "argv must contain a program"),
        (
            "[commands]\nstring = 'unused --flag'\n",
            "expected string-list",
        ),
        (
            "[commands]\nBad = ['unused']\n[lock.solo]\npath = ''\n",
            "dot-separated segments",
        ),
    ] {
        let repository = Repository::new(configuration);
        for owner in ["config", "doctor", "lock"] {
            let output = repository.invoke(&["--json", owner]);
            assert_mount_refusal(&output, &[expected]);
        }
    }
}

#[test]
fn local_configuration_cannot_introduce_an_executable_mount() {
    let repository = Repository::new("");
    let local = repository.path().join(".git/.spice-rs/config.toml");
    fs::create_dir_all(local.parent().expect("local configuration parent"))
        .expect("create local configuration parent");
    fs::write(&local, "[commands]\nlocal = ['unused']\n").expect("write local executable intent");

    for owner in ["config", "doctor", "lock"] {
        let output = repository.invoke(&["--json", owner]);
        assert_mount_refusal(
            &output,
            &["commands.local", "commands.<name> is tracked policy"],
        );
    }
}

fn assert_mount_refusal(output: &Output, expected: &[&str]) {
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let outcome: wire::Outcome<serde_json::Value> =
        wire::decode(&output.stderr).expect("mount refusal is typed JSON");
    let wire::Outcome::Refusal(refusal) = outcome else {
        panic!("mount loading must refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason(), "cli::mount-registry-refused");
    for text in expected {
        assert!(
            refusal.explanation().contains(text),
            "refusal was {refusal:?}"
        );
    }
}
