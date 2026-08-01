#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use wire::Outcome;

struct Fixture {
    root: PathBuf,
    original_config: Vec<u8>,
}

impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-install-cli-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("install fixture root is exclusively created");
        let output = Command::new("/usr/bin/git")
            .args(["init", "-q"])
            .current_dir(&root)
            .output()
            .expect("initialize install fixture repository");
        assert!(output.status.success(), "git init: {output:?}");
        let root = fs::canonicalize(root).expect("canonicalize install fixture root");
        let original_config = fs::read(root.join(".git/config")).expect("read original Git config");
        Self {
            root,
            original_config,
        }
    }

    fn with_predecessor_hooks(label: &str) -> Self {
        let mut fixture = Self::new(label);
        fs::create_dir_all(fixture.root.join(".spice/hooks"))
            .expect("create predecessor hooks directory");
        fs::write(
            fixture.root.join(".spice/hooks/pre-commit"),
            "#!/bin/sh\nexec python3 predecessor.py\n",
        )
        .expect("write predecessor hook");
        let output = Command::new("/usr/bin/git")
            .args(["config", "core.hooksPath", ".spice/hooks"])
            .current_dir(&fixture.root)
            .output()
            .expect("select predecessor hooks");
        assert!(output.status.success(), "git config: {output:?}");
        fixture.original_config =
            fs::read(fixture.root.join(".git/config")).expect("read predecessor Git config");
        fixture
    }

    fn invoke(&self, argv: &[&str]) -> Spoken {
        let output = Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(argv)
            .current_dir(&self.root)
            .env_remove(steer::hook::INBOX)
            .output()
            .expect("spice install process launches");
        Spoken {
            code: output.status.code(),
            out: output.stdout,
            err: output.stderr,
        }
    }

    fn record(&self) -> PathBuf {
        self.root.join(".git/.spice/install")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Spoken {
    code: Option<i32>,
    out: Vec<u8>,
    err: Vec<u8>,
}

#[test]
fn preview_is_zero_write_and_apply_requires_its_exact_digest() {
    let fixture = Fixture::new("digest");
    let preview = reply_body(fixture.invoke(&["--json", "install"]));
    assert_eq!(preview["action"], "preview");
    assert_eq!(preview["coexistence"], false);
    assert_eq!(preview["operations"].as_array().map(Vec::len), Some(8));
    assert!(!fixture.record().exists());
    assert!(!fixture.root.join("spice.toml").exists());

    refusal(
        fixture.invoke(&[
            "--json",
            "install",
            "--apply=0000000000000000000000000000000000000000000000000000000000000000",
        ]),
        "install::plan-moved",
    );
    assert!(!fixture.record().exists());

    let digest = preview["digest"]
        .as_str()
        .expect("preview carries a digest");
    let applied = reply_body(fixture.invoke(&["--json", "install", &format!("--apply={digest}")]));
    assert_eq!(applied["action"], "applied");
    assert!(fixture.record().is_file());
    assert_eq!(mode(&fixture.record()), 0o600);
    assert!(fixture.root.join("spice.toml").is_file());
    assert!(fixture.root.join("baseline.toml").is_file());
    assert!(fixture.root.join(".agents/skills/spice/SKILL.md").is_file());
    assert_eq!(
        mode(&fixture.root.join(".git/.spice/hooks/entry.sh")),
        0o755
    );
    assert!(
        fixture
            .root
            .join(".git/.spice/unit/spice.service")
            .is_file()
    );
    assert!(
        fixture
            .root
            .join(".git/.spice/unit/dev.spice.resident.plist")
            .is_file()
    );
    let config =
        fs::read_to_string(fixture.root.join(".git/config")).expect("read installed Git config");
    assert!(config.contains("hooksPath"));
    assert!(config.contains(".git/.spice/hooks"));
}

#[test]
fn unapply_restores_only_the_durable_record_and_refuses_drift() {
    let fixture = Fixture::new("unapply");
    apply_preview(&fixture);
    let skill = fixture.root.join(".agents/skills/spice/SKILL.md");
    fs::write(&skill, "operator changed this after install\n").expect("drift installed skill");
    refusal(
        fixture.invoke(&["--json", "install", "--unapply"]),
        "install::installed-state-moved",
    );
    assert!(fixture.record().exists());
    assert_eq!(
        fs::read_to_string(&skill).expect("read drifted skill"),
        "operator changed this after install\n"
    );

    let current = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = current["digest"].as_str().expect("repair plan digest");
    reply_body(fixture.invoke(&["--json", "install", &format!("--apply={digest}")]));
    let removed = reply_body(fixture.invoke(&["--json", "install", "--unapply"]));
    assert_eq!(removed["action"], "unapplied");
    assert!(!fixture.record().exists());
    assert!(!skill.exists());
    assert!(!fixture.root.join("spice.toml").exists());
    assert!(!fixture.root.join("baseline.toml").exists());
    assert_eq!(
        fs::read(fixture.root.join(".git/config")).expect("read restored Git config"),
        fixture.original_config
    );
}

#[test]
fn predecessor_entrypoint_and_tracked_intent_coexist_with_rust_artifacts() {
    let fixture = Fixture::with_predecessor_hooks("coexist");
    fs::write(
        fixture.root.join("spice.toml"),
        "[tasks]\nstems = ['rim']\n",
    )
    .expect("write predecessor tracked config");
    let tracked = fs::read(fixture.root.join("spice.toml")).expect("read tracked fixture");

    let preview = reply_body(fixture.invoke(&["--json", "install"]));
    assert_eq!(preview["coexistence"], true);
    let git = preview["operations"]
        .as_array()
        .expect("operation rows")
        .iter()
        .find(|row| row["artifact"] == "git-config")
        .expect("Git config operation");
    assert_eq!(git["disposition"], "preserve");
    let digest = preview["digest"].as_str().expect("preview digest");
    reply_body(fixture.invoke(&["--json", "install", &format!("--apply={digest}")]));

    assert_eq!(
        fs::read(fixture.root.join(".git/config")).expect("read preserved Git config"),
        fixture.original_config
    );
    assert_eq!(
        fs::read(fixture.root.join("spice.toml")).expect("read preserved tracked config"),
        tracked
    );
    assert!(fixture.root.join(".spice/hooks/pre-commit").is_file());
    assert!(fixture.root.join(".git/.spice/hooks/entry.sh").is_file());
    let text = fixture.invoke(&["install"]);
    assert_eq!(text.code, Some(0));
    assert!(text.err.is_empty());
    let body: Value = serde_json::from_slice(&text.out).expect("text plan body is stable JSON");
    assert_eq!(body["coexistence"], true);
}

fn apply_preview(fixture: &Fixture) {
    let preview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview["digest"].as_str().expect("preview digest");
    reply_body(fixture.invoke(&["--json", "install", &format!("--apply={digest}")]));
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path)
        .expect("installed path metadata")
        .permissions()
        .mode()
        & 0o777
}

fn reply_body(spoken: Spoken) -> Value {
    assert_eq!(
        spoken.code,
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&spoken.err)
    );
    assert!(spoken.err.is_empty());
    let outcome: Outcome<Value> = wire::decode(&spoken.out).expect("JSON install outcome decodes");
    let Outcome::Reply(reply) = outcome else {
        panic!("install command must reply: {outcome:?}");
    };
    assert_eq!(reply.position, wire::ReplyPosition::Direct {});
    reply.body
}

fn refusal(spoken: Spoken, reason: &str) {
    assert_eq!(spoken.code, Some(2));
    assert!(spoken.out.is_empty());
    let outcome: Outcome<Value> = wire::decode(&spoken.err).expect("install refusal decodes");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("install request must refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason, reason);
}
