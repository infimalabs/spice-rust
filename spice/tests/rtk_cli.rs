use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use wire::{Outcome, RtkGainReport};

const AGENT: &str = "019fe52f-10ad-7c83-9df4-511323d1b7fb";
const COMPACT_AGENT: &str = "019fe52f10ad7c839df4511323d1b7fb";
const OTHER_AGENT: &str = "019fe57c-9c42-7bf0-954a-d7a79dbd6eca";

struct Fixture {
    root: PathBuf,
    rtk: PathBuf,
}

impl Fixture {
    fn new(label: &str, script: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-rtk-cli-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("fixture root is exclusively created");
        let root = fs::canonicalize(root).expect("fixture root canonicalizes");
        fs::create_dir(root.join(".git")).expect("fixture Git directory creates");
        let rtk = root.join("fixture-rtk");
        fs::write(&rtk, script).expect("fixture RTK script writes");
        executable(&rtk);
        fs::write(
            root.join("spice-rust.toml"),
            format!("[rtk]\nexecutable = {:?}\n", rtk.display().to_string()),
        )
        .expect("tracked RTK configuration writes");
        Self { root, rtk }
    }

    fn rust_history(&self, agent: &str) -> PathBuf {
        self.root
            .join(".git/.spice-rs/agents")
            .join(agent)
            .join("rtk/history.db")
    }

    fn stale_worktree_history(&self, root: &Path, agent: &str) -> PathBuf {
        root.join(".spice-rs/agents")
            .join(agent)
            .join("rtk/history.db")
    }

    fn invoke(&self, arguments: &[&str]) -> Output {
        self.invoke_from(&self.root, arguments)
    }

    fn invoke_from(&self, current: &Path, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(arguments)
            .current_dir(current)
            .env_remove(spice::rtk::EXECUTABLE_VARIABLE)
            .env_remove(spice::rtk::HISTORY_VARIABLE)
            .env_remove(steer::hook::INBOX)
            .output()
            .expect("spice RTK command launches")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(unix)]
fn executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .expect("script metadata reads")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).expect("script becomes executable");
}

#[cfg(windows)]
fn executable(_path: &Path) {}

fn report(output: Output) -> RtkGainReport {
    assert!(
        output.status.success(),
        "query failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let outcome: Outcome<RtkGainReport> =
        wire::decode(&output.stdout).expect("typed reply decodes");
    match outcome {
        Outcome::Reply(reply) => reply.body,
        other => panic!("expected RTK reply, got {other:?}"),
    }
}

#[test]
#[cfg(unix)]
fn configured_history_records_and_queries_only_the_selected_agent_database() {
    let fixture = Fixture::new(
        "present",
        r#"#!/bin/sh
case "$1" in
  rewrite)
    printf 'recorded:%s\n' "$*" > "$RTK_DB_PATH"
    printf '%s\n' '/usr/bin/printf rewritten'
    exit 3
    ;;
  gain)
    printf 'selected=%s\n' "$RTK_DB_PATH"
    cat "$RTK_DB_PATH"
    ;;
  *) exit 64 ;;
esac
"#,
    );
    let history = fixture.rust_history(AGENT);
    fs::create_dir_all(history.parent().expect("history has a parent"))
        .expect("agent RTK directory creates");
    let other = fixture.rust_history(OTHER_AGENT);
    fs::create_dir_all(other.parent().expect("other history has a parent"))
        .expect("other agent RTK directory creates");
    fs::write(&other, b"wrong-agent\n").expect("other history writes");

    let recorded = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args(["run", "--", "/usr/bin/printf", "native"])
        .current_dir(&fixture.root)
        .env(spice::rtk::EXECUTABLE_VARIABLE, &fixture.rtk)
        .env(spice::rtk::HISTORY_VARIABLE, &history)
        .env_remove(steer::hook::INBOX)
        .output()
        .expect("supervised command records through RTK");
    assert!(
        recorded.status.success(),
        "recording command failed: stdout={} stderr={}",
        String::from_utf8_lossy(&recorded.stdout),
        String::from_utf8_lossy(&recorded.stderr)
    );
    assert_eq!(recorded.stdout, b"rewritten");

    let queried = report(fixture.invoke(&["--json", "rtk", "gain", COMPACT_AGENT]));
    assert_eq!(queried.agent, COMPACT_AGENT);
    assert_eq!(queried.database, history.display().to_string());
    assert!(
        queried
            .output
            .contains(&format!("selected={}", history.display()))
    );
    assert!(
        queried
            .output
            .contains("recorded:rewrite -- /usr/bin/printf native")
    );
    assert!(!queried.output.contains("wrong-agent"));

    let peer = fixture.root.join("peer-worktree");
    fs::create_dir(&peer).expect("peer worktree creates");
    fs::copy(
        fixture.root.join("spice-rust.toml"),
        peer.join("spice-rust.toml"),
    )
    .expect("peer RTK configuration copies");
    let retained = fixture.root.join(".git/worktrees/peer");
    fs::create_dir_all(&retained).expect("peer worktree metadata creates");
    fs::write(retained.join("commondir"), "../..\n").expect("common pointer writes");
    fs::write(
        retained.join("gitdir"),
        peer.join(".git").display().to_string(),
    )
    .expect("peer worktree pointer writes");
    fs::write(
        peer.join(".git"),
        format!("gitdir: {}\n", retained.display()),
    )
    .expect("peer Git marker writes");
    let stale = fixture.stale_worktree_history(&peer, COMPACT_AGENT);
    fs::create_dir_all(stale.parent().expect("stale history has a parent"))
        .expect("stale RTK directory creates");
    fs::write(&stale, b"stale-wrong-path\n").expect("stale history writes");
    let queried = report(fixture.invoke_from(&peer, &["--json", "rtk", "gain", AGENT]));
    assert_eq!(queried.database, history.display().to_string());
    assert!(
        queried
            .output
            .contains(&format!("selected={}", history.display()))
    );
    assert!(
        queried
            .output
            .contains("recorded:rewrite -- /usr/bin/printf native")
    );
    assert!(!queried.output.contains("stale-wrong-path"));
}

