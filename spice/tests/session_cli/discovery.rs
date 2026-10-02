use super::*;
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::Value;
use std::fs::{self, File};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

const OTHER_THREAD: &str = "fedcba98-7654-3210-fedc-ba9876543210";

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
    let omp_home = scratch.path().join("omp-home");
    let folder = omp_home.join("sessions/-fixture");
    fs::create_dir_all(&folder).expect("omp session root exists");
    let omp_path = folder.join(format!("2026-01-01T00-00-00-000Z_{THREAD}.jsonl"));
    let omp_bytes = write_jsonl(&omp_path, &omp_rows(THREAD));
    let home = (
        "PI_CODING_AGENT_DIR",
        omp_home.to_string_lossy().into_owned(),
    );
    let live = [
        (vendor::omp::SESSION_ENVIRONMENT, THREAD.to_owned()),
        home.clone(),
    ];

    let root = reply_forms(&strings(&["session"]), &live, "recall");
    assert_eq!(root["sources"][0]["thread"], THREAD);
    assert_eq!(root["sources"][0]["vendor"], "omp");

    let thread = reply_forms(
        &strings(&["session", "thread", THREAD]),
        std::slice::from_ref(&home),
        "thread",
    );
    assert_eq!(thread["thread"]["id"], THREAD);
    assert_eq!(thread["thread"]["vendor"], "omp");

    let resolved_as_input = reply_forms(
        &strings(&["session", "summary", THREAD]),
        std::slice::from_ref(&home),
        "summary",
    );
    assert_eq!(resolved_as_input["sources"][0]["thread"], THREAD);

    let gzip_path = scratch.path().join("omp-session.jsonl.gz");
    write_gzip(&gzip_path, &omp_bytes);
    let gzip = gzip_path.to_string_lossy().into_owned();
    let deduped = reply_forms(
        &strings(&["session", "summary", &gzip, &gzip, "--limit", "2"]),
        &[],
        "summary",
    );
    assert_eq!(deduped["sources"].as_array().map(Vec::len), Some(1));
    assert_eq!(deduped["sources"][0]["compressed"], true);
    assert_eq!(deduped["turns"], 3);

    scratch.remove();
}

#[cfg(unix)]
#[test]
fn public_thread_refusal_preserves_an_incomplete_omp_search() {
    let scratch = Scratch::new("unreadable-thread");
    let omp_home = scratch.path().join("omp-home");
    let sessions = omp_home.join("sessions");
    let denied = sessions.join("-denied");
    fs::create_dir_all(&denied).expect("denied omp session folder exists");
    let original = fs::metadata(&denied)
        .expect("denied folder metadata exists")
        .permissions();
    let restore = PermissionRestore::new(&denied, original.clone());
    let mut blocked = original;
    blocked.set_mode(0o0);
    fs::set_permissions(&denied, blocked).expect("deny omp session folder reads");

    let read_error = fs::read_dir(&denied)
        .expect_err("fixture must force a real read_dir failure")
        .to_string();
    let spoken = invoke(
        &strings(&["--json", "session", "thread", OTHER_THREAD]),
        &[(
            "PI_CODING_AGENT_DIR",
            omp_home.to_string_lossy().into_owned(),
        )],
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
    assert!(
        remedy.contains(&format!(
            "could not read directory {}: {read_error}",
            denied.display()
        )),
        "the unreadable folder is named rather than reported as an absent thread: {remedy}"
    );

    scratch.remove();
}

/// An omp home whose sessions root is wider than the bounded walk will list
/// still answers for the thread filed beneath the folder oh-my-pi names for
/// the working directory the command runs from: `-` and the directory
/// relative to the home.
#[test]
fn an_omp_thread_beneath_the_working_directory_resolves_past_the_walk_bound() {
    let scratch = Scratch::new("cwd-omp-thread");
    let cwd = scratch.path().join("worktree");
    fs::create_dir(&cwd).expect("fixture working directory exists");
    let omp_home = scratch.path().join("omp-home");
    let sessions = omp_home.join("sessions");
    // More siblings than the 256-entry bound the transcript walk stops at.
    for index in 0..300 {
        fs::create_dir_all(sessions.join(format!("-unrelated-project-{index}")))
            .expect("decoy omp session folder exists");
    }
    let stem = format!("2026-09-16T06-43-21-805Z_{OTHER_THREAD}");
    let folder = sessions.join("-worktree");
    fs::create_dir_all(folder.join(&stem)).expect("omp artifact directory exists");
    let omp_path = folder.join(format!("{stem}.jsonl"));
    write_jsonl(
        &omp_path,
        &[
            json!({"type": "session", "version": 3, "id": OTHER_THREAD,
                "timestamp": "2026-09-16T06:43:21.805Z", "cwd": cwd.to_string_lossy()}),
            json!({"type": "message", "id": "u1", "parentId": null,
                "timestamp": "2026-09-16T06:43:25.000Z",
                "message": {"role": "user", "content": [{"type": "text", "text": "hello"}],
                    "timestamp": 1789540205000_u64}}),
            json!({"type": "message", "id": "a1", "parentId": "u1",
                "timestamp": "2026-09-16T06:43:26.000Z",
                "message": {"role": "assistant", "content": [{"type": "text", "text": "hi"}],
                    "usage": {"input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0},
                    "stopReason": "stop"}}),
        ],
    );
    let environment = [
        ("HOME", scratch.path().to_string_lossy().into_owned()),
        (
            "PI_CODING_AGENT_DIR",
            omp_home.to_string_lossy().into_owned(),
        ),
        ("SPICE_AGENT_VENDOR", "omp".to_owned()), // env-policy: allow
    ];

    let elsewhere = invoke(
        &strings(&["--json", "session", "thread", OTHER_THREAD]),
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
        &strings(&["session", "thread", OTHER_THREAD]),
        &environment,
        "thread",
    );
    assert_eq!(thread["thread"]["id"], OTHER_THREAD);
    assert_eq!(thread["thread"]["vendor"], "omp");
    assert_eq!(
        thread["thread"]["transcript"],
        omp_path
            .canonicalize()
            .expect("filed transcript canonicalizes")
            .to_string_lossy()
            .as_ref()
    );

    scratch.remove();
}
