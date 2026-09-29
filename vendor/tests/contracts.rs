use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::time::Duration;

use listen::{Dialect, EventKind, Locus};
use sill::{
    Cancellation, Deadlines, Environment, ProcessGroup, Redaction, RestartDisposition, StdinPolicy,
    StreamPolicy, WaitPolicy,
};
use vendor::{
    Attribution, ContractError, Effort, Launch, McpServer, Process, Record, RecordStream, Resume,
    SessionId, SteeringHook, SteeringKey, claude, codex, permission_request,
};
use wire::CanonicalUuid;

const CLAUDE_PROSE: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":7,"cache_read_input_tokens":5,"cache_creation_input_tokens":3,"output_tokens":2}}}"#;
const CLAUDE_SESSION: &[u8] =
    br#"{"type":"system","subtype":"init","session_id":"018f7777-7777-7444-8888-999999999999"}"#;
const CODEX_PROSE: &[u8] =
    br#"{"type":"item.completed","item":{"id":"item-1","type":"agent_message","text":"done"}}"#;
const CODEX_SESSION: &[u8] =
    br#"{"type":"thread.started","thread_id":"019fbc32-beb1-7ee0-b814-a7513f8f8ea5"}"#;

const HOOK_COMMAND: &str = "/usr/bin/spice agent post-tool-hook --repo-root /worktree";
const STEERING_TOKEN: &str = "1kJHh5bb";
const CLAUDE_MODEL: &str = "claude-opus-5";
const CODEX_MODEL: &str = "gpt-5-codex";
/// Stands in for the skill's instructions, which ride Claude's system prompt.
const SKILL: &str = "Answer steering with ACK before any tool.";
const CODEX_CONTEXT_WINDOW: u64 = 1_050_000;
/// The window Codex is told to compact against, well inside the model's.
const AUTO_COMPACT_WINDOW: u64 = 272_000;
/// An overlay only oh-my-pi has a launch seam for.
const CONFIG_OVERLAY: &str = "/supervisors/lane/occupant/omp.yml";
const MCP_COMMAND: &str = "npx";
const MCP_ARGS: [&str; 3] = ["@playwright/mcp@latest", "--isolated", "--headless"];

const CODEX_HOOK_OVERRIDE: &str = r#"hooks.PostToolUse=[{matcher="*",hooks=[{type="command",command="/usr/bin/spice agent post-tool-hook --repo-root /worktree",timeout=30,statusMessage="Checking spice steering"}]}]"#;
const CODEX_RTK_HOOK_OVERRIDE: &str = r#"hooks.PreToolUse=[{matcher="^Bash$",hooks=[{type="command",command="/usr/bin/spice agent post-tool-hook --repo-root /worktree",timeout=10,statusMessage="Checking RTK command rewrite"}]}]"#;
const CODEX_MCP_COMMAND_OVERRIDE: &str = r#"mcp_servers.playwright.command="npx""#;
const CODEX_MCP_ARGS_OVERRIDE: &str =
    r#"mcp_servers.playwright.args=["@playwright/mcp@latest","--isolated","--headless"]"#;

