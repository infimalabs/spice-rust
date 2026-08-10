use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use listen::{EventKind, Mode, Observation, StreamOutcome};
use rusqlite::{Connection, params};
use spice::placeholder::PlaceholderReason;
use tape::Tape;
use wire::{Outcome, ReplyPosition, Version};

#[path = "support/staged_app.rs"]
mod staged_app;

/// The exit a refused request carries, in either form.
const REFUSAL_EXIT: i32 = 2;
/// The exit a landed command's typed environment fault carries.
const FAULT_EXIT: i32 = 1;

const CODEX_PROSE: &[u8] =
    br#"{"type":"item.completed","item":{"id":"item-1","type":"agent_message","text":"working"}}"#;

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-cli-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("scratch root is exclusively created");
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn remove(self) {
        fs::remove_dir_all(&self.0).expect("scratch root is explicitly removed");
        assert!(!self.0.exists(), "scratch root outlived removal");
        std::mem::forget(self);
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn record(bytes: &[u8]) -> Vec<u8> {
    let mut record = bytes.to_vec();
    record.push(b'\n');
    record
}

fn session(root: &Path) -> (PathBuf, Tape) {
    let path = root.join("lane-a/codex");
    let tape = Tape::open(&path).expect("fixture tape opens");
    (path, tape)
}

/// One invocation, as the process boundary reports it.
struct Spoken {
    code: Option<i32>,
    out: Vec<u8>,
    err: Vec<u8>,
}

fn invoke(argv: &[&str]) -> Spoken {
    invoke_with_inbox(argv, None)
}

fn invoke_with_inbox(argv: &[&str], inbox: Option<&OsStr>) -> Spoken {
    invoke_configured(argv, inbox, None)
}

fn invoke_at(argv: &[&str], current_dir: &Path) -> Spoken {
    invoke_configured(argv, None, Some(current_dir))
}

fn invoke_configured(argv: &[&str], inbox: Option<&OsStr>, current_dir: Option<&Path>) -> Spoken {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
    command
        .args(argv)
        .env_remove(steer::hook::INBOX)
        // Assertions below exercise the CLI's words, not terminal styling.
        // Refuse ambient color-forcing variables so captured output remains
        // stable under developer shells, CI runners, and test harnesses.
        .env_remove("CLICOLOR")
        .env_remove("CLICOLOR_FORCE")
        .env_remove("FORCE_COLOR")
        .env("NO_COLOR", "1");
    if let Some(inbox) = inbox {
        command.env(steer::hook::INBOX, inbox);
    }
    if let Some(current_dir) = current_dir {
        command.current_dir(current_dir);
    }
    let output = command.output().expect("spice launches");
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
fn finite_retro_renders_typed_text_json_and_success_exit() {
    let scratch = Scratch::new("transcript-leaves");
    let (path, tape) = session(scratch.path());
    tape.tee(&record(CODEX_PROSE))
        .expect("fixture record is durable");
    let session = path.to_string_lossy().into_owned();

    let text = invoke(&["retro", &session]);
    assert_eq!(text.code, Some(0));
    assert!(text.err.is_empty());
    let text = utf8(text.out);
    assert!(text.contains("event lane=lane-a dialect=codex"));
    assert!(text.contains("kind=prose"));
    assert!(
        text.contains("working"),
        "text renders the typed event payload"
    );
    assert!(!text.contains("\n\n"), "records have no blank separators");

    let json = invoke(&["--json", "retro", &session]);
    assert_eq!(json.code, Some(0));
    assert!(json.err.is_empty());
    assert_eq!(json.out.iter().filter(|byte| **byte == b'\n').count(), 1);
    let replayed: StreamOutcome = wire::decode(&json.out).expect("retro emits its typed DTO");
    assert_eq!(replayed.mode, Mode::Retro);
    assert!(replayed.observations.iter().any(|observation| {
        matches!(
            observation,
            Observation::Event { event, .. } if event.kind() == EventKind::Prose
        )
    }));

    let missing = scratch.path().join("missing/codex");
    let missing = missing.to_string_lossy().into_owned();
    let missing = invoke(&["--json", "retro", &missing]);
    assert_eq!(missing.code, Some(0));
    let accounted: StreamOutcome =
        wire::decode(&missing.out).expect("a missing path remains a typed observation");
    assert!(matches!(
        accounted.observations.as_slice(),
        [Observation::Missing { .. }]
    ));

    scratch.remove();
}

#[test]
fn malformed_locus_is_a_typed_refusal_without_selector_fallthrough() {
    let spoken = invoke(&["--json", "retro", "one:two:three"]);

    assert_eq!(spoken.code, Some(REFUSAL_EXIT));
    assert!(spoken.out.is_empty());
    let decoded: Outcome<()> = wire::decode(&spoken.err).expect("refusal is a wire DTO");
    let Outcome::Refusal(refusal) = decoded else {
        panic!("malformed selector must refuse: {decoded:?}");
    };
    assert_eq!(refusal.reason, "retro::selector");
    assert_eq!(refusal.observed.as_deref(), Some("one:two:three"));
}

#[test]
fn watch_follows_a_real_filesystem_notification_then_reconciles_tape() {
    let scratch = Scratch::new("watch-notification");
    let (_, tape) = session(scratch.path());
    let root = scratch.path().to_string_lossy().into_owned();
    let mut child = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args(["--json", "watch", &root])
        .env_remove(steer::hook::INBOX)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("watch launches");
    let stdout = child.stdout.take().expect("stdout is piped");
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if sender.send(line.expect("watch speaks UTF-8")).is_err() {
                break;
            }
        }
    });

    let first = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("armed watch emits its initial cursor state");
    let initial: StreamOutcome = wire::decode(first.as_bytes()).expect("initial DTO decodes");
    assert!(matches!(
        initial.observations.as_slice(),
        [Observation::Absent { .. }]
    ));

    tape.tee(&record(CODEX_PROSE))
        .expect("a record arrives after the watcher is armed");
    let observed = loop {
        let line = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("filesystem activity wakes the blocking watcher");
        let outcome: StreamOutcome = wire::decode(line.as_bytes()).expect("wake DTO decodes");
        if outcome
            .observations
            .iter()
            .any(|observation| matches!(observation, Observation::Event { .. }))
        {
            break outcome;
        }
    };
    assert_eq!(observed.mode, Mode::Watch);

    child.kill().expect("fixture follower stops");
    let status = child.wait().expect("fixture follower is reaped");
    assert!(
        !status.success(),
        "the endless follower was explicitly stopped"
    );
    drop(receiver);
    reader.join().expect("stdout reader joins");
    scratch.remove();
}