#[test]
#[cfg(unix)]
fn unknown_agent_uuid_is_a_clear_refusal_without_starting_rtk() {
    let fixture = Fixture::new("absent", "#!/bin/sh\nexit 99\n");
    let output = fixture.invoke(&["rtk", "gain", COMPACT_AGENT]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).expect("refusal is UTF-8");
    assert!(diagnostic.contains("rtk::history-absent"));
    assert!(diagnostic.contains(AGENT));
}

#[test]
#[cfg(unix)]
fn duplicate_stale_worktree_histories_are_ignored() {
    let fixture = Fixture::new("stale", "#!/bin/sh\nexit 99\n");
    let first = fixture.stale_worktree_history(&fixture.root, AGENT);
    fs::create_dir_all(first.parent().expect("stale history has a parent"))
        .expect("stale RTK directory creates");
    fs::write(&first, b"stale-one\n").expect("stale history writes");
    let peer = fixture.root.join("peer-worktree");
    fs::create_dir(&peer).expect("peer worktree root creates");
    let second = fixture.stale_worktree_history(&peer, COMPACT_AGENT);
    fs::create_dir_all(second.parent().expect("stale history has a parent"))
        .expect("peer stale RTK directory creates");
    fs::write(&second, b"stale-two\n").expect("peer stale history writes");

    let output = fixture.invoke(&["rtk", "gain", AGENT]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).expect("refusal is UTF-8");
    assert!(diagnostic.contains("rtk::history-absent"));
    assert!(diagnostic.contains(&fixture.rust_history(AGENT).display().to_string()));
    assert!(!diagnostic.contains(&first.display().to_string()));
    assert!(!diagnostic.contains(&second.display().to_string()));
}

#[test]
#[cfg(unix)]
fn one_unreadable_history_names_its_exact_provenance_without_starting_rtk() {
    let fixture = Fixture::new("unreadable", "#!/bin/sh\nexit 99\n");
    let history = fixture.rust_history(AGENT);
    fs::create_dir_all(&history).expect("nonregular history fixture creates");

    let output = fixture.invoke(&["rtk", "gain", AGENT]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).expect("fault is UTF-8");
    assert!(diagnostic.contains("rtk::history-unreadable"));
    assert!(diagnostic.contains(&history.display().to_string()));
    assert!(diagnostic.contains("not a regular file"));
}

#[test]
#[cfg(unix)]
fn rtk_failure_names_the_exact_database_and_child_diagnostic() {
    let fixture = Fixture::new(
        "failure",
        "#!/bin/sh\nprintf 'fixture RTK failure\\n' >&2\nexit 9\n",
    );
    let history = fixture.rust_history(AGENT);
    fs::create_dir_all(history.parent().expect("history has a parent"))
        .expect("agent RTK directory creates");
    fs::write(&history, b"present\n").expect("history writes");

    let output = fixture.invoke(&["rtk", "gain", COMPACT_AGENT]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).expect("fault is UTF-8");
    assert!(diagnostic.contains("rtk::command-failed"));
    assert!(diagnostic.contains(&history.display().to_string()));
    assert!(diagnostic.contains("exit status: 9"));
    assert!(diagnostic.contains("fixture RTK failure"));
}