#[test]
fn codex_carries_every_launch_knob_and_resumes_after_its_flags() {
    let fresh = codex::record("/opt/codex", &fixture(CODEX_MODEL, Resume::Fresh))
        .expect("valid fresh record");
    assert_record(
        &fresh,
        "/opt/codex",
        &[
            "exec",
            "--cd",
            "/worktree",
            "--model",
            "gpt-5-codex",
            "--json",
            "--config",
            CODEX_RTK_HOOK_OVERRIDE,
            "--config",
            CODEX_HOOK_OVERRIDE,
            "--config",
            r#"model_reasoning_effort="high""#,
            "--config",
            "model_context_window=1050000",
            "--config",
            "model_auto_compact_token_limit=258400",
            "--config",
            CODEX_MCP_COMMAND_OVERRIDE,
            "--config",
            CODEX_MCP_ARGS_OVERRIDE,
            "--disable",
            "fast_mode",
            "--sandbox",
            "danger-full-access",
            "--dangerously-bypass-approvals-and-sandbox",
            "--dangerously-bypass-hook-trust",
            "start",
        ],
        Dialect::Codex,
        None,
    );

    let mut resuming = fixture(
        CODEX_MODEL,
        Resume::Session(SessionId::new("codex-thread").expect("session identity")),
    );
    resuming.fast_mode = Some(true);
    resuming.personality = Some("terse".to_owned());
    let resumed = codex::record("/opt/codex", &resuming).expect("valid resumed record");
    assert_record(
        &resumed,
        "/opt/codex",
        &[
            "exec",
            "--cd",
            "/worktree",
            "--model",
            "gpt-5-codex",
            "--json",
            "--config",
            CODEX_RTK_HOOK_OVERRIDE,
            "--config",
            CODEX_HOOK_OVERRIDE,
            "--config",
            r#"model_reasoning_effort="high""#,
            "--config",
            "model_context_window=1050000",
            "--config",
            "model_auto_compact_token_limit=258400",
            "--config",
            CODEX_MCP_COMMAND_OVERRIDE,
            "--config",
            CODEX_MCP_ARGS_OVERRIDE,
            "--config",
            r#"personality="terse""#,
            "--enable",
            "fast_mode",
            "--sandbox",
            "danger-full-access",
            "--dangerously-bypass-approvals-and-sandbox",
            "--dangerously-bypass-hook-trust",
            // `resume` is an `exec` subcommand, so it follows every exec flag
            // and takes the thread ahead of the prompt.
            "resume",
            "codex-thread",
            "start",
        ],
        Dialect::Codex,
        Some("codex-thread"),
    );
    assert_stream_wiring(&resumed, CODEX_PROSE);
}

#[test]
fn every_launch_closes_the_steering_path_end_to_end() {
    let launch = fixture(CLAUDE_MODEL, Resume::Fresh);
    let codex_launch = fixture(CODEX_MODEL, Resume::Fresh);

    // Delivery: both dialects install the hook that carries steering back in.
    let claude = claude::record("/opt/claude", &launch).expect("valid record");
    let codex = codex::record("/opt/codex", &codex_launch).expect("valid record");
    assert!(
        arguments(&claude)
            .iter()
            .any(|argument| argument.contains(HOOK_COMMAND)),
        "a Claude launch with no steering hook is an agent no operator can steer"
    );
    assert!(
        arguments(&codex)
            .iter()
            .any(|argument| argument.contains(HOOK_COMMAND)),
        "a Codex launch with no steering hook is an agent no operator can steer"
    );

    // Authentication: the token the agent will see wrapping live steering is
    // the token its system prompt names.
    let system_prompt = claude::system_prompt(&launch);
    assert!(system_prompt.contains(SKILL));
    assert!(system_prompt.contains(&format!(
        "Your spice steering key for this worktree is {STEERING_TOKEN}."
    )));
    assert!(system_prompt.contains(&format!("<{STEERING_TOKEN}> ... </{STEERING_TOKEN}>")));

    // The skill rides the system prompt, which compaction never summarizes;
    // each turn prompt re-anchors on the key and only names the skill.
    let claude_arguments = arguments(&claude);
    let append = claude_arguments
        .iter()
        .position(|argument| argument == "--append-system-prompt")
        .expect("Claude appends a system prompt");
    assert_eq!(claude_arguments[append + 1], system_prompt);
    let turn_prompt = claude_arguments.last().expect("Claude takes a turn prompt");
    assert_eq!(*turn_prompt, claude::turn_prompt(&launch));
    assert!(turn_prompt.contains("start"));
    assert!(turn_prompt.contains(&format!("<{STEERING_TOKEN}> ... </{STEERING_TOKEN}>")));
    assert!(!turn_prompt.contains(SKILL));

    // A launch with no key composes neither prompt with an anchor the agent was
    // never given.
    let mut anonymous = launch;
    anonymous.steering_key = None;
    assert!(!claude::system_prompt(&anonymous).contains(STEERING_TOKEN));
    assert!(!claude::turn_prompt(&anonymous).contains(STEERING_TOKEN));
}

