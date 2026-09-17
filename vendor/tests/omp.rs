//! oh-my-pi's RPC launch contract: the argument vector, the piped stdin it
//! opens, and the steering anchor its system prompt carries.

use std::ffi::{OsStr, OsString};
use std::path::Path;
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
/// Width of the title slot omp rewrites in place at the top of a session file.
const TITLE_SLOT_BYTES: usize = 256;
const MODEL: &str = "claude-opus-5";

fn fixture(resume: Resume) -> Launch {
    Launch {
        cwd: "/worktree".into(),
        prompt: "start".to_owned(),
        model: MODEL.to_owned(),
        effort: Effort::XHigh,
        personality: None,
        fast_mode: Some(false),
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
        "--service-tier",
        "none",
        "--tools",
        "read,write,edit,bash,glob,grep,eval,todo,hub,web_search",
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
    assert!(
        record.environment().is_empty(),
        "a fresh launch pins nothing"
    );

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
        record.environment(),
        [(
            OsString::from(omp::SESSION_ENVIRONMENT),
            OsString::from("01a0a807-666f-7524-be0d-2ee476b6c15c"),
        )],
        "the identity rides the child environment because omp exports none"
    );
    assert_eq!(OMP.thread_id_environment(), omp::SESSION_ENVIRONMENT);
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
    let mut window = fixture(Resume::Fresh);
    window.model_context_window = Some(1_000_000);
    for (launch, path) in [
        (personality, "vendor.omp.personality"),
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
fn fast_mode_is_the_priority_service_tier_and_must_be_explicit() {
    let mut fast = fixture(Resume::Fresh);
    fast.fast_mode = Some(true);
    let record = omp::record("/opt/omp", &fast).expect("valid fast record");
    let tier = record
        .arguments()
        .windows(2)
        .find(|pair| pair[0] == OsStr::new("--service-tier"))
        .map(|pair| pair[1].as_os_str());
    assert_eq!(tier, Some(OsStr::new("priority")));

    let mut implicit = fixture(Resume::Fresh);
    implicit.fast_mode = None;
    assert_eq!(
        omp::record("/opt/omp", &implicit).expect_err("refused"),
        ContractError::Missing {
            path: "vendor.omp.fast_mode"
        }
    );
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
fn a_drain_interrupts_the_turn_with_one_abort_request() {
    let abort = omp::abort_request();
    let frame: serde_json::Value =
        serde_json::from_slice(abort.strip_suffix(b"\n").expect("one line")).expect("JSON");
    assert_eq!(
        frame,
        serde_json::json!({"id": "spice-abort", "type": "abort"})
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

#[test]
fn a_pinned_session_lands_where_omp_keeps_this_worktree_and_reads_back_as_its_own() {
    let at = "2026-09-16T06:43:21.805Z";
    let identity = "01a0a8f4-aa4d-7581-a92c-d7ddf0405263";
    let sessions = Path::new("/home/lane/.omp/agent/sessions");
    let home = Path::new("/home/lane");
    let temp = Path::new("/private/tmp");
    let (path, contents) = omp::session_file(
        sessions,
        home,
        temp,
        Path::new("/home/lane/devel/tree"),
        identity,
        at,
    );
    assert_eq!(
        path,
        sessions
            .join("-devel-tree")
            .join(format!("2026-09-16T06-43-21-805Z_{identity}.jsonl"))
    );
    let (slot, header) = contents.split_once('\n').expect("a slot, then the header");
    assert_eq!(
        slot.len() + 1,
        TITLE_SLOT_BYTES,
        "the title slot keeps omp's fixed width"
    );
    let slot: serde_json::Value = serde_json::from_str(slot).expect("the slot is JSON");
    assert_eq!(slot["type"], "title");
    assert_eq!(slot["title"], "");
    assert_eq!(slot["updatedAt"], at);
    let header: serde_json::Value = serde_json::from_str(header).expect("the header is JSON");
    assert_eq!(header["type"], "session");
    assert_eq!(header["version"], 3);
    assert_eq!(header["id"], identity);
    assert_eq!(header["timestamp"], at);
    assert_eq!(header["cwd"], "/home/lane/devel/tree");
    assert!(contents.ends_with('\n'));

    // The file already reads as omp's own: the slot is silent, the header declares.
    let lines: Vec<_> = contents.lines().collect();
    let slot = listen::decode(
        Dialect::Omp,
        lines[0].as_bytes(),
        Locus::new(1, 0, lines[0].len() as u64),
    )
    .expect("locus");
    assert!(slot.events().is_empty() && !matches!(slot, listen::Decoded::Residue(_)));
    let header = listen::decode(
        Dialect::Omp,
        lines[1].as_bytes(),
        Locus::new(1, 0, lines[1].len() as u64),
    )
    .expect("locus");
    assert!(
        matches!(header.events(), [listen::Event::Session(declaration)] if declaration.identity == identity)
    );

    // The three roots omp tells apart: home itself, the temporary tree, anywhere else.
    for (cwd, directory) in [
        ("/home/lane", "-"),
        ("/private/tmp/scratch/tree", "-tmp-scratch-tree"),
        ("/srv/tree", "--srv-tree--"),
    ] {
        let (path, _) = omp::session_file(sessions, home, temp, Path::new(cwd), identity, at);
        assert_eq!(
            path.parent(),
            Some(sessions.join(directory).as_path()),
            "{cwd}"
        );
    }
}
