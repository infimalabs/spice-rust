use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use wire::Outcome;

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-task-migration-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("create migration fixture root");
        Self { root }
    }

    fn export(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, bytes).expect("write predecessor export");
        path
    }

    fn invoke(&self, path: &PathBuf, json: bool) -> std::process::Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        command
            .args(["config", "migrate-tasks"])
            .arg(path)
            .current_dir(&self.root)
            .env("PATH", "")
            .env_remove(steer::hook::INBOX);
        if json {
            command.arg("--json");
        }
        command.output().expect("run task migration command")
    }

    fn snapshot(&self) -> Vec<(String, Vec<u8>)> {
        let mut entries = fs::read_dir(&self.root)
            .expect("read migration fixture")
            .map(|entry| {
                let entry = entry.expect("read fixture entry");
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    fs::read(entry.path()).expect("read fixture file"),
                )
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

const ACCEPTED: &[u8] = br#"{
  "version": 1,
  "release": "0.31.0",
  "tasks": {"hidden_stems": ["sandbox", "oops", "sandbox"]},
  "teams": [
    {"teamId": "alpha", "shellSettings": {"rc": "rc.bad=yes", "taskRc": ""}},
    {"teamId": "beta", "shellSettings": {"rc": []}}
  ]
}"#;

#[test]
fn command_emits_stable_text_and_json_without_external_runtime_lookup() {
    let fixture = Fixture::new();
    let export = fixture.export("accepted.json", ACCEPTED);
    let before = fixture.snapshot();

    let text = fixture.invoke(&export, false);
    assert!(text.status.success(), "text migration: {text:?}");
    assert!(text.stderr.is_empty(), "text migration: {text:?}");
    let text = String::from_utf8(text.stdout).expect("text migration is UTF-8");
    assert!(text.contains("source_release = \"0.31.0\""));
    assert!(text.contains("hidden_stems = [\n    \"oops\""));
    assert!(text.contains("lineage = \"alpha\""));

    let json = fixture.invoke(&export, true);
    assert!(json.status.success(), "JSON migration: {json:?}");
    assert!(json.stderr.is_empty(), "JSON migration: {json:?}");
    let outcome: Outcome<Value> = wire::decode(&json.stdout).expect("typed JSON outcome decodes");
    let Outcome::Reply(reply) = outcome else {
        panic!("accepted export must reply: {outcome:?}");
    };
    assert_eq!(reply.body["source_release"], "0.31.0");
    assert_eq!(
        reply.body["tasks"]["hidden_stems"],
        serde_json::json!(["oops", "maxim_proposal", "sandbox"])
    );
    assert_eq!(fixture.snapshot(), before, "migration is read-only");
}

#[test]
fn refusal_is_typed_and_never_mutates_the_export_directory() {
    let fixture = Fixture::new();
    let export = fixture.export(
        "refused.json",
        br#"{
          "version": 1,
          "release": "0.31.0",
          "teams": [{
            "teamId": "red",
            "shellSettings": {"taskRc": ["", "rc.report.next.filter=project:x"]}
          }]
        }"#,
    );
    let before = fixture.snapshot();
    let output = fixture.invoke(&export, true);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "refusal speaks only on stderr");
    let outcome: Outcome<()> = wire::decode(&output.stderr).expect("typed refusal decodes");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("raw override must refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason, "task-migration::unsupported-override");
    let witness = refusal.witness.expect("refusal carries source witness");
    assert!(witness.contains("release=0.31.0"));
    assert!(witness.contains("lineage=red"));
    assert!(witness.contains("field=teams[0].shellSettings.taskRc[1]"));
    assert!(witness.contains("index=1"));
    assert!(witness.contains("token=\"rc.report.next.filter=project:x\""));
    assert_eq!(fixture.snapshot(), before, "refusal is read-only");
}