#[test]
fn a_repository_that_refuses_claudes_trailer_gets_attribution_withheld() {
    for attribution in [Attribution::Harness, Attribution::Withheld] {
        let mut launch = fixture(CLAUDE_MODEL, Resume::Fresh);
        launch.attribution = attribution;
        let record = claude::record("/opt/claude", &launch).expect("valid record");
        let arguments = arguments(&record);
        let settings = arguments
            .windows(2)
            .find(|pair| pair[0] == "--settings")
            .expect("Claude receives settings");
        let settings: serde_json::Value =
            serde_json::from_str(&settings[1]).expect("Claude settings are a JSON object");
        match attribution {
            Attribution::Harness => assert!(settings.get("attribution").is_none()),
            Attribution::Withheld => assert_eq!(
                settings["attribution"],
                serde_json::json!({"commit": "", "sessionUrl": false})
            ),
        }
    }
}

#[test]
fn vendor_effort_rendering_carries_every_shared_value_once() {
    for effort in Effort::ALL {
        let mut claude_launch = fixture(CLAUDE_MODEL, Resume::Fresh);
        claude_launch.effort = *effort;
        let record = claude::record("/opt/claude", &claude_launch).expect("valid Claude record");
        let rendered = arguments(&record);
        assert_eq!(
            rendered
                .windows(2)
                .filter(|pair| pair == &["--effort", effort.as_str()])
                .count(),
            1,
            "Claude effort {:?} must render exactly once",
            effort
        );

        let mut codex_launch = fixture(CODEX_MODEL, Resume::Fresh);
        codex_launch.effort = *effort;
        let codex = codex::record("/opt/codex", &codex_launch).expect("valid Codex record");
        assert!(
            arguments(&codex).contains(&format!("model_reasoning_effort=\"{}\"", effort.as_str())),
        );
    }
}

#[test]
fn vendor_specific_knobs_refuse_instead_of_being_silently_ignored() {
    let mut claude_launch = fixture(CLAUDE_MODEL, Resume::Fresh);
    claude_launch.personality = Some("friendly".to_owned());
    assert_eq!(
        claude::record("/opt/claude", &claude_launch).expect_err("personality refuses"),
        ContractError::Unsupported {
            path: "vendor.claude.personality",
            vendor: "Claude",
        }
    );

    claude_launch.personality = None;
    claude_launch.fast_mode = Some(false);
    assert_eq!(
        claude::record("/opt/claude", &claude_launch).expect_err("fast mode refuses"),
        ContractError::Unsupported {
            path: "vendor.claude.fast_mode",
            vendor: "Claude",
        }
    );

    claude_launch.fast_mode = None;
    claude_launch.model_context_window = Some(CODEX_CONTEXT_WINDOW);
    assert_eq!(
        claude::record("/opt/claude", &claude_launch).expect_err("context window refuses"),
        ContractError::Unsupported {
            path: "vendor.claude.model_context_window",
            vendor: "Claude",
        }
    );

    claude_launch.model_context_window = None;
    claude_launch.config_overlay = Some(CONFIG_OVERLAY.into());
    assert_eq!(
        claude::record("/opt/claude", &claude_launch).expect_err("overlay refuses"),
        ContractError::Unsupported {
            path: "vendor.claude.config_overlay",
            vendor: "Claude",
        }
    );

    claude_launch.config_overlay = None;
    claude_launch.lsp = Some(true);
    assert_eq!(
        claude::record("/opt/claude", &claude_launch).expect_err("language servers refuse"),
        ContractError::Unsupported {
            path: "vendor.claude.lsp",
            vendor: "Claude",
        }
    );

    let mut codex_launch = fixture(CODEX_MODEL, Resume::Fresh);
    codex_launch.fast_mode = None;
    assert_eq!(
        codex::record("/opt/codex", &codex_launch).expect_err("omitted fast mode refuses"),
        ContractError::Missing {
            path: "vendor.codex.fast_mode",
        }
    );

    codex_launch.fast_mode = Some(false);
    codex_launch.model_context_window = None;
    assert_eq!(
        codex::record("/opt/codex", &codex_launch).expect_err("context window is required"),
        ContractError::Missing {
            path: "vendor.codex.model_context_window",
        }
    );

    codex_launch.model_context_window = Some(CODEX_CONTEXT_WINDOW);
    codex_launch.auto_compact_window_tokens = None;
    assert_eq!(
        codex::record("/opt/codex", &codex_launch).expect_err("compaction window is required"),
        ContractError::Missing {
            path: "vendor.codex.auto_compact_window_tokens",
        }
    );

    codex_launch.auto_compact_window_tokens = Some(AUTO_COMPACT_WINDOW);
    codex_launch.config_overlay = Some(CONFIG_OVERLAY.into());
    assert_eq!(
        codex::record("/opt/codex", &codex_launch).expect_err("overlay refuses"),
        ContractError::Unsupported {
            path: "vendor.codex.config_overlay",
            vendor: "Codex",
        }
    );

    codex_launch.config_overlay = None;
    codex_launch.lsp = Some(true);
    assert_eq!(
        codex::record("/opt/codex", &codex_launch).expect_err("language servers refuse"),
        ContractError::Unsupported {
            path: "vendor.codex.lsp",
            vendor: "Codex",
        }
    );
}