#[cfg(unix)]
#[test]
fn watch_accounts_for_a_selected_symlink_without_following_it() {
    use std::os::unix::fs::symlink;

    let scratch = Scratch::new("watch-symlink");
    let (session, tape) = session(scratch.path());
    tape.tee(&record(CODEX_PROSE))
        .expect("real fixture record is durable");
    let alias = scratch.path().join("alias");
    symlink(&session, &alias).expect("fixture symlink exists");
    let alias = alias.to_string_lossy().into_owned();

    let spoken = invoke(&["--json", "watch", &alias]);
    assert_eq!(spoken.code, Some(0), "a symlink is finite, not followed");
    assert!(spoken.err.is_empty());
    let outcome: StreamOutcome = wire::decode(&spoken.out).expect("absence DTO decodes");
    assert!(matches!(
        outcome.observations.as_slice(),
        [Observation::Absent { .. }]
    ));

    scratch.remove();
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
fn a_registered_landed_leaf_reaches_its_implementation() {
    let repository = RepositoryScratch::new();
    let spoken = invoke_at(&["taste"], repository.path());

    assert_eq!(spoken.code, Some(FAULT_EXIT));
    assert!(spoken.out.is_empty());
    let stderr = utf8(spoken.err);
    assert!(!stderr.contains(PlaceholderReason::NotLanded.as_str()));
    assert!(!stderr.contains(PlaceholderReason::DispatchGap.as_str()));
}

#[test]
fn version_reports_the_running_artifacts_and_finalized_schema_in_both_forms() {
    let repository = RepositoryScratch::finalized();
    let before = directory_state(repository.path());

    let text = invoke_at(&["version"], repository.path());
    let json = invoke_at(&["version", "--json"], repository.path());

    assert_eq!(text.code, Some(0));
    assert_eq!(json.code, Some(0));
    assert!(text.err.is_empty());
    assert!(json.err.is_empty());
    let placeholder = PlaceholderReason::NotLanded.as_str().as_bytes();
    assert!(
        !text
            .out
            .windows(placeholder.len())
            .any(|window| { window == placeholder })
    );

    let text_body: Version = wire::decode(&text.out).expect("text is the stable version body");
    let json_outcome: Outcome<Version> =
        wire::decode(&json.out).expect("JSON is the typed version outcome");
    let Outcome::Reply(json_reply) = json_outcome else {
        panic!("version succeeds with a reply: {json_outcome:?}");
    };

    assert_eq!(json_reply.position, ReplyPosition::Direct {});
    assert_eq!(json_reply.body, text_body);
    assert_eq!(
        text_body.binary,
        wire::Digest::compute(
            wire::Domain::DistArtifact,
            &fs::read(env!("CARGO_BIN_EXE_spice")).expect("read launched spice binary"),
        )
    );
    assert_eq!(text_body.wire, wire::WireVersion::CURRENT);
    assert_eq!(text_body.journal, journal::SCHEMA_VERSION);
    assert_eq!(
        text_body.app,
        canvas::AppSource::resolve(repository.path())
            .expect("fixture app source resolves")
            .digest()
            .expect("fixture app shell reads")
    );
    assert_eq!(text.out.iter().filter(|byte| **byte == b'\n').count(), 1);
    assert_eq!(json.out.iter().filter(|byte| **byte == b'\n').count(), 1);
    assert_eq!(directory_state(repository.path()), before);
}

#[test]
fn version_refuses_unfinalized_and_incompatible_schema_without_mutation() {
    for (name, version, state, from, to, expected) in [
        (
            "prepared",
            1,
            "prepared",
            Some(0),
            Some(1),
            "prepared and unfinalized",
        ),
        (
            "forward",
            journal::SCHEMA_VERSION + 1,
            "finalized",
            None,
            None,
            "newer than this build",
        ),
    ] {
        let repository = RepositoryScratch::schema(version, state, from, to);
        let before = directory_state(repository.path());

        let text = invoke_at(&["version"], repository.path());
        let json = invoke_at(&["--json", "version"], repository.path());

        assert_eq!(text.code, Some(REFUSAL_EXIT), "{name}");
        assert_eq!(json.code, Some(REFUSAL_EXIT), "{name}");
        assert!(text.out.is_empty(), "{name}");
        assert!(json.out.is_empty(), "{name}");

        let decoded: Outcome<Version> =
            wire::decode(&json.err).expect("schema refusal is a typed outcome");
        let Outcome::Refusal(refusal) = decoded else {
            panic!("{name} schema refuses: {decoded:?}");
        };
        assert_eq!(refusal.reason, "journal::schema-refused", "{name}");
        assert!(
            refusal
                .observed
                .as_deref()
                .is_some_and(|observed| observed.contains(expected)),
            "{name} did not preserve the schema error: {refusal:?}",
        );
        assert_eq!(
            utf8(text.err),
            format!("spice: {}; {}\n", refusal.reason, refusal.remedy),
            "{name}",
        );
        assert_eq!(directory_state(repository.path()), before, "{name}");
    }
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
    let repository = RepositoryScratch::new();
    let text = invoke_at(&["taste"], repository.path());
    let json = invoke_at(&["--json", "taste"], repository.path());

    assert_eq!(text.code, Some(FAULT_EXIT));
    assert_eq!(json.code, Some(FAULT_EXIT));
    assert!(text.out.is_empty(), "a fault is not an answer");
    assert!(json.out.is_empty(), "the form does not move the stream");

    let decoded: Outcome<()> = wire::decode(&json.err).expect("the document is the wire DTO");
    let Outcome::Fault(fault) = decoded else {
        panic!("a landed leaf reports its environment fault: {decoded:?}");
    };

    assert_eq!(
        utf8(text.err),
        format!("spice: {}; {}\n", fault.reason, fault.detail),
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
#[cfg(unix)]
fn a_child_commands_flag_is_not_the_renderers() {
    let child = invoke(&["run", "--", "/bin/echo", "--json"]);

    assert_eq!(child.code, Some(0));
    assert_eq!(child.out, b"--json\n");
    assert!(child.err.is_empty());

    let renderer = invoke(&["run", "--json", "--", "/bin/echo", "child"]);
    assert_eq!(renderer.code, Some(0));
    assert_eq!(renderer.out, b"child\n");
    assert!(renderer.err.is_empty());
}

#[test]
#[cfg(unix)]
fn run_emits_pending_direction_then_inherits_the_childs_streams_and_exit() {
    let inbox = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/run-inbox.txt");
    let spoken = invoke_with_inbox(
        &[
            "run",
            "--",
            "/bin/sh",
            "-c",
            "printf child-out; printf child-err >&2; exit 7",
        ],
        Some(inbox.as_os_str()),
    );

    assert_eq!(spoken.code, Some(7));
    assert_eq!(spoken.out, b"child-out");
    assert_eq!(
        spoken.err,
        b"<run-test>\npending direction\n</run-test>\nchild-err"
    );
}

#[test]
#[cfg(unix)]
fn absent_empty_and_unreadable_inboxes_never_block_the_child() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let absent = manifest.join("tests/never-written-run-inbox");

    for inbox in [absent.as_path(), Path::new("/dev/null"), manifest] {
        let spoken = invoke_with_inbox(
            &["run", "--", "/bin/sh", "-c", "printf ran"],
            Some(inbox.as_os_str()),
        );

        assert_eq!(spoken.code, Some(0), "{}", inbox.display());
        assert_eq!(spoken.out, b"ran", "{}", inbox.display());
        assert!(spoken.err.is_empty(), "{}", inbox.display());
    }
}

#[test]
#[cfg(unix)]
fn run_preserves_the_childs_argument_vector() {
    let spoken = invoke(&[
        "run",
        "--",
        "/bin/sh",
        "-c",
        "printf '<%s>\\n' \"$@\"",
        "sh",
        "two words",
        "--json",
        "",
    ]);

    assert_eq!(spoken.code, Some(0));
    assert_eq!(spoken.out, b"<two words>\n<--json>\n<>\n");
    assert!(spoken.err.is_empty());
}

#[test]
#[cfg(unix)]
fn installed_shell_boundary_wraps_once_then_restores_native_process_semantics() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new("structural-shell-boundary");
    let hooks = steer::hook::Hooks::new(scratch.path().join("hooks"));
    hooks.install().expect("structural stages install");
    let inbox = scratch.path().join("inbox");
    fs::write(&inbox, b"<steering>\none wrapper\n</steering>\n")
        .expect("steering rendering writes");

    let arguments = scratch.path().join("native-arguments");
    let native = scratch.path().join("native-shell");
    fs::write(
        &native,
        concat!(
            "#!/bin/sh\n",
            "printf '%s\\0' \"$@\" > \"$BOUNDARY_ARGUMENTS\"\n",
            "exec /bin/sh \"$@\"\n",
        ),
    )
    .expect("native-shell fixture writes");
    fs::set_permissions(&native, fs::Permissions::from_mode(0o700))
        .expect("native-shell fixture is executable");

    let sourced = scratch.path().join("sourced.sh");
    fs::write(
        &sourced,
        "printf 'sourced=<%s>:<%s>\\n' \"$(boundary_function)\" \"$-\"\n",
    )
    .expect("sourced fixture writes");
    let executed = scratch.path().join("executed.sh");
    fs::write(
        &executed,
        concat!(
            "#!/bin/sh\n",
            "printf 'executed=<%s>:<%s>:<%s>\\n' \"$SHELL\" ",
            "\"${SPICE_HOOKS-unset}\" \"${SPICE_NATIVE_SHELL-unset}\"\n",
            "printf 'hidden descendant stderr\\n' >&2\n",
        ),
    )
    .expect("executed fixture writes");
    fs::set_permissions(&executed, fs::Permissions::from_mode(0o700))
        .expect("executed fixture is executable");

    let complete = format!(
        "boundary_function() {{ printf shared; }}; set -u; . {}; {} 2>/dev/null",
        sourced.display(),
        executed.display()
    );
    let binary = Path::new(env!("CARGO_BIN_EXE_spice"));
    let mut path = vec![
        binary
            .parent()
            .expect("spice binary has a directory")
            .to_path_buf(),
    ];
    if let Some(ambient) = std::env::var_os("PATH") {
        path.extend(std::env::split_paths(&ambient));
    }
    let output = Command::new(hooks.stage(steer::hook::STAGES[0]))
        .args(["-c", &complete])
        .env(
            "PATH",
            std::env::join_paths(path).expect("fixture PATH joins"),
        )
        .env(steer::hook::HOOKS, hooks.root())
        .env(steer::hook::NATIVE_SHELL, &native)
        .env(steer::hook::INBOX, &inbox)
        .env("BOUNDARY_ARGUMENTS", &arguments)
        .output()
        .expect("outer structural shell runs");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("command output is UTF-8");
    assert!(stdout.contains("sourced=<shared>:<"));
    assert!(
        stdout
            .lines()
            .find(|line| line.starts_with("sourced="))
            .and_then(|line| line.rsplit_once(":<"))
            .is_some_and(|(_, options)| options.contains('u')),
        "sourced script did not share nounset shell option: {stdout:?}"
    );
    assert!(stdout.contains(&format!("executed=<{}>:<unset>:<unset>", native.display())));
    assert_eq!(
        output.stderr, b"<steering>\none wrapper\n</steering>\n",
        "descendant stderr redirection captured or duplicated wrapper steering"
    );
    assert_eq!(
        fs::read(&arguments).expect("native shell captured its arguments"),
        [b"-c\0".as_slice(), complete.as_bytes(), b"\0"].concat(),
        "the complete command string did not cross the wrapper exactly once"
    );
    scratch.remove();
}

#[test]
#[cfg(unix)]
fn steering_follows_the_wrappers_own_stderr_redirection() {
    let scratch = Scratch::new("run-stderr");
    let inbox = scratch.path().join("inbox");
    let redirected = scratch.path().join("wrapper-stderr");
    fs::write(&inbox, b"wrapper steering\n").expect("inbox writes");
    let stderr = fs::File::create(&redirected).expect("redirect target creates");

    let output = Command::new(env!("CARGO_BIN_EXE_spice"))
        .args([
            "run",
            "--",
            "/bin/sh",
            "-c",
            "printf child-output; printf hidden-child-error >&2",
        ])
        .env(steer::hook::INBOX, &inbox)
        .stderr(Stdio::from(stderr))
        .output()
        .expect("wrapper runs with redirected stderr");

    assert!(output.status.success());
    assert_eq!(output.stdout, b"child-output");
    assert!(output.stderr.is_empty(), "stderr was redirected at spawn");
    assert_eq!(
        fs::read(&redirected).expect("redirected stderr reads"),
        b"wrapper steering\nhidden-child-error"
    );
    scratch.remove();
}

#[test]
#[cfg(unix)]
fn run_waits_through_exec_and_returns_while_a_background_descendant_lives() {
    let scratch = Scratch::new("run-child-identity");
    let before = scratch.path().join("before-pid");
    let after = scratch.path().join("after-pid");
    let replacement = format!(
        "printf '%s' $$ > {}; (exit 0) & wait; exec /bin/sh -c 'printf \"%s\" $$ > {}; exit 9'",
        before.display(),
        after.display()
    );
    let replaced = invoke(&["run", "--", "/bin/sh", "-c", &replacement]);
    assert_eq!(replaced.code, Some(9));
    assert_eq!(fs::read(&before).unwrap(), fs::read(&after).unwrap());

    let descendant = scratch.path().join("descendant-pid");
    let backgrounded = format!(
        "sleep 30 </dev/null >/dev/null 2>&1 & printf '%s' $! > {}",
        descendant.display()
    );
    let started = Instant::now();
    let returned = invoke(&["run", "--", "/bin/sh", "-c", &backgrounded]);
    assert_eq!(returned.code, Some(0));
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "wrapper waited for a descendant after its immediate child exited"
    );
    let pid = fs::read_to_string(&descendant).expect("descendant pid was published");
    let pid = pid.trim();
    assert!(
        Command::new("/bin/kill")
            .args(["-0", pid])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("/bin/kill")
            .args(["-TERM", pid])
            .status()
            .unwrap()
            .success()
    );
    scratch.remove();
}

