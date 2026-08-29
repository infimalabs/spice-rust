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
            "[tasks]\nstems = ['rim']\nhidden_stems = ['sandbox']\n",
            "[policy]\nenv_names = []\n",
            "[taste.polling]\nwords = ['polling']\nmessage = 'use events'\n",
            "[lock.worker]\ndirectory = 'locks/worker'\n",
        )
        .as_bytes()
        .to_vec();
        fs::write(root.join("spice-rust.toml"), &tracked).expect("write tracked current config");
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
        self.root.join(".git/.spice-rs/config.toml")
    }

    fn worktree_config(&self) -> PathBuf {
        self.root.join(".spice-rs/config.toml")
    }

    fn worktree(&self, text: &str) {
        fs::create_dir(self.root.join(".spice-rs")).expect("create worktree configuration parent");
        fs::write(self.worktree_config(), text).expect("write worktree configuration");
    }

    fn assert_tracked_unchanged(&self) {
        assert_eq!(
            fs::read(self.root.join("spice-rust.toml")).expect("read tracked config"),
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
fn reads_name_the_current_effective_tree_and_provenance() {
    let fixture = Fixture::new("read");
    fixture.worktree("[vendor.codex]\nmodel = 'lane-model'\neffort = 'xhigh'\n");
    let text = fixture.invoke(&["config"]);
    let json = fixture.invoke(&["--json", "config"]);
    let text = text_body(text);
    let json = reply_body(json);
    assert_eq!(text["mode"], json["mode"]);
    assert_eq!(text["effective"], json["effective"]);
    assert_eq!(text["provenance"], json["provenance"]);
    assert_eq!(json["mode"], "current");
    assert_eq!(
        json["effective"]["taste"]["polling"]["message"],
        "use events"
    );
    assert_eq!(
        json["effective"]["tasks"]["stems"],
        serde_json::json!(["rim"])
    );
    assert_eq!(
        json["effective"]["tasks"]["hidden_stems"],
        serde_json::json!(["oops", "maxim_proposal", "sandbox"])
    );
    assert_eq!(
        json["effective"]["tasks"]["project_min_depth"],
        dialect::PROJECT_MIN_DEPTH
    );
    assert_eq!(
        json["effective"]["tasks"]["project_max_depth"],
        dialect::PROJECT_MAX_DEPTH
    );
    assert_eq!(
        json["effective"]["policy"]["env_names"],
        serde_json::json!([])
    );
    assert_eq!(json["effective"]["vendor"]["codex"]["model"], "lane-model");
    assert!(!fixture.local().exists());

    let selected = reply_body(fixture.invoke(&["config", "lock.busy", "--json"]));
    assert_eq!(selected["selected"]["path"], "lock.busy");
    assert_eq!(selected["selected"]["value"], 75);
    assert_eq!(selected["selected"]["source"], "builtin");
    let selected = reply_body(fixture.invoke(&["config", "vendor.codex.model", "--json"]));
    assert_eq!(selected["selected"]["value"], "lane-model");
    assert_eq!(selected["selected"]["source"], "worktree");
    fixture.assert_tracked_unchanged();
}

#[test]
fn machine_local_sets_preview_then_apply_one_exact_digest() {
    let fixture = Fixture::new("set");
    let text = text_body(fixture.invoke(&["config", "lock.worker.shards", "2"]));
    let preview = reply_body(fixture.invoke(&["--json", "config", "lock.worker.shards", "2"]));
    assert_eq!(text["plan"], preview["plan"]);
    assert_eq!(preview["selected"]["value"], 2);
    assert_eq!(preview["selected"]["source"], "local");
    assert_eq!(preview["plan"]["path"], "lock.worker.shards");
    assert_ne!(preview["plan"]["before"], preview["plan"]["after"]);
    assert!(preview.get("mutation").is_none());
    assert!(
        !fixture.local().exists(),
        "preview must not create local intent"
    );

    let moved = wire::Digest::compute(wire::Domain::ConfigPlan, b"another plan");
    refusal(
        fixture.invoke(&[
            "--json",
            "config",
            "lock.worker.shards",
            "2",
            &format!("--apply={moved}"),
        ]),
        "config::plan-moved",
    );
    assert!(!fixture.local().exists());

    let digest = plan_digest(&preview);
    let first = reply_body(fixture.invoke(&[
        "--json",
        "config",
        "lock.worker.shards",
        "2",
        &format!("--apply={digest}"),
    ]));
    assert_eq!(first["mutation"]["plan"], digest);
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
                    .starts_with(".spice-config-")
            }),
        "handle-bound publication leaves no stage residue"
    );

    refusal(
        fixture.invoke(&[
            "--json",
            "config",
            "lock.worker.shards",
            "2",
            &format!("--apply={digest}"),
        ]),
        "config::plan-moved",
    );
    let repeated = reply_body(fixture.invoke(&["--json", "config", "lock.worker.shards", "2"]));
    let repeated_digest = plan_digest(&repeated);
    let second = reply_body(fixture.invoke(&[
        "config",
        "lock.worker.shards",
        "2",
        &format!("--apply={repeated_digest}"),
        "--json",
    ]));
    assert_eq!(second["mutation"]["changed"], false);
    assert_eq!(second["mutation"]["before"], second["mutation"]["after"]);
    assert_eq!(
        fs::read(fixture.local()).expect("reread local config"),
        local
    );

    let duration = reply_body(fixture.invoke(&["config", "listen.window", "2m", "--json"]));
    assert_eq!(duration["selected"]["value"], "2m");
    assert!(duration.get("mutation").is_none());
    fixture.assert_tracked_unchanged();
}