#[test]
fn codex_personality_is_rendered_as_one_toml_string() {
    let mut launch = fixture(CODEX_MODEL, Resume::Fresh);
    launch.personality = Some("direct \"operator\"\\nvoice".to_owned());

    let record = codex::record("/opt/codex", &launch).expect("personality renders");
    assert_arguments_contain(
        &record,
        &["personality=\"direct \\\"operator\\\"\\\\nvoice\""],
    );
}

#[test]
fn a_session_identity_stores_exactly_what_it_admitted() {
    let padded = SessionId::new("  codex-thread\n").expect("session identity");
    assert_eq!(padded.as_str(), "codex-thread");

    let mut launch = fixture(CODEX_MODEL, Resume::Fresh);
    launch.resume = Resume::Session(padded);
    let record = codex::record("/opt/codex", &launch).expect("valid record");
    assert!(
        arguments(&record).contains(&"codex-thread".to_owned()),
        "padding admitted for the emptiness check must not ride into --resume"
    );
    assert!(SessionId::new("  ").is_err());
}

#[test]
fn both_selected_record_contracts_admit_the_exact_typed_session_declaration() {
    let cases = [
        (
            claude::record("/opt/claude", &fixture(CLAUDE_MODEL, Resume::Fresh))
                .expect("Claude record"),
            CLAUDE_SESSION,
            "018f7777-7777-7444-8888-999999999999",
        ),
        (
            codex::record("/opt/codex", &fixture(CODEX_MODEL, Resume::Fresh))
                .expect("Codex record"),
            CODEX_SESSION,
            "019fbc32-beb1-7ee0-b814-a7513f8f8ea5",
        ),
    ];
    for (record, bytes, expected) in cases {
        let locus = Locus::new(7, 31, bytes.len() as u64);
        let decoded = record
            .decode(RecordStream::Stdout, bytes, locus)
            .expect("matching locus")
            .expect("selected stream");
        let declaration = record
            .session_declaration(&decoded)
            .expect("session identity is admitted")
            .expect("initialization record declares a session");
        assert_eq!(declaration.session.as_str(), expected);
        assert_eq!(declaration.locus, locus);
    }
}

#[test]
fn diagnostics_malformed_records_and_unrelated_events_never_create_session_authority() {
    let cases = [
        (
            claude::record("/opt/claude", &fixture(CLAUDE_MODEL, Resume::Fresh))
                .expect("Claude record"),
            CLAUDE_SESSION,
            CLAUDE_PROSE,
            br#"{"type":"system","subtype":"init"}"#.as_slice(),
            br#"{"type":"system","subtype":"init","session_id":""}"#.as_slice(),
        ),
        (
            codex::record("/opt/codex", &fixture(CODEX_MODEL, Resume::Fresh))
                .expect("Codex record"),
            CODEX_SESSION,
            CODEX_PROSE,
            br#"{"type":"thread.started"}"#.as_slice(),
            br#"{"type":"thread.started","thread_id":""}"#.as_slice(),
        ),
    ];
    for (record, session, unrelated, malformed, empty) in cases {
        let diagnostic_locus = Locus::new(8, 0, session.len() as u64);
        assert!(
            record
                .decode(RecordStream::Stderr, session, diagnostic_locus)
                .expect("diagnostic locus matches")
                .is_none()
        );

        for bytes in [unrelated, malformed] {
            let locus = Locus::new(9, 0, bytes.len() as u64);
            let decoded = record
                .decode(RecordStream::Stdout, bytes, locus)
                .expect("selected record locus matches")
                .expect("selected record is accounted");
            assert_eq!(
                record
                    .session_declaration(&decoded)
                    .expect("non-session record has no invalid identity"),
                None
            );
        }

        let locus = Locus::new(10, 0, empty.len() as u64);
        let decoded = record
            .decode(RecordStream::Stdout, empty, locus)
            .expect("empty declaration locus matches")
            .expect("selected declaration is decoded");
        assert!(record.session_declaration(&decoded).is_err());
    }
}

