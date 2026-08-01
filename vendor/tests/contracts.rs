use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::time::Duration;

use listen::{Dialect, EventKind, Locus};
use sill::{
    Cancellation, Deadlines, Environment, ProcessGroup, Redaction, RestartDisposition, StdinPolicy,
    StreamPolicy, WaitPolicy,
};
use vendor::{
    Attribution, Effort, Launch, McpServer, Process, Record, RecordStream, Resume, SessionId,
    SteeringHook, SteeringKey, claude, codex, permission_request,
};
use wire::CanonicalUuid;

const CLAUDE_PROSE: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":7,"cache_read_input_tokens":5,"cache_creation_input_tokens":3,"output_tokens":2}}}"#;
const CODEX_PROSE: &[u8] =
    br#"{"type":"item.completed","item":{"id":"item-1","type":"agent_message","text":"done"}}"#;

const HOOK_COMMAND: &str = "/usr/bin/spice agent post-tool-hook --repo-root /worktree";
const STEERING_TOKEN: &str = "1kJHh5bb";
const CLAUDE_MODEL: &str = "claude-opus-5";
const CODEX_MODEL: &str = "gpt-5-codex";
const MCP_COMMAND: &str = "npx";
const MCP_ARGS: [&str; 2] = ["@playwright/mcp@latest", "--isolated"];

/// The `--mcp-config` document, spelled out rather than rebuilt from the code
/// under test, so a change to either one has to be stated here to pass.
const CLAUDE_MCP_CONFIG: &str = r#"{"mcpServers":{"playwright":{"command":"npx","args":["@playwright/mcp@latest","--isolated"]}}}"#;
const CLAUDE_SETTINGS: &str = r#"{"hooks":{"PostToolUse":[{"hooks":[{"command":"/usr/bin/spice agent post-tool-hook --repo-root /worktree","statusMessage":"Checking spice steering","timeout":30,"type":"command"}],"matcher":"*"}]},"permissions":{"deny":["Task","Agent","TaskCreate","TaskGet","TaskList","TaskUpdate","TaskOutput","TaskStop","Monitor"]}}"#;
const CLAUDE_SETTINGS_WITHOUT_ATTRIBUTION: &str = r#"{"attribution":{"commit":"","sessionUrl":false},"hooks":{"PostToolUse":[{"hooks":[{"command":"/usr/bin/spice agent post-tool-hook --repo-root /worktree","statusMessage":"Checking spice steering","timeout":30,"type":"command"}],"matcher":"*"}]},"permissions":{"deny":["Task","Agent","TaskCreate","TaskGet","TaskList","TaskUpdate","TaskOutput","TaskStop","Monitor"]}}"#;

const CODEX_HOOK_OVERRIDE: &str = r#"hooks.PostToolUse=[{matcher="^(Bash|apply_patch|Edit|Write|mcp__.*)$",hooks=[{type="command",command="/usr/bin/spice agent post-tool-hook --repo-root /worktree",timeout=30,statusMessage="Checking spice steering"}]}]"#;
const CODEX_MCP_COMMAND_OVERRIDE: &str = r#"mcp_servers.playwright.command="npx""#;
const CODEX_MCP_ARGS_OVERRIDE: &str =
    r#"mcp_servers.playwright.args=["@playwright/mcp@latest","--isolated"]"#;