#[test]
#[cfg(unix)]
fn run_maps_a_child_signal_to_the_shell_exit_contract() {
    let spoken = invoke(&["run", "--", "/bin/sh", "-c", "kill -TERM $$"]);

    assert_eq!(spoken.code, Some(128 + 15));
    assert!(spoken.out.is_empty());
    assert!(spoken.err.is_empty());
}

#[test]
fn run_without_a_child_is_a_clap_refusal() {
    let spoken = invoke(&["run", "--"]);

    assert_eq!(spoken.code, Some(REFUSAL_EXIT));
    assert!(spoken.out.is_empty());
    let stderr = utf8(spoken.err);
    assert!(stderr.contains("required arguments were not provided"));
    assert!(stderr.contains("<CMD>"));
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
    assert!(!stderr.contains(PlaceholderReason::NotLanded.as_str()));
}

#[test]
fn an_operand_outside_a_leaf_contract_is_rejected_by_clap() {
    let spoken = invoke(&["doctor", "garbage"]);

    assert_eq!(spoken.code, Some(REFUSAL_EXIT));
    assert!(spoken.out.is_empty());
    let stderr = utf8(spoken.err);
    assert!(stderr.contains("unexpected argument 'garbage'"));
    assert!(!stderr.contains(PlaceholderReason::NotLanded.as_str()));
}

