#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use wire::Outcome;

struct Fixture {
    root: PathBuf,
    python: PathBuf,
    python_bytes: Vec<u8>,
    tracked_bytes: Vec<u8>,
    exclude_bytes: Vec<u8>,
}

impl Fixture {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-install-cli-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(root.join(".git/info")).expect("create repository metadata");
        fs::create_dir_all(root.join(".git/.spice/data"))
            .expect("create Python authority sentinel directory");
        fs::create_dir_all(root.join("bin")).expect("create predecessor bin directory");
        let exclude_bytes = b"# python-owned exclusions\n/cache/\n".to_vec();
        fs::write(root.join(".git/info/exclude"), &exclude_bytes).expect("write exclusions");
        fs::write(
            root.join(".git/.spice/data/spicejournal.sqlite3"),
            b"python authority sentinel\n",
        )
        .expect("write Python authority sentinel");
        fs::write(
            root.join(".git/.spice/config.toml"),
            b"[operator]\nmode = 'python'\n",
        )
        .expect("write Python local configuration sentinel");
        let tracked_bytes = b"[tasks]\nmax_parallel = 7\n\n[policy]\nowner = \"python\"\n".to_vec();
        fs::write(root.join("spice.toml"), &tracked_bytes).expect("write tracked intent");
        let python = root.join("bin/spice");
        let python_bytes = b"#!/bin/sh\nprintf 'python-spice\\n'\n".to_vec();
        fs::write(&python, &python_bytes).expect("write predecessor entrypoint");
        fs::set_permissions(&python, fs::Permissions::from_mode(0o755))
            .expect("make predecessor entrypoint executable");
        Self {
            root,
            python,
            python_bytes,
            tracked_bytes,
            exclude_bytes,
        }
    }

    fn invoke(&self, arguments: &[&str]) -> Spoken {
        let output = Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(arguments)
            .current_dir(&self.root)
            .env_remove(steer::hook::INBOX)
            .output()
            .expect("spice install launches");
        Spoken {
            code: output.status.code(),
            out: output.stdout,
            err: output.stderr,
        }
    }

    fn assert_python_authority_unchanged(&self) {
        assert_eq!(fs::read(&self.python).unwrap(), self.python_bytes);
        assert_eq!(
            fs::read(self.root.join("spice.toml")).unwrap(),
            self.tracked_bytes
        );
        assert_eq!(
            fs::read(self.root.join(".git/.spice/data/spicejournal.sqlite3")).unwrap(),
            b"python authority sentinel\n"
        );
        assert_eq!(
            fs::read(self.root.join(".git/.spice/config.toml")).unwrap(),
            b"[operator]\nmode = 'python'\n"
        );
        let mut entries = fs::read_dir(self.root.join(".git/.spice"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        entries.sort();
        assert_eq!(entries, ["config.toml", "data"]);
    }

    fn installed_binary(&self) -> PathBuf {
        self.root.join(".spice/rust/bin/spice-rust")
    }

    fn record(&self) -> PathBuf {
        self.root.join(".git/.spice-rust/install.json")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.root)
            && !std::thread::panicking()
        {
            panic!("remove {}: {error}", self.root.display());
        }
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
    let text = fixture.invoke(&["install"]);
    let json = fixture.invoke(&["--json", "install"]);
    let text = text_body(text);
    let preview = reply_body(json);
    assert_eq!(text, preview);
    assert_eq!(preview["action"], "preview");
    assert_eq!(preview["plan"]["operations"].as_array().unwrap().len(), 5);
    assert!(!fixture.installed_binary().exists());
    assert!(!fixture.record().exists());
    assert!(!fixture.root.join(".git/.spice-rust").exists());
    assert_eq!(
        fs::read(fixture.root.join(".git/info/exclude")).unwrap(),
        fixture.exclude_bytes
    );
    fixture.assert_python_authority_unchanged();

    let digest = preview["digest"].as_str().expect("preview digest");
    let moved = alternate_digest(digest);
    let refused = fixture.invoke(&["--json", "install", &format!("--apply={moved}")]);
    assert_eq!(refused.code, Some(2));
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::plan-moved");
    assert!(!fixture.installed_binary().exists());
    assert!(!fixture.record().exists());

    let applied = reply_body(fixture.invoke(&["--json", "install", &format!("--apply={digest}")]));
    assert_eq!(applied["action"], "applied");
    assert_eq!(applied["status"], "complete");
    assert_eq!(applied["completed"], 5);
    assert_eq!(
        fs::read(fixture.installed_binary()).unwrap(),
        fs::read(env!("CARGO_BIN_EXE_spice")).unwrap()
    );
    assert!(fixture.record().is_file());
    fixture.assert_python_authority_unchanged();
}

#[test]
fn unapply_restores_exact_prior_images_and_keeps_the_namespaced_receipt() {
    let fixture = Fixture::new("unapply");
    let preview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview["digest"].as_str().unwrap();
    let applied = fixture.invoke(&["install", &format!("--apply={digest}")]);
    assert_eq!(applied.code, Some(0));
    assert!(applied.err.is_empty());

    let unapplied = reply_body(fixture.invoke(&["--json", "install", "--unapply"]));
    assert_eq!(unapplied["action"], "unapplied");
    assert_eq!(unapplied["status"], "uninstalled");
    assert_eq!(unapplied["completed"], 5);
    assert!(!fixture.installed_binary().exists());
    assert!(fixture.record().is_file());
    assert_eq!(
        fs::read(fixture.root.join(".git/info/exclude")).unwrap(),
        fixture.exclude_bytes
    );
    fixture.assert_python_authority_unchanged();

    let python = Command::new(&fixture.python)
        .output()
        .expect("run Python entrypoint");
    assert!(python.status.success());
    assert_eq!(python.stdout, b"python-spice\n");
}

#[test]
fn drift_refuses_reversal_without_overwriting_the_changed_target() {
    let fixture = Fixture::new("drift");
    let preview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview["digest"].as_str().unwrap();
    assert_eq!(
        fixture
            .invoke(&["install", &format!("--apply={digest}")])
            .code,
        Some(0)
    );
    let entry = fixture.root.join(".spice/rust/hooks/entry.sh");
    fs::write(&entry, b"operator drift\n").expect("drift installed entry hook");

    let refused = fixture.invoke(&["--json", "install", "--unapply"]);
    assert_eq!(refused.code, Some(2));
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::drift");
    assert_eq!(fs::read(entry).unwrap(), b"operator drift\n");
    assert!(fixture.record().is_file());
    fixture.assert_python_authority_unchanged();
}

fn text_body(spoken: Spoken) -> Value {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    wire::decode(&spoken.out).expect("text body decodes")
}

fn reply_body(spoken: Spoken) -> Value {
    assert_eq!(spoken.code, Some(0));
    assert!(spoken.err.is_empty());
    let outcome: Outcome<Value> = wire::decode(&spoken.out).expect("JSON outcome decodes");
    let Outcome::Reply(reply) = outcome else {
        panic!("install must reply: {outcome:?}");
    };
    assert_eq!(reply.position, wire::ReplyPosition::Direct {});
    reply.body
}

fn assert_refusal(bytes: &[u8], reason: &str) {
    let outcome: Outcome<()> = wire::decode(bytes).expect("typed refusal decodes");
    let Outcome::Refusal(refusal) = outcome else {
        panic!("install must refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason, reason);
}

fn alternate_digest(digest: &str) -> String {
    let mut moved = digest.as_bytes().to_vec();
    let last = moved.last_mut().expect("digest is nonempty");
    *last = if *last == b'0' { b'1' } else { b'0' };
    String::from_utf8(moved).expect("digest is ASCII")
}
