//! oh-my-pi's RPC launch contract: the argument vector, the piped stdin it
//! opens, and the steering anchor its system prompt carries.

use std::ffi::OsStr;
use std::time::Duration;

use listen::{Dialect, EventKind, Locus};
use sill::{
    Cancellation, Deadlines, Environment, ProcessGroup, Redaction, RestartDisposition, StdinPolicy,
    StreamPolicy, WaitPolicy,
};
use vendor::{
    Attribution, ContractError, Effort, Launch, OMP, Process, RecordStream, Resume, SessionId,
    SteeringHook, SteeringKey, omp,
};

const SESSION: &[u8] = br#"{"id":"spice-state","type":"response","command":"get_state","success":true,"data":{"model":{"id":"claude-opus-5","contextWindow":1000000},"sessionId":"01a0a807-666f-7524-be0d-2ee476b6c15c","isStreaming":false}}"#;
const PROSE: &[u8] = br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"usage":{"input":7,"output":2,"cacheRead":5,"cacheWrite":3},"stopReason":"stop"}}"#;

const STEERING_TOKEN: &str = "1kJHh5bb";
const MODEL: &str = "claude-opus-5";

fn fixture(resume: Resume) -> Launch {
    Launch {
        cwd: "/worktree".into(),
        prompt: "start".to_owned(),
        model: MODEL.to_owned(),
        effort: Effort::XHigh,
        personality: None,
        fast_mode: None,
        model_context_window: None,
        attribution: Attribution::Harness,
        hook: SteeringHook {
            command: "/usr/bin/spice hook omp".to_owned(),
        },
        steering_key: Some(SteeringKey {
            token: STEERING_TOKEN.to_owned(),
        }),
        mcp: None,
        resume,
    }
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

#[test]
fn omp_carries_every_launch_knob_and_opens_the_conversation_over_a_piped_stdin() {
    let launch = fixture(Resume::Fresh);
    let system_prompt = omp::system_prompt(&launch);
    let record = omp::record("/opt/omp", &launch).expect("valid fresh record");
    assert_eq!(record.binary(), OsStr::new("/opt/omp"));
    let expected = [
        "--mode",
        "rpc",
        "--approval-mode",
        "yolo",
        "--no-title",
        "--model",
        "claude-opus-5",
        "--thinking",
        "xhigh",
        "--append-system-prompt",
        &system_prompt,
    ];
    let actual: Vec<_> = record
        .arguments()
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(record.dialect(), Dialect::Omp);
    assert_eq!(record.stream(), RecordStream::Stdout);
    assert_eq!(record.resume().session_id(), None);

    let spec = record.spec(&process());
    assert_eq!(spec.argv.program(), record.binary());
    assert_eq!(spec.group, ProcessGroup::New);
    assert_eq!(spec.output.stdin(), StdinPolicy::Pipe);
    assert_eq!(spec.output.stdout(), StreamPolicy::Capture { limit: 4096 });
    assert_eq!(spec.output.stderr(), StreamPolicy::Capture { limit: 4096 });
    assert_eq!(spec.redaction, Redaction::All);

    let opening = record.opening().expect("omp opens over stdin");
    assert_eq!(opening, omp::opening_request(&launch).as_slice());
    let lines: Vec<serde_json::Value> = opening
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("one JSON request per line"))
        .collect();
    assert_eq!(
        lines,
        [
            serde_json::json!({"id": "spice-state", "type": "get_state"}),
            serde_json::json!({"id": "spice-prompt", "type": "prompt", "message": system_prompt}),
        ]
    );
    assert!(opening.ends_with(b"\n"));
}