static NEXT_REPOSITORY: AtomicU64 = AtomicU64::new(0);

struct RepositoryScratch {
    path: PathBuf,
}

impl RepositoryScratch {
    fn finalized() -> Self {
        let scratch = Self::new();
        drop(journal::Journal::open(scratch.journal_path()).expect("finalize journal fixture"));
        scratch
    }

    fn schema(version: u32, state: &str, from: Option<u32>, to: Option<u32>) -> Self {
        let scratch = Self::new();
        let connection = Connection::open(scratch.journal_path()).expect("open schema fixture");
        connection
            .execute_batch(concat!(
                "CREATE TABLE journal_schema(singleton INTEGER PRIMARY KEY, ",
                "version INTEGER NOT NULL, state TEXT NOT NULL, ",
                "from_version INTEGER, to_version INTEGER, plan_digest TEXT);",
            ))
            .expect("create schema identity");
        connection
            .execute(
                concat!(
                    "INSERT INTO journal_schema(singleton, version, state, ",
                    "from_version, to_version) VALUES (1, ?1, ?2, ?3, ?4)"
                ),
                params![version, state, from, to],
            )
            .expect("insert schema identity");
        connection
            .pragma_update(None, "user_version", version)
            .expect("set schema pragma");
        drop(connection);
        scratch
    }

