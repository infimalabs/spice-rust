#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use wire::Outcome;

struct Fixture {
    root: PathBuf,
    tracked: Vec<u8>,
}

impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-config-cli-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("config fixture root is exclusively created");
        let output = Command::new("/usr/bin/git")
            .args(["init", "-q"])
            .current_dir(&root)
            .output()
            .expect("initialize config fixture repository");
        assert!(output.status.success(), "git init: {output:?}");
        let root = fs::canonicalize(root).expect("canonicalize config fixture root");
        let tracked = concat!(
            "[tasks]\nstems = ['rim']\n",
            "[policy]\nenv_names = []\n",
            "[taste.polling]\nwords = ['poll*']\nmessage = 'use events'\n",
        )
        .as_bytes()
        .to_vec();
        fs::write(root.join("spice.toml"), &tracked).expect("write tracked coexistence config");
        Self { root, tracked }
    }

    fn invoke(&self, argv: &[&str]) -> Spoken {
        let output = Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(argv)
            .current_dir(&self.root)
            .env_remove(steer::hook::INBOX)
            .output()
            .expect("spice config process launches");
        Spoken {
            code: output.status.code(),
            out: output.stdout,
            err: output.stderr,
        }
    }

    fn local(&self) -> PathBuf {
        self.root.join(".git/.spice/config.toml")
    }

    fn assert_tracked_unchanged(&self) {
        assert_eq!(
            fs::read(self.root.join("spice.toml")).expect("read tracked config"),
            self.tracked
        );
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
fn reads_name_the_coexistence_effective_tree_and_provenance() {
    let fixture = Fixture::new("read");
    let text = fixture.invoke(&["config"]);
    let json = fixture.invoke(&["--json", "config"]);
    let text = text_body(text);
    let json = reply_body(json);
    assert_eq!(text["mode"], json["mode"]);
    assert_eq!(text["ignored_tracked_roots"], json["ignored_tracked_roots"]);
    assert_eq!(text["effective"], json["effective"]);
    assert_eq!(text["provenance"], json["provenance"]);
    assert_eq!(json["mode"], "python-scaffold-coexistence");
    assert_eq!(
        json["ignored_tracked_roots"],
        serde_json::json!(["policy", "tasks"])
    );
    assert_eq!(
        json["effective"]["taste"]["polling"]["message"],
        "use events"
    );
    assert!(json["effective"].get("tasks").is_none());
    assert!(json["effective"].get("policy").is_none());
    assert!(!fixture.local().exists());

    let selected = reply_body(fixture.invoke(&["config", "lock.busy", "--json"]));
    assert_eq!(selected["selected"]["path"], "lock.busy");
    assert_eq!(selected["selected"]["value"], 75);
    assert_eq!(selected["selected"]["source"], "builtin");
    fixture.assert_tracked_unchanged();
}

#[test]
fn machine_local_sets_are_atomic_typed_and_digest_receipted() {
    let fixture = Fixture::new("set");
    let first = reply_body(fixture.invoke(&["--json", "config", "lock.worker.shards", "2"]));
    assert_eq!(first["selected"]["value"], 2);
    assert_eq!(first["selected"]["source"], "local");
    assert_eq!(first["mutation"]["path"], "lock.worker.shards");
    assert_eq!(first["mutation"]["changed"], true);
    assert_ne!(first["mutation"]["before"], first["mutation"]["after"]);
    let local = fs::read(fixture.local()).expect("local config was published");
    let parsed: toml::Value =
        toml::from_str(std::str::from_utf8(&local).expect("local config is UTF-8"))
            .expect("local config is valid TOML");
    assert_eq!(parsed["lock"]["worker"]["shards"].as_integer(), Some(2));
    assert!(
        fs::read_dir(fixture.local().parent().expect("local parent"))
            .expect("read local state")
            .all(|entry| {
                !entry
                    .expect("local state entry")
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".spice-atomic-")
            }),
        "atomic publication leaves no stage residue"
    );

    let second = reply_body(fixture.invoke(&["config", "lock.worker.shards", "2", "--json"]));
    assert_eq!(second["mutation"]["changed"], false);
    assert_eq!(second["mutation"]["before"], second["mutation"]["after"]);
    assert_eq!(
        fs::read(fixture.local()).expect("reread local config"),
        local
    );

    let duration = reply_body(fixture.invoke(&["config", "listen.window", "2m", "--json"]));
    assert_eq!(duration["selected"]["value"], "2m");
    fixture.assert_tracked_unchanged();
}

#[test]
fn refused_policy_unknown_and_malformed_values_do_not_create_local_intent() {
    for (label, argv, reason) in [
        (
            "policy",
            &["--json", "config", "lane.vendor", "codex"][..],
            "config::tracked-policy",
        ),
        (
            "unknown",
            &["--json", "config", "absent.value", "1"][..],
            "config::key-unknown",
        ),
        (
            "malformed",
            &["--json", "config", "lock.worker.shards", "zero"][..],
            "config::value-refused",
        ),
    ] {
        let fixture = Fixture::new(label);
        refusal(fixture.invoke(argv), reason);
        assert!(!fixture.local().exists());
        fixture.assert_tracked_unchanged();
    }
}

#[test]
fn local_symlinks_are_refused_without_following_or_replacing_them() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new("symlink");
    let target = fixture.root.join("outside.toml");
    fs::write(&target, "[lock.worker]\nshards = 7\n").expect("write symlink target");
    fs::create_dir_all(fixture.local().parent().expect("local parent"))
        .expect("create local parent");
    symlink(&target, fixture.local()).expect("plant local symlink");

    refusal(
        fixture.invoke(&["--json", "config", "lock.worker.shards", "2"]),
        "config::source-not-regular",
    );
    assert_eq!(
        fs::read(&target).expect("read symlink target"),
        b"[lock.worker]\nshards = 7\n"
    );
    assert!(
        fs::symlink_metadata(fixture.local())
            .expect("local link remains")
            .file_type()
            .is_symlink()
    );
}

fn text_body(spoken: Spoken) -> Value {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    let text = std::str::from_utf8(&spoken.out).expect("text config body is UTF-8");
    let value: toml::Value = toml::from_str(text).expect("text config body is valid TOML");
    serde_json::to_value(value).expect("TOML config body converts to JSON value")
}

fn reply_body(spoken: Spoken) -> Value {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    let outcome: Outcome<Value> = wire::decode(&spoken.out).expect("JSON config outcome decodes");
    let Outcome::Reply(reply) = outcome else {
        panic!("config command must reply: {outcome:?}");
    };
    assert_eq!(reply.position, wire::ReplyPosition::Direct {});
    reply.body
}

fn refusal(spoken: Spoken, reason: &str) {
    assert_eq!(spoken.code, Some(2));
    assert!(spoken.out.is_empty());
    let outcome: Outcome<Value> = wire::decode(&spoken.err).expect("config refusal decodes");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("config request must refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason, reason);
}
