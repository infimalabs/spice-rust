use super::*;
use flate2::Compression;
use flate2::write::GzEncoder;
use rusqlite::{Connection, params};
use serde_json::Value;
use std::fs::{self, File};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

const CODEX_THREAD: &str = "0123456789abcdef0123456789abcdef";
const CODEX_THREAD_DASHED: &str = "01234567-89ab-cdef-0123-456789abcdef";
const CLAUDE_THREAD: &str = "fedcba9876543210fedcba9876543210";
const CLAUDE_THREAD_DASHED: &str = "fedcba98-7654-3210-fedc-ba9876543210";

#[cfg(unix)]
struct PermissionRestore {
    path: PathBuf,
    permissions: Option<fs::Permissions>,
}

#[cfg(unix)]
impl PermissionRestore {
    fn new(path: &Path, permissions: fs::Permissions) -> Self {
        Self {
            path: path.to_path_buf(),
            permissions: Some(permissions),
        }
    }

    fn restore(mut self) {
        let permissions = self
            .permissions
            .take()
            .expect("permission restoration remains armed");
        fs::set_permissions(&self.path, permissions).expect("restore fixture permissions");
    }
}

#[cfg(unix)]
impl Drop for PermissionRestore {
    fn drop(&mut self) {
        if let Some(permissions) = self.permissions.take() {
            let _ = fs::set_permissions(&self.path, permissions);
        }
    }
}

fn write_gzip(path: &Path, bytes: &[u8]) {
    let file = File::create(path).expect("gzip fixture is created");
    let mut encoder = GzEncoder::new(file, Compression::default());
    encoder
        .write_all(bytes)
        .expect("gzip fixture payload is written");
    encoder.finish().expect("gzip fixture is finalized");
}

#[test]
fn root_thread_discovery_gzip_and_deduplication_preserve_identity() {
    let scratch = Scratch::new("discovery");
    let codex_home = scratch.path().join("codex-home");
    fs::create_dir_all(codex_home.join("sessions")).expect("Codex session root exists");
    let codex_path = codex_home.join("sessions/codex-thread.jsonl");
    let codex_bytes = write_jsonl(&codex_path, &codex_rows());
    let state =
        Connection::open(codex_home.join("state_5.sqlite")).expect("Codex state fixture opens");
    state
        .execute("CREATE TABLE threads (id TEXT, rollout_path TEXT)", [])
        .expect("Codex thread table exists");
    state
        .execute(
            "INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)",
            params![CODEX_THREAD_DASHED, codex_path.to_string_lossy()],
        )
        .expect("Codex thread path is indexed");
    drop(state);
    let codex_env = [
        ("CODEX_THREAD_ID", CODEX_THREAD_DASHED.to_owned()), // env-policy: allow
        ("CODEX_HOME", codex_home.to_string_lossy().into_owned()),
    ];

    let root = reply_forms(&strings(&["session"]), &codex_env, "recall");
    assert_eq!(root["sources"][0]["thread"], CODEX_THREAD_DASHED);
    assert_eq!(root["sources"][0]["vendor"], "codex");

    let codex_thread = reply_forms(
        &strings(&["session", "thread", CODEX_THREAD_DASHED]),
        &[("CODEX_HOME", codex_home.to_string_lossy().into_owned())],
        "thread",
    );
    assert_eq!(codex_thread["thread"]["id"], CODEX_THREAD_DASHED);
    assert_eq!(codex_thread["thread"]["vendor"], "codex");

    let claude_home = scratch.path().join("claude-home");
    let claude_project = claude_home.join("projects/project-a");
    fs::create_dir_all(&claude_project).expect("Claude project root exists");
    let claude_path = claude_project.join(format!("session-{CLAUDE_THREAD}.jsonl"));
    write_jsonl(&claude_path, &claude_rows());
    let claude_thread = reply_forms(
        &strings(&["session", "thread", CLAUDE_THREAD_DASHED]),
        &[
            (
                "CLAUDE_CONFIG_DIR",
                claude_home.to_string_lossy().into_owned(),
            ),
            ("SPICE_AGENT_VENDOR", "claude".to_owned()), // env-policy: allow
        ],
        "thread",
    );
    assert_eq!(claude_thread["thread"]["id"], CLAUDE_THREAD_DASHED);
    assert_eq!(claude_thread["thread"]["vendor"], "claude");
    assert_eq!(
        claude_thread["thread"]["latest_turn"]["turn_id"],
        "prompt-two"
    );

    let resolved_as_input = reply_forms(
        &strings(&["session", "summary", CLAUDE_THREAD_DASHED]),
        &[
            (
                "CLAUDE_CONFIG_DIR",
                claude_home.to_string_lossy().into_owned(),
            ),
            ("SPICE_AGENT_VENDOR", "claude".to_owned()), // env-policy: allow
        ],
        "summary",
    );
    assert_eq!(
        resolved_as_input["sources"][0]["thread"],
        CLAUDE_THREAD_DASHED
    );

    let gzip_path = scratch.path().join("codex-session.jsonl.gz");
    write_gzip(&gzip_path, &codex_bytes);
    let gzip = gzip_path.to_string_lossy().into_owned();
    let deduped = reply_forms(
        &strings(&["session", "summary", &gzip, &gzip, "--recent", "2"]),
        &[],
        "summary",
    );
    assert_eq!(deduped["sources"].as_array().map(Vec::len), Some(1));
    assert_eq!(deduped["sources"][0]["compressed"], true);
    assert_eq!(deduped["turns"], 3);

    scratch.remove();
}

