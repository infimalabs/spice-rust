use std::process::Command;

use wire::Outcome;

/// The exit a refused request carries, in either form.
const REFUSAL_EXIT: i32 = 2;

/// One invocation, as the process boundary reports it.
struct Spoken {
    code: Option<i32>,
    out: Vec<u8>,
    err: Vec<u8>,
}

fn invoke(argv: &[&str]) -> Spoken {
    let output = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args(argv)
        .output()
        .expect("spice launches");
    Spoken {
        code: output.status.code(),
        out: output.stdout,
        err: output.stderr,
    }
}

fn utf8(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).expect("spice speaks UTF-8")
}

#[test]
fn root_help_exposes_the_generated_command_families() {
    let spoken = invoke(&["--help"]);

    assert_eq!(spoken.code, Some(0));
    let stdout = utf8(spoken.out);
    for family in ["board", "gate", "journal", "task", "up"] {
        assert!(
            stdout.contains(family),
            "root help omitted {family}: {stdout}"
        );
    }
}

#[test]
fn a_registered_unlanded_leaf_refuses_by_name() {
    let spoken = invoke(&["task", "next"]);

    assert_eq!(spoken.code, Some(REFUSAL_EXIT));
    assert!(spoken.out.is_empty());
    assert_eq!(
        utf8(spoken.err),
        "spice: release::not-landed; finish the board::next battery before Rust release one admits this leaf\n",
    );
}

/// Both forms carry one outcome: the same value, the same stream, the same
/// exit, and nothing of the other form mixed in.
///
/// The two renderings are checked against each other rather than each against
/// a literal, because a literal only proves what someone wrote down twice. The
/// sentence is rebuilt out of the decoded document, so text and JSON agreeing
/// here means they came from one value rather than from two descriptions of it
/// that happen to match today.
#[test]
fn one_outcome_reaches_one_stream_in_either_form() {
    let text = invoke(&["task", "next"]);
    let json = invoke(&["--json", "task", "next"]);

    assert_eq!(text.code, Some(REFUSAL_EXIT));
    assert_eq!(json.code, Some(REFUSAL_EXIT));
    assert!(text.out.is_empty(), "a refusal is not an answer");
    assert!(json.out.is_empty(), "the form does not move the stream");

    let decoded: Outcome<()> = wire::decode(&json.err).expect("the document is the wire DTO");
    let Outcome::Refusal(refusal) = decoded else {
        panic!("an unlanded leaf refuses: {decoded:?}");
    };

    assert_eq!(
        utf8(text.err),
        format!("spice: {}; {}\n", refusal.reason, refusal.remedy),
    );
    assert_eq!(
        json.err.iter().filter(|byte| **byte == b'\n').count(),
        1,
        "the document is one line",
    );
    assert!(
        !utf8(json.err).contains("spice: "),
        "the sentence must not accompany the document",
    );
}

/// The renderer flag is read wherever spice owns the word, and reads the same
/// everywhere.
///
/// Placement is a parser detail with a silent failure: a position spice does
/// not recognise renders a sentence to a caller who asked for a document, and
/// that caller finds out by failing to parse what arrived.
#[test]
fn the_renderer_flag_is_read_wherever_it_is_written() {
    let expected = invoke(&["--json", "task", "next"]);
    assert!(expected.err.starts_with(b"{"), "the leading form is JSON");

    for argv in [
        &["task", "--json", "next"][..],
        &["task", "next", "--json"][..],
    ] {
        let spoken = invoke(argv);
        assert_eq!(spoken.code, expected.code, "{argv:?}");
        assert_eq!(spoken.out, expected.out, "{argv:?}");
        assert_eq!(spoken.err, expected.err, "{argv:?}");
    }
}

/// After the command delimiter the words belong to the child, including a word
/// spice would otherwise answer to.
///
/// `spice run` hands an arbitrary tool an argument list the operator wrote. A
/// renderer that claimed `--json` there would both silently change its own
/// output and hand the child a shorter command than was typed, and the child
/// is as likely as spice to have a flag by that name.
#[test]
fn a_child_commands_flag_is_not_the_renderers() {
    let child = invoke(&["run", "--", "echo", "--json"]);

    assert_eq!(child.code, Some(REFUSAL_EXIT));
    assert!(child.out.is_empty());
    assert_eq!(
        utf8(child.err),
        "spice: release::not-landed; finish the steer::run battery before Rust release one admits this leaf\n",
    );

    let renderer = invoke(&["run", "--json", "--", "echo"]);
    assert_eq!(renderer.code, Some(REFUSAL_EXIT));
    assert!(renderer.out.is_empty());

    let decoded: Outcome<()> = wire::decode(&renderer.err).expect("the document is the wire DTO");
    let Outcome::Refusal(refusal) = decoded else {
        panic!("an unlanded leaf refuses: {decoded:?}");
    };
    assert_eq!(refusal.observed.as_deref(), Some("run"));
}

#[test]
fn a_name_outside_the_registry_is_a_clap_refusal() {
    let spoken = invoke(&["removed-command"]);

    assert_eq!(spoken.code, Some(REFUSAL_EXIT));
    assert!(utf8(spoken.err).contains("unrecognized subcommand 'removed-command'"));
}

#[test]
fn a_required_contract_operand_is_enforced_at_the_process_boundary() {
    let spoken = invoke(&["task", "add"]);

    assert_eq!(spoken.code, Some(REFUSAL_EXIT));
    assert!(spoken.out.is_empty());
    let stderr = utf8(spoken.err);
    assert!(stderr.contains("required arguments were not provided"));
    assert!(stderr.contains("<TITLE>"));
    assert!(!stderr.contains("release::not-landed"));
}

#[test]
fn an_operand_outside_a_leaf_contract_is_rejected_by_clap() {
    let spoken = invoke(&["doctor", "garbage"]);

    assert_eq!(spoken.code, Some(REFUSAL_EXIT));
    assert!(spoken.out.is_empty());
    let stderr = utf8(spoken.err);
    assert!(stderr.contains("unexpected argument 'garbage'"));
    assert!(!stderr.contains("release::not-landed"));
}