#[test]
fn omp_resumes_by_the_exact_session_and_the_vendor_value_renders_the_same_record() {
    let launch = fixture(Resume::Session(
        SessionId::new("01a0a807-666f-7524-be0d-2ee476b6c15c").expect("session identity"),
    ));
    let record = omp::record("/opt/omp", &launch).expect("valid resumed record");
    let arguments: Vec<_> = record
        .arguments()
        .iter()
        .map(|a| a.to_string_lossy())
        .collect();
    assert_eq!(
        &arguments[arguments.len() - 2..],
        ["--resume", "01a0a807-666f-7524-be0d-2ee476b6c15c"]
    );
    assert_eq!(
        record.resume().session_id(),
        Some("01a0a807-666f-7524-be0d-2ee476b6c15c")
    );
    assert_eq!(
        OMP.record("/opt/omp", &launch)
            .expect("vendor value renders"),
        record
    );
}

#[test]
fn omp_refuses_knobs_it_has_no_seam_for_instead_of_ignoring_them() {
    let mut personality = fixture(Resume::Fresh);
    personality.personality = Some("terse".to_owned());
    let mut fast_mode = fixture(Resume::Fresh);
    fast_mode.fast_mode = Some(true);
    let mut window = fixture(Resume::Fresh);
    window.model_context_window = Some(1_000_000);
    for (launch, path) in [
        (personality, "vendor.omp.personality"),
        (fast_mode, "vendor.omp.fast_mode"),
        (window, "vendor.omp.model_context_window"),
    ] {
        assert_eq!(
            omp::record("/opt/omp", &launch).expect_err("refused"),
            ContractError::Unsupported {
                path,
                vendor: "oh-my-pi"
            }
        );
    }
    assert!(matches!(
        omp::record("", &fixture(Resume::Fresh)),
        Err(ContractError::Spec(_))
    ));
}

#[test]
fn the_steering_anchor_names_the_interjection_channel_and_steer_requests_carry_it() {
    let launch = fixture(Resume::Fresh);
    let system_prompt = omp::system_prompt(&launch);
    assert!(system_prompt.starts_with(vendor::claude::SKILL_SYSTEM_PROMPT_PREAMBLE));
    assert!(system_prompt.contains("\n\nstart\n\n"));
    assert!(system_prompt.contains(&format!("<{STEERING_TOKEN}>")));
    assert!(system_prompt.contains("interjected user message"));
    assert!(!system_prompt.contains("stderr"));

    let mut anonymous = launch.clone();
    anonymous.steering_key = None;
    assert!(!omp::system_prompt(&anonymous).contains(STEERING_TOKEN));

    let steer = omp::steer_request(3, "<1kJHh5bb>\ndirective=d1\nstop\n</1kJHh5bb>\n");
    let frame: serde_json::Value =
        serde_json::from_slice(steer.strip_suffix(b"\n").expect("one line")).expect("JSON");
    assert_eq!(
        frame,
        serde_json::json!({
            "id": "spice-steer-3",
            "type": "prompt",
            "message": "<1kJHh5bb>\ndirective=d1\nstop\n</1kJHh5bb>\n",
            "streamingBehavior": "steer",
        })
    );
}

#[test]
fn the_state_reply_declares_the_session_and_content_rides_the_selected_stream() {
    let record = omp::record("/opt/omp", &fixture(Resume::Fresh)).expect("valid record");
    let locus = Locus::new(2, 17, SESSION.len() as u64);
    let decoded = record
        .decode(RecordStream::Stdout, SESSION, locus)
        .expect("matching locus")
        .expect("selected stream");
    let declaration = record
        .session_declaration(&decoded)
        .expect("nonempty identity")
        .expect("the state reply declares the session");
    assert_eq!(
        declaration.session.as_str(),
        "01a0a807-666f-7524-be0d-2ee476b6c15c"
    );
    assert_eq!(declaration.locus, locus);

    let prose = record
        .decode(
            RecordStream::Stdout,
            PROSE,
            Locus::new(2, 17, PROSE.len() as u64),
        )
        .expect("matching locus")
        .expect("selected stream");
    assert!(prose.events().iter().any(|e| e.kind() == EventKind::Prose));
    assert_eq!(
        record
            .decode(RecordStream::Stderr, b"diag", Locus::new(2, 17, 4))
            .expect("diagnostics are not decoded"),
        None
    );
}