#[test]
fn thread_identity_uses_unique_index_without_collecting_directory_history() {
    let scratch = Scratch::new("continuations");
    let codex_home = scratch.path().join("codex-home");
    let sessions = codex_home.join("sessions");
    fs::create_dir_all(&sessions).expect("Codex session root exists");
    let older = sessions.join(format!("a-rollout-{CODEX_THREAD}.jsonl"));
    let newer = sessions.join(format!("z-rollout-{CODEX_THREAD}.jsonl"));
    write_jsonl(
        &older,
        &[
            codex_event(
                "2026-01-01T00:00:00Z",
                "event_msg",
                json!({"type":"task_started","turn_id":"earlier-turn"}),
            ),
            codex_event(
                "2026-01-01T00:00:01Z",
                "event_msg",
                json!({
                    "type":"task_complete",
                    "turn_id":"earlier-turn",
                    "last_agent_message":"earlier history"
                }),
            ),
        ],
    );
    write_jsonl(
        &newer,
        &[
            codex_event(
                "2026-01-01T00:00:02Z",
                "event_msg",
                json!({"type":"task_started","turn_id":"newer-turn"}),
            ),
            codex_event(
                "2026-01-01T00:00:03Z",
                "event_msg",
                json!({
                    "type":"task_complete",
                    "turn_id":"newer-turn",
                    "last_agent_message":"newer history"
                }),
            ),
        ],
    );
    let state =
        Connection::open(codex_home.join("state_5.sqlite")).expect("Codex state fixture opens");
    state
        .execute("CREATE TABLE threads (id TEXT, rollout_path TEXT)", [])
        .expect("Codex thread table exists");
    state
        .execute(
            "INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)",
            params![CODEX_THREAD_DASHED, newer.to_string_lossy()],
        )
        .expect("newest continuation is indexed");
    drop(state);
    let environment = &[("CODEX_HOME", codex_home.to_string_lossy().into_owned())];

    let first = reply_forms(
        &strings(&["session", "thread", CODEX_THREAD_DASHED]),
        environment,
        "thread",
    );
    let second = reply_forms(
        &strings(&["session", "thread", CODEX_THREAD_DASHED]),
        environment,
        "thread",
    );
    assert_eq!(
        first, second,
        "continuation resolution and replay are deterministic"
    );
    assert_eq!(first["sources"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        first["thread"]["transcripts"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(first["thread"]["turns"], 1);
    assert_eq!(first["thread"]["start"], "2026-01-01T00:00:02.000Z");
    assert_eq!(first["thread"]["latest_turn"]["turn_id"], "newer-turn");
    assert_eq!(
        first["source"]["path"],
        newer
            .canonicalize()
            .expect("current path")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(first["thread"]["transcript"], first["source"]["path"]);
    assert_eq!(
        first["sources"][0]["path"],
        newer
            .canonicalize()
            .expect("newer path")
            .to_string_lossy()
            .as_ref()
    );

    let summary = reply_forms(
        &strings(&["session", "summary", CODEX_THREAD_DASHED]),
        environment,
        "summary",
    );
    assert_eq!(summary["sources"].as_array().map(Vec::len), Some(1));
    assert_eq!(summary["turns"], 1);
    scratch.remove();
}

#[cfg(unix)]
#[test]
fn public_thread_refusal_preserves_an_incomplete_claude_search_before_codex() {
    let scratch = Scratch::new("unreadable-thread");
    let claude_home = scratch.path().join("claude-home");
    let projects = claude_home.join("projects");
    let denied = projects.join("denied");
    fs::create_dir_all(&denied).expect("denied Claude project branch exists");
    let original = fs::metadata(&denied)
        .expect("denied branch metadata exists")
        .permissions();
    let restore = PermissionRestore::new(&denied, original.clone());
    let mut blocked = original;
    blocked.set_mode(0o0);
    fs::set_permissions(&denied, blocked).expect("deny Claude project branch reads");

    let read_error = fs::read_dir(&denied)
        .expect_err("fixture must force a real read_dir failure")
        .to_string();
    let codex_home = scratch.path().join("codex-home");
    fs::create_dir_all(codex_home.join("sessions")).expect("empty Codex session storage exists");
    let omp_home = scratch.path().join("omp-home");
    fs::create_dir_all(omp_home.join("sessions")).expect("empty omp session storage exists");
    let spoken = invoke(
        &strings(&["--json", "session", "thread", CLAUDE_THREAD_DASHED]),
        &[
            (
                "CLAUDE_CONFIG_DIR",
                claude_home.to_string_lossy().into_owned(),
            ),
            ("CODEX_HOME", codex_home.to_string_lossy().into_owned()),
            (
                "PI_CODING_AGENT_DIR",
                omp_home.to_string_lossy().into_owned(),
            ),
            ("SPICE_AGENT_VENDOR", "claude".to_owned()), // env-policy: allow
        ],
    );
    restore.restore();

    assert_eq!(
        spoken.code,
        Some(REFUSAL_EXIT),
        "refusal stdout: {} stderr: {}",
        utf8(&spoken.out),
        utf8(&spoken.err)
    );
    assert!(
        spoken.out.is_empty(),
        "refusal does not impersonate a reply"
    );
    assert_eq!(
        spoken.err.iter().filter(|byte| **byte == b'\n').count(),
        1,
        "public refusal is one JSON outcome"
    );
    let envelope: Value =
        serde_json::from_slice(&spoken.err).expect("refusal is a structured outcome DTO");
    let diagnostic = refusal_diagnostic(&envelope);
    assert_eq!(diagnostic["reason"], "session::thread-unresolved");
    let remedy = diagnostic["detail"]
        .as_str()
        .expect("thread refusal carries diagnostic detail");
    let claude_failure = format!(
        "transcript search was incomplete: could not read directory {}: {read_error}",
        denied.display()
    );
    let codex_failure = format!(
        "missing Codex state database {}",
        codex_home.join("state_5.sqlite").display()
    );
    let omp_failure = format!(
        "omp thread {CLAUDE_THREAD_DASHED} is absent beneath {}",
        omp_home.join("sessions").display()
    );
    assert_eq!(
        remedy,
        format!(
            "restore the owning vendor session record, then retry; \
             {claude_failure}; {codex_failure}; {omp_failure}"
        ),
        "selected Claude failure precedes the prepared Codex and omp failures"
    );
    let former_absence = format!(
        "Claude thread {CLAUDE_THREAD} is absent beneath {}",
        projects.display()
    );
    assert!(!remedy.contains(&former_absence), "{remedy}");

    scratch.remove();
}

/// A Claude home whose projects root is wider than the bounded walk will list
/// still answers for the thread kept beneath the directory keyed by the
/// working directory the command runs from: that directory is checked first,
/// so the walk never starts.
#[test]
fn a_claude_thread_beneath_the_working_directory_resolves_past_the_walk_bound() {
    let scratch = Scratch::new("cwd-thread");
    let cwd = scratch.path().join("worktree");
    fs::create_dir(&cwd).expect("fixture working directory exists");
    let claude_home = scratch.path().join("claude-home");
    let projects = claude_home.join("projects");
    // More siblings than the 256-entry bound the transcript walk stops at.
    for index in 0..300 {
        fs::create_dir_all(projects.join(format!("unrelated-project-{index}")))
            .expect("decoy Claude project directory exists");
    }
    let keyed = cwd
        .canonicalize()
        .expect("fixture working directory canonicalizes")
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "-");
    let claude_path = projects
        .join(keyed)
        .join(format!("{CLAUDE_THREAD_DASHED}.jsonl"));
    fs::create_dir_all(claude_path.parent().expect("keyed project parent"))
        .expect("keyed Claude project directory exists");
    write_jsonl(&claude_path, &claude_rows());
    let environment = [
        (
            "CLAUDE_CONFIG_DIR",
            claude_home.to_string_lossy().into_owned(),
        ),
        ("SPICE_AGENT_VENDOR", "claude".to_owned()), // env-policy: allow
    ];

    let elsewhere = invoke(
        &strings(&["--json", "session", "thread", CLAUDE_THREAD_DASHED]),
        &environment,
    );
    assert_eq!(
        elsewhere.code,
        Some(REFUSAL_EXIT),
        "{}",
        utf8(&elsewhere.err)
    );
    let envelope: Value =
        serde_json::from_slice(&elsewhere.err).expect("refusal is a structured outcome DTO");
    let diagnostic = refusal_diagnostic(&envelope);
    assert_eq!(diagnostic["reason"], "session::thread-unresolved");
    assert!(
        diagnostic["detail"]
            .as_str()
            .expect("thread refusal carries diagnostic detail")
            .contains("256-entry limit"),
        "{}",
        diagnostic["detail"]
    );

    let thread = reply_forms_in(
        Some(&cwd),
        &strings(&["session", "thread", CLAUDE_THREAD_DASHED]),
        &environment,
        "thread",
    );
    assert_eq!(thread["thread"]["id"], CLAUDE_THREAD_DASHED);
    assert_eq!(thread["thread"]["vendor"], "claude");
    assert_eq!(
        thread["thread"]["transcript"],
        claude_path
            .canonicalize()
            .expect("keyed transcript canonicalizes")
            .to_string_lossy()
            .as_ref()
    );

    scratch.remove();
}
