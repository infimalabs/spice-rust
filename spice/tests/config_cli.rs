use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use wire::Outcome;

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str, tracked: &str, local: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-config-cli-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("fixture root is created once");
        fs::create_dir_all(root.join(".git/.spice")).expect("local config parent exists");
        if !tracked.is_empty() {
            fs::write(root.join("spice.toml"), tracked).expect("tracked config is written");
        }
        if !local.is_empty() {
            fs::write(root.join(".git/.spice/config.toml"), local)
                .expect("local config is written");
        }
        Self { root }
    }

    fn invoke(&self, argv: &[&str]) -> Spoken {
        let output = Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(argv)
            .current_dir(&self.root)
            .env_remove(steer::hook::INBOX)
            .output()
            .expect("spice config launches");
        Spoken {
            code: output.status.code(),
            out: output.stdout,
            err: output.stderr,
        }
    }

    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        snapshot(&self.root)
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
fn complete_builtin_registry_has_identical_text_and_json_bodies() {
    let fixture = Fixture::new("builtin", "", "");
    let before = fixture.snapshot();

    let text = fixture.invoke(&["config"]);
    let json = fixture.invoke(&["--json", "config"]);

    assert_eq!(text.code, Some(0));
    assert_eq!(json.code, Some(0));
    assert!(text.err.is_empty());
    assert!(json.err.is_empty());
    let text: Value = wire::decode(&text.out).expect("text body decodes");
    let json: Outcome<Value> = wire::decode(&json.out).expect("JSON outcome decodes");
    let Outcome::Reply(json) = json else {
        panic!("config must reply: {json:?}");
    };
    assert_eq!(json.position, wire::ReplyPosition::Direct {});
    assert_eq!(text, json.body);
    assert_eq!(entries(&text).len(), 24);
    let life = entry(&text, "lane.life");
    assert_eq!(life["value"], "Drive");
    assert_eq!(life["source"], "builtin");
    assert_eq!(life["kind"], "life");
    assert_eq!(life["class"], "policy");
    let container = entry(&text, "proof.container");
    assert_eq!(container["value"], Value::Null);
    assert_eq!(container["builtin"]["kind"], "absent");
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn one_key_reports_tracked_and_local_precedence_without_mutation() {
    let fixture = Fixture::new(
        "precedence",
        concat!("[lane]\nlife = \"Steer\"\n", "[listen]\nwindow = \"2m\"\n",),
        "[listen]\nwindow = \"3m\"\n",
    );
    let before = fixture.snapshot();

    let local = reply(fixture.invoke(&["config", "listen.window", "--json"]));
    assert_eq!(entries(&local).len(), 1);
    let local = entry(&local, "listen.window");
    assert_eq!(local["value"], "3m");
    assert_eq!(local["source"], "local");
    assert_eq!(local["builtin"]["value"], "90s");

    let tracked = reply(fixture.invoke(&["--json", "config", "lane.life"]));
    let tracked = entry(&tracked, "lane.life");
    assert_eq!(tracked["value"], "Steer");
    assert_eq!(tracked["source"], "tracked");
    assert_eq!(tracked["builtin"]["value"], "Drive");
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn unknown_keys_mutation_and_malformed_values_are_typed_refusals() {
    let fixture = Fixture::new("refusal", "", "");
    let before = fixture.snapshot();

    let unknown = refusal(fixture.invoke(&["--json", "config", "missing.key"]));
    assert_eq!(unknown.reason, "config::key-unknown");
    assert_eq!(unknown.observed.as_deref(), Some("missing.key"));

    let mutation = refusal(fixture.invoke(&["config", "lane.life", "Drive", "--json"]));
    assert_eq!(mutation.reason, "config::mutation-unavailable");
    assert_eq!(mutation.observed.as_deref(), Some("lane.life"));
    assert_eq!(fixture.snapshot(), before);

    fs::write(
        fixture.root.join("spice.toml"),
        "[lane]\ndeaths = \"many\"\n",
    )
    .expect("malformed tracked config is written");
    let malformed = refusal(fixture.invoke(&["config", "--json"]));
    assert_eq!(malformed.reason, "config::configuration-refused");
    assert!(
        malformed
            .observed
            .as_deref()
            .is_some_and(|value| value.contains("lane.deaths")),
        "typed path must survive: {malformed:?}"
    );
}

fn reply(spoken: Spoken) -> Value {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    let outcome: Outcome<Value> = wire::decode(&spoken.out).expect("reply outcome decodes");
    let Outcome::Reply(reply) = outcome else {
        panic!("config must reply: {outcome:?}");
    };
    reply.body
}

fn refusal(spoken: Spoken) -> wire::Refusal {
    assert_eq!(spoken.code, Some(2));
    assert!(spoken.out.is_empty());
    let outcome: Outcome<()> = wire::decode(&spoken.err).expect("refusal outcome decodes");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("config must refuse: {outcome:?}");
    };
    refusal
}

fn entries(report: &Value) -> &[Value] {
    report["entries"].as_array().expect("config entries")
}

fn entry<'a>(report: &'a Value, path: &str) -> &'a Value {
    entries(report)
        .iter()
        .find(|entry| entry["path"] == path)
        .unwrap_or_else(|| panic!("config entry {path:?} exists"))
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, at: &Path, output: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let mut children = fs::read_dir(at)
            .expect("snapshot directory is readable")
            .collect::<Result<Vec<_>, _>>()
            .expect("snapshot entries are readable");
        children.sort_by_key(fs::DirEntry::file_name);
        for child in children {
            let path = child.path();
            if child
                .file_type()
                .expect("snapshot type is readable")
                .is_dir()
            {
                walk(root, &path, output);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .expect("snapshot child is beneath root")
                    .to_path_buf();
                output.insert(relative, fs::read(path).expect("snapshot file is readable"));
            }
        }
    }

    let mut output = BTreeMap::new();
    walk(root, root, &mut output);
    output
}