#[test]
fn worktree_local_vendor_models_preview_apply_and_read_back() {
    let fixture = Fixture::new("set-worktree-model");

    let codex = reply_body(fixture.invoke(&[
        "--json",
        "config",
        "vendor.codex.model",
        "gpt-5.1-codex-max",
    ]));
    assert_eq!(codex["selected"]["value"], "gpt-5.1-codex-max");
    assert_eq!(codex["selected"]["source"], "worktree");
    assert_eq!(codex["plan"]["path"], "vendor.codex.model");
    assert_eq!(
        codex["plan"]["destination"],
        fixture.worktree_config().display().to_string()
    );
    assert!(codex.get("mutation").is_none());
    assert!(
        !fixture.worktree_config().exists(),
        "preview must not create worktree intent"
    );
    assert!(!fixture.local().exists());

    let codex_digest = plan_digest(&codex);
    let applied_codex = reply_body(fixture.invoke(&[
        "--json",
        "config",
        "vendor.codex.model",
        "gpt-5.1-codex-max",
        &format!("--apply={codex_digest}"),
    ]));
    assert_eq!(applied_codex["mutation"]["changed"], true);
    assert_eq!(applied_codex["mutation"]["path"], "vendor.codex.model");

    let claude =
        reply_body(fixture.invoke(&["--json", "config", "vendor.claude.model", "opus-4.1"]));
    assert_eq!(claude["selected"]["value"], "opus-4.1");
    assert_eq!(claude["selected"]["source"], "worktree");
    assert_eq!(claude["plan"]["path"], "vendor.claude.model");
    assert_eq!(
        claude["plan"]["destination"],
        fixture.worktree_config().display().to_string()
    );

    let claude_digest = plan_digest(&claude);
    let applied_claude = reply_body(fixture.invoke(&[
        "--json",
        "config",
        "vendor.claude.model",
        "opus-4.1",
        &format!("--apply={claude_digest}"),
    ]));
    assert_eq!(applied_claude["mutation"]["changed"], true);
    assert_eq!(applied_claude["mutation"]["path"], "vendor.claude.model");

    let worktree = fs::read(fixture.worktree_config()).expect("worktree config was published");
    let parsed: toml::Value =
        toml::from_str(std::str::from_utf8(&worktree).expect("worktree config is UTF-8"))
            .expect("worktree config is valid TOML");
    assert_eq!(
        parsed["vendor"]["codex"]["model"].as_str(),
        Some("gpt-5.1-codex-max")
    );
    assert_eq!(
        parsed["vendor"]["claude"]["model"].as_str(),
        Some("opus-4.1")
    );

    let read_codex = reply_body(fixture.invoke(&["--json", "config", "vendor.codex.model"]));
    assert_eq!(read_codex["selected"]["value"], "gpt-5.1-codex-max");
    assert_eq!(read_codex["selected"]["source"], "worktree");
    let read_claude = reply_body(fixture.invoke(&["--json", "config", "vendor.claude.model"]));
    assert_eq!(read_claude["selected"]["value"], "opus-4.1");
    assert_eq!(read_claude["selected"]["source"], "worktree");

    assert!(!fixture.local().exists());
    fixture.assert_tracked_unchanged();
}

