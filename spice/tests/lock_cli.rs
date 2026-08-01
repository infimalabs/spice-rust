#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use wire::Outcome;

const BUSY_EXIT: i32 = 70;
const TAKEN_EXIT: i32 = 71;
const FULL_EXIT: i32 = 72;

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-lock-cli-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("lock fixture root is exclusively created");
        let output = Command::new("/usr/bin/git")
            .args(["init", "-q"])
            .current_dir(&root)
            .output()
            .expect("initialize lock fixture repository");
        assert!(output.status.success(), "git init: {output:?}");
        let root = fs::canonicalize(root).expect("canonicalize lock fixture root");
        fs::write(
            root.join("spice.toml"),
            concat!(
                "[lock]\n",
                "busy = 70\n",
                "taken = 71\n",
                "full = 72\n",
                "\n[lock.solo]\n",
                "shards = 1\n",
                "\n[lock.pool]\n",
                "shards = 2\n",
            ),
        )
        .expect("write lock fixture configuration");
        Self { root }
    }

    fn invoke(&self, argv: &[&str]) -> Spoken {
        let output = Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(argv)
            .current_dir(&self.root)
            .env_remove(steer::hook::INBOX)
            .output()
            .expect("spice lock process launches");
        Spoken {
            code: output.status.code(),
            out: output.stdout,
            err: output.stderr,
        }
    }

    fn hold(&self, name: &str, shard: Option<u32>) -> HeldProcess {
        let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
        command
            .arg("lock")
            .arg("run")
            .arg(name)
            .current_dir(&self.root)
            .env_remove(steer::hook::INBOX)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(shard) = shard {
            command.arg("--shard").arg(shard.to_string());
        }
        let mut child = command
            .args([
                "--",
                "/bin/sh",
                "-c",
                "printf 'READY\\n'; /bin/cat >/dev/null",
            ])
            .spawn()
            .expect("spawn held lock process");
        let stdout = child.stdout.take().expect("held process stdout is piped");
        let mut reader = BufReader::new(stdout);
        let mut ready = String::new();
        reader
            .read_line(&mut ready)
            .expect("held child writes readiness line");
        assert_eq!(ready, "READY\n");
        HeldProcess {
            child,
            stdout: reader,
        }
    }

    fn lock_root(&self) -> PathBuf {
        self.root.join(".git/.spice/locks")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct HeldProcess {
    child: Child,
    stdout: BufReader<std::process::ChildStdout>,
}

impl HeldProcess {
    fn release(mut self) {
        self.child
            .stdin
            .take()
            .expect("held process stdin is piped")
            .write_all(b"release\n")
            .expect("release held child");
        let mut trailing = String::new();
        self.stdout
            .read_to_string(&mut trailing)
            .expect("drain held child stdout");
        let output = self
            .child
            .wait_with_output()
            .expect("held lock process exits");
        assert!(output.status.success(), "held lock process: {output:?}");
        assert!(output.stderr.is_empty(), "held lock stderr: {output:?}");
    }
}

struct Spoken {
    code: Option<i32>,
    out: Vec<u8>,
    err: Vec<u8>,
}

#[test]
fn status_is_a_stable_read_only_direct_report_in_both_forms() {
    let fixture = Fixture::new("status");
    let text = fixture.invoke(&["lock"]);
    let json = fixture.invoke(&["lock", "--json"]);

    assert_eq!(text.code, Some(0));
    assert!(text.err.is_empty());
    let text: Value = wire::decode(&text.out).expect("text body is the typed report");
    let json: Outcome<Value> = wire::decode(&json.out).expect("JSON is a typed outcome");
    let Outcome::Reply(json) = json else {
        panic!("lock status must reply: {json:?}");
    };
    assert_eq!(json.position, wire::ReplyPosition::Direct {});
    assert_eq!(text, json.body);

    let rows = text["locks"].as_array().expect("lock rows are an array");
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|row| row["state"] == "available"));
    assert!(rows.iter().all(|row| row.get("holder").is_none()));
    assert!(
        !fixture.lock_root().exists(),
        "a status read does not create lock state"
    );
}

#[test]
fn run_preserves_exact_argv_environment_cwd_and_child_status() {
    let fixture = Fixture::new("child");
    let output = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args([
            "lock",
            "run",
            "solo",
            "--",
            "/bin/sh",
            "-c",
            "printf '%s|%s|%s' \"$LOCK_PROBE\" \"$1\" \"$PWD\"",
            "child-zero",
            "--child-flag",
        ])
        .current_dir(&fixture.root)
        .env_remove(steer::hook::INBOX)
        .env("LOCK_PROBE", "exact")
        .output()
        .expect("lock child process launches");
    assert!(output.status.success(), "lock child: {output:?}");
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).expect("child output is UTF-8"),
        format!("exact|--child-flag|{}", fixture.root.display())
    );
    assert_eq!(
        fs::read(fixture.lock_root().join("solo/0")).expect("read released lock metadata"),
        b""
    );

    let exited = fixture.invoke(&["lock", "run", "solo", "--", "/bin/sh", "-c", "exit 23"]);
    assert_eq!(exited.code, Some(23));
    assert!(exited.out.is_empty());
    assert!(exited.err.is_empty());
}

