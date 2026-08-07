use super::*;

/// The vendor identifier an operator must never be shown.
const VENDOR_ID: &str = "toolu_01ABCdefGHIjklMNOpqrs";

const BASH_INPUT: &str = r#"{"command":"git status --short","description":"read the tree"}"#;

fn lane(value: u8) -> CanonicalUuid {
    format!("00000000-0000-4000-8000-{value:012x}")
        .parse()
        .expect("fixture uuid is canonical")
}

fn tool(call_id: &str, moment: listen::ToolMoment) -> listen::Tool {
    listen::Tool {
        call_id: call_id.to_owned(),
        moment,
        locus: listen::Locus::new(4, 120, 48),
    }
}

fn call(name: &str, arguments: &str) -> listen::ToolMoment {
    listen::ToolMoment::Call {
        name: name.to_owned(),
        arguments: arguments.to_owned(),
    }
}

fn update(name: &str, arguments: &str) -> listen::ToolMoment {
    listen::ToolMoment::Update {
        name: name.to_owned(),
        arguments: arguments.to_owned(),
    }
}

fn succeeded() -> listen::ToolMoment {
    listen::ToolMoment::Result(listen::ToolResult::Succeeded)
}

/// A call reads as the work being done, not as the tool's bare name.
///
/// The strip exists so an operator can tell one shell call from the next, and
/// `Bash` alone cannot do that. The description field is in the payload
/// deliberately: a rule that showed any string it found would show it too.
#[test]
fn a_call_carries_the_command_line_and_not_merely_its_tool_name() {
    let mut previews = CallPreviews::default();

    assert_eq!(
        activity(
            lane(1),
            &tool(VENDOR_ID, call("Bash", BASH_INPUT)),
            &mut previews
        ),
        "Bash: git status --short"
    );
}

/// A result names the call it answers, correlating across records to do it.
///
/// The result event carries the vendor id alone, so the only way to render a
/// name is to remember the call. Asserting the id is absent as well as
/// asserting the text keeps a rule that appended the id from passing.
#[test]
fn a_result_names_its_call_and_never_paints_the_vendor_identifier() {
    let mut previews = CallPreviews::default();
    activity(
        lane(1),
        &tool(VENDOR_ID, call("Bash", BASH_INPUT)),
        &mut previews,
    );

    let result = activity(lane(1), &tool(VENDOR_ID, succeeded()), &mut previews);

    assert_eq!(result, "Bash: git status --short succeeded");
    assert!(
        !result.contains(VENDOR_ID),
        "an operator cannot act on a vendor correlation id: {result}"
    );
}

