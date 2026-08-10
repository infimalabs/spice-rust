use std::ffi::OsString;
use std::path::PathBuf;

use sill::Environment;
use spice::agent_launch::{
    CLAUDE_ATTRIBUTION_TRAILER, CLAUDE_AUTO_COMPACT_WINDOW, ComposeError, Inputs, SPICE_RS_LANE,
    compose,
};
use spice::config::Root;
use vendor::{Attribution, Effort, Resume, SessionId};
use wire::{CanonicalUuid, Name};

const LANE: &str = "018f82d4-6f59-7d06-92e3-bce9131163c7";

fn root(document: &str) -> Root {
    let deserializer = toml::Deserializer::parse(document).expect("launch fixture TOML parses");
    serde_path_to_error::deserialize(deserializer).expect("launch fixture root admits")
}

fn inputs(vendor: &str, environment: Environment, resume: Resume) -> Inputs {
    Inputs {
        worktree: PathBuf::from("/work/tree"),
        vendor: vendor.parse::<Name>().expect("fixture vendor name"),
        prompt: "[$spice](.agents/skills/spice/SKILL.md)".to_owned(),
        executable: "/opt/spice's bin".to_owned(),
        environment,
        lane: LANE.parse::<CanonicalUuid>().expect("fixture lane"),
        resume,
    }
}

fn environment_value<'a>(environment: &'a Environment, key: &str) -> Option<&'a str> {
    environment
        .entries()
        .iter()
        .find(|(candidate, _)| candidate == key)
        .and_then(|(_, value)| value.to_str())
}