    fn new() -> Self {
        for _ in 0..1024 {
            let path = std::env::temp_dir().join(format!(
                "spice-version-process-{}-{}",
                std::process::id(),
                NEXT_REPOSITORY.fetch_add(1, Ordering::Relaxed),
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    fs::create_dir_all(path.join(".git/.spice-rs/data"))
                        .expect("create repository-common Spice state");
                    staged_app::seed(&path);
                    return Self { path };
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create version repository {}: {error}", path.display()),
            }
        }
        panic!("could not create a distinct version repository fixture");
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn journal_path(&self) -> PathBuf {
        self.path.join(".git/.spice-rs/data/spicejournal.sqlite3")
    }
}

impl Drop for RepositoryScratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).expect("remove version repository fixture");
    }
}

#[derive(Debug, PartialEq, Eq)]
struct FileState {
    bytes: Vec<u8>,
    length: u64,
    modified: std::time::SystemTime,
}

fn directory_state(root: &Path) -> BTreeMap<PathBuf, FileState> {
    let mut state = BTreeMap::new();
    collect_directory_state(root, root, &mut state);
    state
}

fn collect_directory_state(
    root: &Path,
    directory: &Path,
    state: &mut BTreeMap<PathBuf, FileState>,
) {
    for entry in fs::read_dir(directory).expect("read repository fixture") {
        let path = entry.expect("read repository entry").path();
        let metadata = fs::metadata(&path).expect("read repository metadata");
        if metadata.is_dir() {
            collect_directory_state(root, &path, state);
        } else {
            let relative = path
                .strip_prefix(root)
                .expect("fixture entry stays under its root")
                .to_path_buf();
            let previous = state.insert(
                relative,
                FileState {
                    bytes: fs::read(&path).expect("read repository file"),
                    length: metadata.len(),
                    modified: metadata.modified().expect("read repository mtime"),
                },
            );
            assert!(previous.is_none(), "fixture paths are unique");
        }
    }
}