/// Every outcome word survives the correlation, and a failure still says so.
#[test]
fn each_reported_outcome_reaches_the_strip_beside_the_call_it_answers() {
    for (outcome, expected) in [
        (listen::ToolResult::Succeeded, "Read: /etc/hosts succeeded"),
        (listen::ToolResult::Failed, "Read: /etc/hosts failed"),
        (listen::ToolResult::Unreported, "Read: /etc/hosts completed"),
    ] {
        let mut previews = CallPreviews::default();
        activity(
            lane(1),
            &tool(VENDOR_ID, call("Read", r#"{"path":"/etc/hosts"}"#)),
            &mut previews,
        );

        assert_eq!(
            activity(
                lane(1),
                &tool(VENDOR_ID, listen::ToolMoment::Result(outcome)),
                &mut previews
            ),
            expected
        );
    }
}

/// A result whose call was never seen is anonymous, not opaque.
///
/// A call outstanding across a resident restart, or one whose record this lane
/// never selected, leaves nothing to correlate against. The strip still must
/// not fall back to the vendor id.
#[test]
fn an_uncorrelated_result_reads_as_an_anonymous_tool() {
    let mut previews = CallPreviews::default();

    let result = activity(
        lane(1),
        &tool(
            VENDOR_ID,
            listen::ToolMoment::Result(listen::ToolResult::Failed),
        ),
        &mut previews,
    );

    assert_eq!(result, "tool failed");
    assert!(
        !result.contains(VENDOR_ID),
        "an unmatched result must stay anonymous rather than opaque: {result}"
    );
}

/// One resident watches every lane, so a shared call id must not cross lanes.
///
/// A vendor identifier is unique within one transcript, not across the fleet. A
/// correlation keyed on the id alone would paint one lane's strip with another
/// lane's command, which is worse than painting nothing.
#[test]
fn one_call_identifier_reused_by_two_lanes_keeps_its_lanes_apart() {
    let mut previews = CallPreviews::default();
    activity(
        lane(1),
        &tool(VENDOR_ID, call("Bash", BASH_INPUT)),
        &mut previews,
    );

    assert_eq!(
        activity(lane(2), &tool(VENDOR_ID, succeeded()), &mut previews),
        "tool succeeded",
        "another lane's call must not name this lane's result"
    );
    assert_eq!(
        activity(lane(1), &tool(VENDOR_ID, succeeded()), &mut previews),
        "Bash: git status --short succeeded",
        "the owning lane still resolves its own call"
    );
}

/// The correlation table holds in-flight calls only.
///
/// A resident runs for days. If a result did not take its entry, this map would
/// grow one entry per call ever observed and never shrink.
#[test]
fn a_call_preview_is_forgotten_once_its_result_has_landed() {
    let mut previews = CallPreviews::default();
    activity(
        lane(1),
        &tool(VENDOR_ID, call("Bash", BASH_INPUT)),
        &mut previews,
    );
    activity(lane(1), &tool(VENDOR_ID, succeeded()), &mut previews);

    assert!(
        previews.by_call.is_empty(),
        "an answered call is released: {previews:?}"
    );
    assert_eq!(
        activity(lane(1), &tool(VENDOR_ID, succeeded()), &mut previews),
        "tool succeeded",
        "a redelivered result must not resurrect a call already answered"
    );
}

/// An update says the call is still running and carries its arguments.
///
/// It also republishes the correlation, because a dialect that only ever emits
/// updates for a long-running call would otherwise leave its result anonymous.
#[test]
fn an_update_reports_the_running_call_and_still_answers_its_result() {
    let mut previews = CallPreviews::default();

    assert_eq!(
        activity(
            lane(1),
            &tool(
                VENDOR_ID,
                update("command_execution", r#"{"command":"cargo test"}"#)
            ),
            &mut previews
        ),
        "command execution: cargo test update"
    );
    assert_eq!(
        activity(lane(1), &tool(VENDOR_ID, succeeded()), &mut previews),
        "command execution: cargo test succeeded"
    );
}

/// The argument shown is chosen, not stumbled onto.
///
/// Each case is a payload shape a real dialect emits: an argv array, a named
/// field, a field no rule names, a bare string, an empty payload, a payload
/// with no text in it at all, and one this resident cannot parse.
#[test]
fn the_argument_preview_follows_the_shape_the_vendor_sent() {
    for (arguments, expected) in [
        (
            r#"{"command":["cargo","test","-p","spice"]}"#,
            "cargo test -p spice",
        ),
        (r#"{"query":"lane status strip"}"#, "lane status strip"),
        (r#"{"pattern":"ToolMoment"}"#, "ToolMoment"),
        (r#""a bare string payload""#, "a bare string payload"),
        ("{}", ""),
        ("", ""),
        (r#"{"depth":3}"#, ""),
        ("not json at all\nsecond line", "not json at all"),
    ] {
        assert_eq!(
            argument_preview(arguments),
            expected,
            "payload {arguments} previews wrongly"
        );
    }
}

/// A one-line strip stays one line, however long the command was.
///
/// Both halves matter: a newline would break the strip's layout, and an
/// unbounded command line would push every neighbouring control off it.
#[test]
fn a_preview_is_flattened_onto_one_bounded_line() {
    let mut previews = CallPreviews::default();
    let long = "x".repeat(400);
    let arguments = format!(r#"{{"command":"echo   one\ntwo {long}"}}"#);

    let preview = activity(
        lane(1),
        &tool(VENDOR_ID, call("Bash", &arguments)),
        &mut previews,
    );

    assert_eq!(preview.chars().count(), MAX_CHARS);
    assert!(
        preview.ends_with('\u{2026}'),
        "a truncated line says so: {preview}"
    );
    assert!(
        preview.starts_with("Bash: echo one two "),
        "runs of whitespace collapse to one space: {preview}"
    );
    assert!(!preview.contains('\n'), "the strip is one line: {preview}");
}

/// A preview that fits is left exactly as it reads.
#[test]
fn a_preview_within_the_bound_is_not_elided() {
    let mut previews = CallPreviews::default();
    let arguments = format!(
        r#"{{"command":"{}"}}"#,
        "y".repeat(MAX_CHARS - "Bash: ".len())
    );

    let preview = activity(
        lane(1),
        &tool(VENDOR_ID, call("Bash", &arguments)),
        &mut previews,
    );

    assert_eq!(preview.chars().count(), MAX_CHARS);
    assert!(
        !preview.ends_with('\u{2026}'),
        "a preview at the bound is whole: {preview}"
    );
}