fn arguments(record: &vendor::Record) -> Vec<String> {
    record
        .arguments()
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn claude_plan_materializes_shared_fields_policy_and_one_compact_window() {
    let root = root(
        r#"
[policy.commit_message]
allowed_trailers = ["Spice-Origin"]

[vendor.claude]
binary = "/opt/claude"
model = "opus"
effort = "max"
auto_compact_window_tokens = 123456
"#,
    );
    let plan = compose(
        &root,
        inputs(
            "claude",
            Environment::new([("HOME", "/home/operator")]).expect("fixture environment"),
            Resume::Fresh,
        ),
    )
    .expect("Claude plan composes");
    let launch = plan.launch();

    assert_eq!(plan.binary(), "/opt/claude");
    assert_eq!(launch.model, "opus");
    assert_eq!(launch.effort, Effort::Max);
    assert_eq!(launch.attribution, Attribution::Withheld);
    assert_eq!(
        root.policy()
            .commit_message()
            .trailer_rejection(CLAUDE_ATTRIBUTION_TRAILER),
        Some(gate::TrailerRejection::Disallowed)
    );
    assert_eq!(launch.hook.command, "'/opt/spice'\\''s bin' hook claude");
    assert_eq!(launch.mcp.name, "spice");
    assert_eq!(launch.mcp.command, "/opt/spice's bin");
    assert_eq!(launch.mcp.args, ["mcp"]);
    assert_eq!(
        launch
            .steering_key
            .as_ref()
            .expect("steering key is materialized")
            .token,
        LANE
    );
    assert_eq!(
        environment_value(plan.environment(), CLAUDE_AUTO_COMPACT_WINDOW),
        Some("123456")
    );
    assert_eq!(
        plan.environment()
            .entries()
            .iter()
            .filter(|(key, _)| key == CLAUDE_AUTO_COMPACT_WINDOW)
            .count(),
        1
    );
    assert_eq!(
        environment_value(plan.environment(), SPICE_RS_LANE),
        Some(LANE)
    );

    let rendered = arguments(&plan.record().expect("Claude plan renders"));
    assert_eq!(
        rendered
            .windows(2)
            .filter(|pair| pair == &["--effort", "max"])
            .count(),
        1
    );
    let settings = rendered
        .windows(2)
        .find_map(|pair| (pair[0] == "--settings").then_some(pair[1].as_str()))
        .expect("Claude settings argument exists");
    assert!(settings.contains(r#""attribution":{"commit":"","sessionUrl":false}"#));
}

#[test]
fn inherited_claude_compact_window_is_preserved_exactly_once() {
    let inherited = Environment::new([
        ("HOME", "/home/operator"),
        (CLAUDE_AUTO_COMPACT_WINDOW, "777"),
    ])
    .expect("fixture environment");
    let plan = compose(&Root::default(), inputs("claude", inherited, Resume::Fresh))
        .expect("Claude plan composes");

    assert_eq!(
        environment_value(plan.environment(), CLAUDE_AUTO_COMPACT_WINDOW),
        Some("777")
    );
    assert_eq!(
        plan.environment()
            .entries()
            .iter()
            .filter(|(key, _)| key == CLAUDE_AUTO_COMPACT_WINDOW)
            .count(),
        1
    );
}

#[test]
fn codex_plan_renders_admitted_personality_and_explicit_fast_mode() {
    let root = root(
        r#"
[vendor.codex]
binary = "/opt/codex"
model = "gpt-current"
effort = "max"
personality = "pragmatic"
fast_mode = true
"#,
    );
    let plan = compose(&root, inputs("codex", Environment::empty(), Resume::Fresh))
        .expect("Codex plan composes");
    let rendered = arguments(&plan.record().expect("Codex plan renders"));

    assert_eq!(
        plan.launch().hook.command,
        "'/opt/spice'\\''s bin' hook codex"
    );
    assert_eq!(plan.launch().personality.as_deref(), Some("pragmatic"));
    assert_eq!(plan.launch().fast_mode, Some(true));
    assert!(
        rendered
            .windows(2)
            .any(|pair| pair == ["--enable", "fast_mode"])
    );
    assert!(
        rendered
            .windows(2)
            .any(|pair| { pair[0] == "--config" && pair[1] == r#"personality="pragmatic""# })
    );
    assert_eq!(
        environment_value(plan.environment(), CLAUDE_AUTO_COMPACT_WINDOW),
        None
    );
}

#[test]
fn fresh_resume_and_recomposition_keep_the_same_admitted_policy() {
    let root = root(
        r#"
[vendor.codex]
model = "gpt-stable"
effort = "high"
fast_mode = false
"#,
    );
    let environment = Environment::new([("PATH", "/usr/bin")]).expect("fixture environment admits");
    let fresh = compose(&root, inputs("codex", environment.clone(), Resume::Fresh))
        .expect("fresh plan composes");
    let restarted = compose(&root, inputs("codex", environment.clone(), Resume::Fresh))
        .expect("restart plan composes");
    assert_eq!(restarted, fresh);

    let resumed = compose(
        &root,
        inputs(
            "codex",
            environment,
            Resume::Session(SessionId::new("thread-stable").expect("session id")),
        ),
    )
    .expect("resume plan composes");
    assert_eq!(resumed.binary(), fresh.binary());
    assert_eq!(resumed.environment(), fresh.environment());
    assert_eq!(resumed.launch().model, fresh.launch().model);
    assert_eq!(resumed.launch().effort, fresh.launch().effort);
    assert_eq!(resumed.launch().personality, fresh.launch().personality);
    assert_eq!(resumed.launch().fast_mode, fresh.launch().fast_mode);
    assert_eq!(resumed.launch().hook, fresh.launch().hook);
    assert_eq!(resumed.launch().mcp, fresh.launch().mcp);
    assert_eq!(resumed.launch().steering_key, fresh.launch().steering_key);
}

#[test]
fn composition_refuses_unsupported_vendor_and_reserved_lane_environment() {
    let unsupported = compose(
        &Root::default(),
        inputs("gemini", Environment::empty(), Resume::Fresh),
    )
    .expect_err("unsupported vendor refuses");
    assert_eq!(
        unsupported,
        ComposeError::UnsupportedVendor("gemini".to_owned())
    );

    let reserved = Environment::new([(OsString::from(SPICE_RS_LANE), OsString::from(LANE))])
        .expect("reserved fixture environment itself admits");
    let error = compose(&Root::default(), inputs("codex", reserved, Resume::Fresh))
        .expect_err("caller-supplied lane identity refuses");
    assert_eq!(error, ComposeError::ReservedEnvironment(SPICE_RS_LANE));
}