#[test]
fn vendor_model_setter_uses_vendor_launch_value_admission() {
    let fixture = Fixture::new("set-worktree-model-empty");
    refusal(
        fixture.invoke(&["--json", "config", "vendor.codex.model", ""]),
        "config::candidate-refused",
    );
    assert!(!fixture.worktree_config().exists());
    assert!(!fixture.local().exists());
    fixture.assert_tracked_unchanged();
}

#[test]
fn stale_and_malformed_apply_digests_refuse_without_mutation() {
    let fixture = Fixture::new("stale");
    let preview = reply_body(fixture.invoke(&["--json", "config", "lock.worker.shards", "2"]));
    let digest = plan_digest(&preview);
    let moved = String::from_utf8(fixture.tracked.clone())
        .expect("tracked fixture is UTF-8")
        .replace("use events", "prefer events");
    fs::write(fixture.root.join("spice-rust.toml"), moved).expect("move tracked generation");

    refusal(
        fixture.invoke(&[
            "--json",
            "config",
            "lock.worker.shards",
            "2",
            &format!("--apply={digest}"),
        ]),
        "config::plan-moved",
    );
    refusal(
        fixture.invoke(&[
            "--json",
            "config",
            "lock.worker.shards",
            "2",
            "--apply=not-a-digest",
        ]),
        "config::apply-digest-refused",
    );
    refusal(
        fixture.invoke(&["--json", "config", "--apply=not-a-digest"]),
        "config::apply-without-mutation",
    );
    assert!(!fixture.local().exists());
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
    fs::write(
        &target,
        "[lock.worker]\ndirectory = 'locks/worker'\nshards = 7\n",
    )
    .expect("write symlink target");
    fs::create_dir_all(fixture.local().parent().expect("local parent"))
        .expect("create local parent");
    symlink(&target, fixture.local()).expect("plant local symlink");

    refusal(
        fixture.invoke(&["--json", "config", "lock.worker.shards", "2"]),
        "config::source-not-regular",
    );
    assert_eq!(
        fs::read(&target).expect("read symlink target"),
        b"[lock.worker]\ndirectory = 'locks/worker'\nshards = 7\n"
    );
    assert!(
        fs::symlink_metadata(fixture.local())
            .expect("local link remains")
            .file_type()
            .is_symlink()
    );
}

#[test]
fn linked_repository_owner_refuses_without_mutating_outside_authority() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new("linked-owner");
    let marker = fixture.root.join(".git");
    let outside = fixture.root.join("outside-git-owner");
    fs::rename(&marker, &outside).expect("park real Git owner");
    let authority = outside.join(".spice-rs/data/spicejournal.sqlite3");
    fs::create_dir_all(authority.parent().expect("outside authority parent"))
        .expect("create outside authority");
    fs::write(&authority, b"outside authority\n").expect("plant outside authority");
    symlink(&outside, &marker).expect("link repository owner");

    refusal(
        fixture.invoke(&["--json", "config", "lock.worker.shards", "2"]),
        "config::parent-refused",
    );
    assert_eq!(
        fs::read(&authority).expect("read outside authority"),
        b"outside authority\n"
    );
    assert!(!outside.join(".spice-rs/config.toml").exists());
    assert!(!outside.join(".spice-rs/config.lock").exists());
}