#[test]
fn claude_carries_every_launch_knob_and_the_system_prompt_it_re_grounds_in() {
    let launch = fixture(CLAUDE_MODEL, Resume::Fresh);
    let system_prompt = claude::system_prompt(&launch);
    let fresh = claude::record("/opt/claude", &launch).expect("valid fresh record");
    assert_record(
        &fresh,
        "/opt/claude",
        &[
            "--print",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--model",
            "claude-opus-5",
            "--permission-mode",
            "bypassPermissions",
            "--mcp-config",
            CLAUDE_MCP_CONFIG,
            "--settings",
            CLAUDE_SETTINGS,
            "--append-system-prompt",
            &system_prompt,
            "--effort",
            "high",
            // The trailing positional is the composed system prompt, not the
            // bare start prompt: the agent re-reads the skill every turn.
            &system_prompt,
        ],
        Dialect::Claude,
        None,
    );

    let resumed = claude::record(
        "/opt/claude",
        &fixture(
            CLAUDE_MODEL,
            Resume::Session(
                SessionId::new("018f7777777774448888999999999999").expect("session identity"),
            ),
        ),
    )
    .expect("valid resumed record");
    assert_arguments_contain(
        &resumed,
        &["--resume", "018f7777-7777-7444-8888-999999999999"],
    );
    assert_eq!(
        resumed.arguments().last().expect("a trailing prompt"),
        OsStr::new(&system_prompt),
        "resume must not displace the system prompt from the trailing position"
    );
    assert_stream_wiring(&resumed, CLAUDE_PROSE);
}

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
            "--config",
            CODEX_HOOK_OVERRIDE,
            "--config",
            r#"model_reasoning_effort="high""#,
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
    resuming.fast_mode = true;
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
            "--config",
            CODEX_HOOK_OVERRIDE,
            "--config",
            r#"model_reasoning_effort="high""#,
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

    // Delivery: both dialects install the hook that carries steering back in.
    let claude = claude::record("/opt/claude", &launch).expect("valid record");
    let codex = codex::record("/opt/codex", &launch).expect("valid record");
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
    assert!(system_prompt.starts_with(claude::SKILL_SYSTEM_PROMPT_PREAMBLE));
    assert!(system_prompt.contains("start"));
    assert!(system_prompt.contains(&format!(
        "Your spice steering key for this worktree is {STEERING_TOKEN}."
    )));
    assert!(system_prompt.contains(&format!("<{STEERING_TOKEN}> ... </{STEERING_TOKEN}>")));

    // A launch with no key composes the skill and the prompt alone rather than
    // naming an anchor the agent was never given.
    let mut anonymous = launch;
    anonymous.steering_key = None;
    assert!(!claude::system_prompt(&anonymous).contains(STEERING_TOKEN));
}

#[test]
fn a_repository_that_refuses_claudes_trailer_gets_attribution_withheld() {
    let hook = SteeringHook {
        command: HOOK_COMMAND.to_owned(),
    };
    assert_eq!(
        claude::settings_json(&hook, Attribution::Harness),
        CLAUDE_SETTINGS
    );
    assert_eq!(
        claude::settings_json(&hook, Attribution::Withheld),
        CLAUDE_SETTINGS_WITHOUT_ATTRIBUTION
    );

    let mut launch = fixture(CLAUDE_MODEL, Resume::Fresh);
    launch.attribution = Attribution::Withheld;
    let withheld = claude::record("/opt/claude", &launch).expect("valid record");
    assert_arguments_contain(
        &withheld,
        &["--settings", CLAUDE_SETTINGS_WITHOUT_ATTRIBUTION],
    );
}

#[test]
fn both_vendors_carry_every_configured_effort() {
    for effort in Effort::ALL {
        let mut launch = fixture(CLAUDE_MODEL, Resume::Fresh);
        launch.effort = *effort;
        let record = claude::record("/opt/claude", &launch).expect("valid record");
        assert_arguments_contain(&record, &["--effort", effort.as_str()]);

        let codex = codex::record("/opt/codex", &launch).expect("valid record");
        assert!(
            arguments(&codex).contains(&format!("model_reasoning_effort=\"{}\"", effort.as_str())),
        );
    }
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
        model: model.to_owned(),
        effort: Effort::High,
        personality: None,
        fast_mode: false,
        attribution: Attribution::Harness,
        hook: SteeringHook {
            command: HOOK_COMMAND.to_owned(),
        },
        steering_key: Some(SteeringKey {
            token: STEERING_TOKEN.to_owned(),
        }),
        mcp: McpServer {
            name: "playwright".to_owned(),
            command: MCP_COMMAND.to_owned(),
            args: MCP_ARGS.iter().map(|arg| (*arg).to_owned()).collect(),
        },
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
