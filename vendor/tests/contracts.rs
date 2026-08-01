use std::ffi::OsStr;
use std::time::Duration;

use listen::{Dialect, EventKind, Locus};
use sill::{
    Cancellation, Deadlines, Environment, ProcessGroup, Redaction, RestartDisposition, StdinPolicy,
    StreamPolicy, WaitPolicy,
};
use vendor::{Process, Record, RecordStream, Resume, SessionId, claude, codex, permission_request};
use wire::CanonicalUuid;

const CLAUDE_PROSE: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":7,"cache_read_input_tokens":5,"cache_creation_input_tokens":3,"output_tokens":2}}}"#;
const CODEX_PROSE: &[u8] =
    br#"{"type":"item.completed","item":{"id":"item-1","type":"agent_message","text":"done"}}"#;

#[test]
fn claude_fresh_and_resume_use_one_stream_contract_without_io() {
    let fresh = claude::record("/opt/claude", "start", Resume::Fresh).expect("valid fresh record");
    assert_record(
        &fresh,
        "/opt/claude",
        &[
            "--print",
            "--verbose",
            "--output-format",
            "stream-json",
            "start",
        ],
        Dialect::Claude,
        None,
    );

    let resumed = claude::record(
        "/opt/claude",
        "continue",
        Resume::Session(SessionId::new("claude-session").expect("session identity")),
    )
    .expect("valid resumed record");
    assert_record(
        &resumed,
        "/opt/claude",
        &[
            "--print",
            "--verbose",
            "--output-format",
            "stream-json",
            "--resume",
            "claude-session",
            "continue",
        ],
        Dialect::Claude,
        Some("claude-session"),
    );
    assert_stream_wiring(&resumed, CLAUDE_PROSE);
    assert!(SessionId::new("  ").is_err());
}

#[test]
fn codex_fresh_and_resume_use_one_stream_contract_without_io() {
    let fresh = codex::record("/opt/codex", "start", Resume::Fresh).expect("valid fresh record");
    assert_record(
        &fresh,
        "/opt/codex",
        &["exec", "--json", "start"],
        Dialect::Codex,
        None,
    );

    let resumed = codex::record(
        "/opt/codex",
        "continue",
        Resume::Session(SessionId::new("codex-thread").expect("session identity")),
    )
    .expect("valid resumed record");
    assert_record(
        &resumed,
        "/opt/codex",
        &["exec", "resume", "--json", "codex-thread", "continue"],
        Dialect::Codex,
        Some("codex-thread"),
    );
    assert_stream_wiring(&resumed, CODEX_PROSE);
}

#[test]
fn acp_emits_only_the_typed_request_for_consent() {
    let request: CanonicalUuid = "018f7777-7777-7777-8777-777777777777"
        .parse()
        .expect("canonical request");
    let lane: CanonicalUuid = "018f8888-8888-7888-8888-888888888888"
        .parse()
        .expect("canonical lane");

    let asked = permission_request(request, lane, "filesystem.write").expect("valid request");
    assert_eq!(asked.request, request);
    assert_eq!(asked.lane, lane);
    assert_eq!(asked.operation, "filesystem.write");
    assert!(permission_request(request, lane, "  ").is_err());
}

fn assert_record(
    record: &Record,
    binary: &str,
    arguments: &[&str],
    dialect: Dialect,
    session: Option<&str>,
) {
    assert_eq!(record.binary(), OsStr::new(binary));
    assert_eq!(
        record
            .arguments()
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        arguments
    );
    assert_eq!(record.dialect(), dialect);
    assert_eq!(record.stream(), RecordStream::Stdout);
    assert_eq!(record.resume().session_id(), session);

    let spec = record.spec(&process());
    assert_eq!(spec.argv.program(), record.binary());
    assert_eq!(spec.argv.arguments(), record.arguments());
    assert_eq!(spec.group, ProcessGroup::New);
    assert_eq!(spec.output.stdin(), StdinPolicy::Null);
    assert_eq!(spec.output.stdout(), StreamPolicy::Capture { limit: 4096 });
    assert_eq!(spec.output.stderr(), StreamPolicy::Capture { limit: 4096 });
    assert_eq!(spec.redaction, Redaction::All);
}

fn assert_stream_wiring(record: &Record, bytes: &[u8]) {
    let locus = Locus::new(2, 17, bytes.len() as u64);
    let decoded = record
        .decode(RecordStream::Stdout, bytes, locus)
        .expect("matching locus")
        .expect("selected record stream");
    assert!(
        decoded
            .events()
            .iter()
            .any(|event| event.kind() == EventKind::Prose)
    );
    assert_eq!(
        record
            .decode(
                RecordStream::Stderr,
                b"diagnostic",
                Locus::new(2, 17, b"diagnostic".len() as u64),
            )
            .expect("diagnostic stream is not decoded"),
        None
    );
}

fn process() -> Process {
    Process::new(
        "/worktree",
        Environment::new([("HOME", "/home/lane")]).expect("explicit environment"),
        Deadlines::new(
            Duration::from_secs(60),
            Duration::from_secs(1),
            Duration::from_secs(2),
        )
        .expect("bounded deadlines"),
        WaitPolicy::new(
            "vendor contract test",
            Cancellation::Never,
            RestartDisposition::CallerDecides,
        )
        .expect("owned wait"),
        4096,
        Duration::from_millis(100),
    )
    .expect("bounded process")
}