#[test]
fn linked_and_replaced_local_parents_never_receive_config_or_lock_artifacts() {
    use std::os::unix::fs::symlink;

    let linked = Fixture::new("linked-parent");
    let outside = linked.root.join("outside-parent");
    fs::create_dir(&outside).expect("create outside parent");
    symlink(&outside, linked.local().parent().expect("local parent")).expect("link local parent");
    refusal(
        linked.invoke(&["--json", "config", "lock.worker.shards", "2"]),
        "config::parent-refused",
    );
    assert!(
        fs::read_dir(&outside)
            .expect("read outside parent")
            .next()
            .is_none(),
        "linked parent received a config or lock artifact"
    );

    let replaced = Fixture::new("replaced-parent");
    let local = replaced.local();
    let parent = local.parent().expect("local parent");
    fs::create_dir_all(parent).expect("create admitted parent");
    let preview = reply_body(replaced.invoke(&["--json", "config", "lock.worker.shards", "2"]));
    let digest = plan_digest(&preview);
    let parked = replaced.root.join("parked-parent");
    fs::rename(parent, &parked).expect("park admitted parent");
    let outside = replaced.root.join("outside-replacement");
    fs::create_dir(&outside).expect("create replacement target");
    symlink(&outside, parent).expect("replace parent with link");
    refusal(
        replaced.invoke(&[
            "--json",
            "config",
            "lock.worker.shards",
            "2",
            &format!("--apply={digest}"),
        ]),
        "config::parent-refused",
    );
    assert!(
        fs::read_dir(&outside)
            .expect("read replacement target")
            .next()
            .is_none(),
        "replaced parent received a config or lock artifact"
    );
    assert!(
        fs::read_dir(&parked)
            .expect("read parked parent")
            .next()
            .is_none(),
        "the formerly admitted parent changed after preview"
    );
}

#[test]
fn linked_lock_file_refuses_apply_without_touching_its_target() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new("linked-lock");
    let local = fixture.local();
    let parent = local.parent().expect("local parent");
    fs::create_dir_all(parent).expect("create local parent");
    let preview = reply_body(fixture.invoke(&["--json", "config", "lock.worker.shards", "2"]));
    let digest = plan_digest(&preview);
    let outside = fixture.root.join("outside.lock");
    fs::write(&outside, b"outside lock").expect("write outside lock");
    symlink(&outside, parent.join("config.lock")).expect("link config lock");
    fault(
        fixture.invoke(&[
            "--json",
            "config",
            "lock.worker.shards",
            "2",
            &format!("--apply={digest}"),
        ]),
        "config::lock-unavailable",
    );
    assert_eq!(
        fs::read(&outside).expect("read outside lock"),
        b"outside lock"
    );
    assert!(!fixture.local().exists());
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

fn plan_digest(body: &Value) -> String {
    body["plan"]["digest"]
        .as_str()
        .expect("config preview carries a plan digest")
        .to_owned()
}

fn refusal(spoken: Spoken, reason: &str) {
    assert_eq!(spoken.code, Some(2));
    assert!(spoken.out.is_empty());
    let outcome: Outcome<Value> = wire::decode(&spoken.err).expect("config refusal decodes");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("config request must refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason(), reason);
}

fn fault(spoken: Spoken, reason: &str) {
    assert_eq!(spoken.code, Some(1));
    assert!(spoken.out.is_empty());
    let outcome: Outcome<Value> = wire::decode(&spoken.err).expect("config fault decodes");
    let Outcome::Fault(fault) = outcome else {
        panic!("config request must fault: {outcome:?}");
    };
    assert_eq!(fault.reason, reason);
}