#[test]
fn holder_reports_and_all_three_contention_classes_are_observable() {
    let fixture = Fixture::new("contention");
    let solo = fixture.hold("solo", None);

    let status = fixture.invoke(&["lock", "--json"]);
    let report = reply_body(status);
    let solo_row = rows(&report)
        .iter()
        .find(|row| row["name"] == "solo")
        .expect("solo status row exists");
    assert_eq!(solo_row["state"], "held");
    assert_eq!(
        solo_row["holder"]["cwd"],
        fixture.root.to_string_lossy().as_ref()
    );
    assert!(solo_row["holder"]["pid"].as_u64().is_some());
    assert!(solo_row["holder"]["started"].as_str().is_some());

    assert_contention(
        fixture.invoke(&["--json", "lock", "run", "solo", "--", "/usr/bin/true"]),
        BUSY_EXIT,
        "lock::busy",
    );

    let pool_zero = fixture.hold("pool", Some(0));
    assert_contention(
        fixture.invoke(&[
            "--json",
            "lock",
            "run",
            "pool",
            "--shard",
            "0",
            "--",
            "/usr/bin/true",
        ]),
        TAKEN_EXIT,
        "lock::taken",
    );
    let pool_one = fixture.hold("pool", Some(1));
    assert_contention(
        fixture.invoke(&["--json", "lock", "run", "pool", "--", "/usr/bin/true"]),
        FULL_EXIT,
        "lock::full",
    );

    pool_one.release();
    pool_zero.release();
    solo.release();
    let report = reply_body(fixture.invoke(&["lock", "--json"]));
    assert!(rows(&report).iter().all(|row| row["state"] == "available"));
}

#[test]
fn a_signal_to_the_wrapper_terminates_the_child_group_and_releases_the_lock() {
    let fixture = Fixture::new("signal");
    let mut child = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args([
            "lock",
            "run",
            "solo",
            "--",
            "/bin/sh",
            "-c",
            "printf 'READY\\n'; exec /usr/bin/tail -f /dev/null",
        ])
        .current_dir(&fixture.root)
        .env_remove(steer::hook::INBOX)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn signal fixture");
    let stdout = child.stdout.take().expect("signal fixture stdout is piped");
    let mut reader = BufReader::new(stdout);
    let mut ready = String::new();
    reader
        .read_line(&mut ready)
        .expect("signal fixture reaches child execution");
    assert_eq!(ready, "READY\n");

    let killed = Command::new("/bin/kill")
        .args(["-TERM", &child.id().to_string()])
        .output()
        .expect("signal lock wrapper");
    assert!(killed.status.success(), "kill wrapper: {killed:?}");
    let output = child.wait_with_output().expect("signalled wrapper exits");
    assert_eq!(output.status.code(), Some(143));
    assert!(output.stderr.is_empty(), "signal fixture: {output:?}");
    assert_eq!(
        fs::read(fixture.lock_root().join("solo/0")).expect("read lock after signal"),
        b""
    );

    let reacquired = fixture.invoke(&["lock", "run", "solo", "--", "/usr/bin/true"]);
    assert_eq!(reacquired.code, Some(0));
}

#[test]
fn malformed_names_shards_and_configuration_refuse_without_running_a_child() {
    let fixture = Fixture::new("refusals");
    for (argv, reason) in [
        (
            &["--json", "lock", "run", "../escape", "--", "/usr/bin/true"][..],
            "lock::name-refused",
        ),
        (
            &[
                "--json",
                "lock",
                "run",
                "pool",
                "--shard",
                "2",
                "--",
                "/usr/bin/true",
            ][..],
            "lock::shard-refused",
        ),
        (
            &["--json", "lock", "run", "absent", "--", "/usr/bin/true"][..],
            "lock::unknown",
        ),
    ] {
        let spoken = fixture.invoke(argv);
        assert_eq!(spoken.code, Some(2));
        let outcome: Outcome<()> = wire::decode(&spoken.err).expect("typed refusal decodes");
        let Outcome::Refusal(refusal) = outcome else {
            panic!("invalid lock request must refuse: {outcome:?}");
        };
        assert_eq!(refusal.reason, reason);
    }
    assert!(!fixture.lock_root().exists());

    fs::write(fixture.root.join("spice.toml"), "[lock.solo]\nshards = 0\n")
        .expect("write invalid lock configuration");
    let invalid = fixture.invoke(&["lock", "--json"]);
    assert_eq!(invalid.code, Some(2));
    let invalid: Outcome<Value> = wire::decode(&invalid.err).expect("config refusal decodes");
    let Outcome::Refusal(invalid) = invalid else {
        panic!("invalid config must refuse: {invalid:?}");
    };
    assert_eq!(invalid.reason, "lock::configuration-refused");
}

fn reply_body(spoken: Spoken) -> Value {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    let outcome: Outcome<Value> = wire::decode(&spoken.out).expect("typed reply decodes");
    let Outcome::Reply(reply) = outcome else {
        panic!("lock status must reply: {outcome:?}");
    };
    reply.body
}

fn rows(report: &Value) -> &[Value] {
    report["locks"].as_array().expect("lock report rows")
}

fn assert_contention(spoken: Spoken, code: i32, reason: &str) {
    assert_eq!(spoken.code, Some(code));
    assert!(spoken.out.is_empty());
    let outcome: Outcome<()> = wire::decode(&spoken.err).expect("contention refusal decodes");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("lock contention must refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason, reason);
}