#[test]
fn a_thread_that_crossed_through_codex_is_re_dashed_for_claude() {
    assert_eq!(
        claude::dashed_uuid("018F7777777774448888999999999999"),
        "018f7777-7777-7444-8888-999999999999"
    );
    assert_eq!(
        claude::dashed_uuid("018f7777-7777-7444-8888-999999999999"),
        "018f7777-7777-7444-8888-999999999999"
    );
    // Not a UUID at all: some other vendor's spelling, passed through whole
    // rather than reshaped into something Claude would reject anyway.
    assert_eq!(claude::dashed_uuid("codex-thread"), "codex-thread");
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

fn fixture(model: &str, resume: Resume) -> Launch {
    Launch {
        cwd: "/worktree".into(),
        prompt: "start".to_owned(),
        skill: SKILL.to_owned(),
        model: model.to_owned(),
        effort: Effort::High,
        personality: None,
        fast_mode: (model == CODEX_MODEL).then_some(false),
        model_context_window: (model == CODEX_MODEL).then_some(CODEX_CONTEXT_WINDOW),
        auto_compact_window_tokens: (model == CODEX_MODEL).then_some(AUTO_COMPACT_WINDOW),
        config_overlay: None,
        lsp: None,
        attribution: Attribution::Harness,
        hook: SteeringHook {
            command: HOOK_COMMAND.to_owned(),
        },
        steering_key: Some(SteeringKey {
            token: STEERING_TOKEN.to_owned(),
        }),
        mcp: Some(McpServer {
            name: "playwright".to_owned(),
            command: MCP_COMMAND.to_owned(),
            args: MCP_ARGS.iter().map(|arg| (*arg).to_owned()).collect(),
        }),
        resume,
    }
}

fn arguments(record: &Record) -> Vec<String> {
    record
        .arguments()
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

/// Compare whole argument vectors and name whatever diverged.
///
/// A prefix or a length check would pass a launch that dropped `--sandbox` and
/// gained `--sandboxed`, so this reports every argument the launch lost and
/// every one it invented, by value, before checking their order.
fn assert_arguments(record: &Record, expected: &[&str]) {
    let actual = arguments(record);
    let mut tally: BTreeMap<&str, i64> = BTreeMap::new();
    for argument in expected {
        *tally.entry(argument).or_default() += 1;
    }
    for argument in &actual {
        *tally.entry(argument.as_str()).or_default() -= 1;
    }
    let dropped = tally
        .iter()
        .filter(|(_, count)| **count > 0)
        .map(|(argument, count)| format!("{count}x {argument:?}"))
        .collect::<Vec<_>>();
    let invented = tally
        .iter()
        .filter(|(_, count)| **count < 0)
        .map(|(argument, count)| format!("{}x {argument:?}", -count))
        .collect::<Vec<_>>();
    assert!(
        dropped.is_empty(),
        "launch dropped required arguments: {}",
        dropped.join(", ")
    );
    assert!(
        invented.is_empty(),
        "launch carried arguments the contract does not state: {}",
        invented.join(", ")
    );
    assert_eq!(
        actual, expected,
        "launch carried the right arguments in the wrong order"
    );
}

fn assert_arguments_contain(record: &Record, expected: &[&str]) {
    let actual = arguments(record);
    let window = expected.len();
    assert!(
        actual.windows(window).any(|slice| slice
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied())),
        "launch is missing the adjacent arguments {expected:?}, carrying {actual:?}"
    );
}

fn assert_record(
    record: &Record,
    binary: &str,
    arguments: &[&str],
    dialect: Dialect,
    session: Option<&str>,
) {
    assert_eq!(record.binary(), OsStr::new(binary));
    assert_arguments(record, arguments);
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
