use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use listen::{Dialect, EventKind, Mode, Observation, Span, StreamOutcome, SurfaceEvent};
use rusqlite::{Connection, params};
use spice::placeholder::PlaceholderReason;
use tape::Tape;
use wire::{Outcome, ReplyPosition, Version};

#[path = "support/staged_app.rs"]
mod staged_app;
#[path = "cli/watch.rs"]
mod watch;

/// The exit a refused request carries, in either form.
const REFUSAL_EXIT: i32 = 2;
/// The exit a landed command's typed environment fault carries.
const FAULT_EXIT: i32 = 1;

const CODEX_PROSE: &[u8] =
    br#"{"type":"item.completed","item":{"id":"item-1","type":"agent_message","text":"working"}}"#;
const CODEX_PROSE_TEXT: &str = "working";

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
    invoke_configured(argv, None)
}

fn invoke_at(argv: &[&str], current_dir: &Path) -> Spoken {
    invoke_configured(argv, Some(current_dir))
}

fn invoke_configured(argv: &[&str], current_dir: Option<&Path>) -> Spoken {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
    command
        .args(argv)
        .env_remove("CODEX_THREAD_ID") // env-policy: allow
        .env_remove("CLAUDE_CODE_SESSION_ID") // env-policy: allow
        .env_remove("SPICE_AGENT_VENDOR") // env-policy: allow
        .env_remove(steer::hook::LANE)
        // Assertions below exercise the CLI's words, not terminal styling.
        // Refuse ambient color-forcing variables so captured output remains
        // stable under developer shells, CI runners, and test harnesses.
        .env_remove("CLICOLOR")
        .env_remove("CLICOLOR_FORCE")
        .env_remove("FORCE_COLOR")
        .env("NO_COLOR", "1");
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

#[cfg(unix)]
fn invoke_isolated_rtk_run(
    working: &Path,
    executable: &Path,
    history: Option<&Path>,
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_spice"));
    command
        .args(["run", "--", "/bin/sh", "-c", "printf native"])
        .current_dir(working)
        .env_remove(steer::hook::LANE)
        .env_remove("CODEX_THREAD_ID") // env-policy: allow
        .env_remove("CLAUDE_CODE_SESSION_ID") // env-policy: allow
        .env_remove("SPICE_AGENT_VENDOR") // env-policy: allow
        .env_remove(spice::rtk::EXECUTABLE_VARIABLE)
        .env_remove(spice::rtk::HISTORY_VARIABLE)
        .env(spice::rtk::EXECUTABLE_VARIABLE, executable);
    if let Some(history) = history {
        command.env(spice::rtk::HISTORY_VARIABLE, history);
    }
    command.output().expect("isolated RTK run launches")
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
    assert_eq!(refusal.reason(), "retro::selector");
    assert_eq!(
        refusal
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::observed),
        Some("one:two:three")
    );
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
    assert!(stdout.contains("-V, --version"), "{stdout}");
}

#[test]
fn top_level_version_flags_are_repository_free_and_byte_identical() {
    let scratch = Scratch::new("top-level-version");
    let before = directory_state(scratch.path());
    let short = invoke_at(&["-V"], scratch.path());
    let long = invoke_at(&["--version"], scratch.path());

    for spoken in [&short, &long] {
        assert_eq!(spoken.code, Some(0));
        let output = std::str::from_utf8(&spoken.out).expect("version is UTF-8");
        let version = output.split_whitespace().nth(1).expect("product version");
        let version = wire::manifest::ProductVersion::parse(version).expect("canonical semver");
        assert!(
            version.as_str().contains('+'),
            "version identifies the build"
        );
        assert!(spoken.err.is_empty());
    }
    assert_eq!(short.out, long.out);
    assert_eq!(directory_state(scratch.path()), before);
    scratch.remove();
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
    assert!(
        text_body
            .product
            .as_str()
            .contains(text_body.binary.as_str()),
        "the public version identifies the same executable as the artifact report"
    );
    assert_eq!(text_body.wire, wire::WireVersion::CURRENT);
    assert_eq!(text_body.journal, journal::SCHEMA_VERSION);
    assert_eq!(text_body.canvas, wire::CanvasVersion::CURRENT);
    assert_eq!(
        text_body.app,
        canvas::AppSource::resolve(repository.path())
            .expect("fixture app source resolves")
            .current_generation()
            .expect("fixture app generation reads")
            .identity()
            .clone()
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
        assert_eq!(refusal.reason(), "journal::schema-refused", "{name}");
        assert!(
            refusal
                .diagnostic()
                .and_then(wire::RefusalDiagnostic::observed)
                .is_some_and(|observed| observed.contains(expected)),
            "{name} did not preserve the schema error: {refusal:?}",
        );
        assert_eq!(
            utf8(text.err),
            format!(
                "spice: {}; {}\n",
                refusal.reason(),
                refusal
                    .diagnostic()
                    .and_then(wire::RefusalDiagnostic::detail)
                    .expect("schema refusal carries detail")
            ),
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
#[allow(
    clippy::disallowed_methods,
    reason = "the integration fixture acquires ambient PATH at its process boundary"
)]
fn installed_shell_boundary_wraps_once_then_restores_native_process_semantics() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new("structural-shell-boundary");
    let hooks = steer::hook::Hooks::new(scratch.path().join("hooks"));
    hooks.install().expect("structural stages install");

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
            "\"${SPICE_RS_HOOKS-unset}\" \"${SPICE_RS_NATIVE_SHELL-unset}\"\n",
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
    let output = Command::new(hooks.stage(steer::hook::STAGES[0]))
        .args(["-c", &complete])
        .env(steer::hook::EXECUTABLE, binary)
        .env(steer::hook::HOOKS, hooks.root())
        .env(steer::hook::NATIVE_SHELL, &native)
        .env("BOUNDARY_ARGUMENTS", &arguments)
        .env_remove(vendor::CODEX.thread_id_environment())
        .env_remove(vendor::CLAUDE.thread_id_environment())
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
    assert!(
        output.stderr.is_empty(),
        "descendant stderr redirection must remain private"
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
fn direct_zsh_and_bash_command_shells_cross_the_rtk_wrapper_once() {
    use std::os::unix::fs::PermissionsExt;

    for (name, shell) in [
        ("zsh", Path::new("/bin/zsh")),
        ("bash", Path::new("/bin/bash")),
    ] {
        if !shell.is_file() {
            continue;
        }
        let scratch = Scratch::new(&format!("direct-{name}-rtk"));
        let hooks = steer::hook::Hooks::new(scratch.path().join("hooks"));
        hooks.install().expect("structural stages install");
        let history = scratch.path().join("rtk/history.db");
        fs::create_dir_all(history.parent().expect("history has a parent"))
            .expect("history parent creates");
        let rtk = scratch.path().join("fixture-rtk");
        fs::write(
            &rtk,
            concat!(
                "#!/bin/sh\n",
                "printf '%s\\n' \"$*\" >> \"$RTK_DB_PATH\"\n",
                "printf '%s\\n' 'printf rewritten'\n",
                "exit 73\n",
            ),
        )
        .expect("RTK fixture writes");
        fs::set_permissions(&rtk, fs::Permissions::from_mode(0o700))
            .expect("RTK fixture is executable");

        let output = Command::new(shell)
            .args(["-c", "/usr/bin/printf native"])
            .env(steer::hook::ZDOTDIR, hooks.root().join("shell"))
            .env(steer::hook::BASH_ENV, hooks.root().join("shell/bash_env"))
            .env(steer::hook::EXECUTABLE, env!("CARGO_BIN_EXE_spice"))
            .env(steer::hook::HOOKS, hooks.root())
            .env(steer::hook::NATIVE_SHELL, shell)
            .env("SHELL", hooks.stage(steer::hook::STAGES[0]))
            .env(spice::rtk::EXECUTABLE_VARIABLE, &rtk)
            .env(spice::rtk::HISTORY_VARIABLE, &history)
            .env_remove(steer::hook::LANE)
            .output()
            .expect("direct command shell runs");

        assert!(output.status.success(), "{name}: {output:?}");
        assert_eq!(output.stdout, b"rewritten", "{name}");
        assert_eq!(
            fs::read_to_string(&history).expect("RTK history reads"),
            "rewrite -- /usr/bin/printf native\n",
            "{name} crossed the RTK selector more than once",
        );
        scratch.remove();
    }
}

#[test]
#[cfg(unix)]
fn run_uses_optional_rtk_and_degrades_to_the_unchanged_native_command() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new("run-rtk");
    let calls = scratch.path().join("rtk-calls");
    let history = scratch.path().join("lane/rtk/history.db");
    let rtk = scratch.path().join("alternate-rtk");
    fs::write(
        &rtk,
        format!(
            concat!(
                "#!/bin/sh\n",
                "if [ \"$1\" = --version ]; then printf '\\377'; exit 0; fi\n",
                "printf '%s\\n%s\\n' \"$*\" \"$RTK_DB_PATH\" > '{}'\n",
                "printf '%s\\n' \"/bin/sh -c 'printf rewritten; exit 23'\"\n",
                "exit 73\n",
            ),
            calls.display()
        ),
    )
    .expect("write RTK fixture");
    fs::set_permissions(&rtk, fs::Permissions::from_mode(0o700))
        .expect("make RTK fixture executable");

    let health = spice::rtk::probe_health_in_environment(
        rtk.as_os_str(),
        scratch.path(),
        &sill::Environment::empty(),
    );
    assert_eq!(health.state(), spice::rtk::HealthState::Active);
    assert_eq!(health.version(), None);
    assert!(health.detail().contains("selector stdout was not UTF-8"));

    let rewritten = invoke_isolated_rtk_run(scratch.path(), &rtk, Some(&history));
    assert_eq!(rewritten.status.code(), Some(23));
    assert_eq!(rewritten.stdout, b"rewritten");
    assert!(rewritten.stderr.is_empty());
    assert_eq!(
        fs::read_to_string(&calls).expect("read RTK call"),
        format!("rewrite -- printf native\n{}\n", history.display())
    );

    let attempts = scratch.path().join("rewrite-attempts");
    let recovering = scratch.path().join("recovering-rtk");
    fs::write(
        &recovering,
        format!(
            concat!(
                "#!/bin/sh\n",
                "attempt=0\n",
                "if [ -f '{}' ]; then attempt=$(cat '{}'); fi\n",
                "attempt=$((attempt + 1))\n",
                "printf '%s\\n' \"$attempt\" > '{}'\n",
                "if [ \"$attempt\" -eq 1 ]; then\n",
                "  printf '%s\\n' \"rtk 'unterminated\"\n",
                "  exit 41\n",
                "fi\n",
                "printf '%s\\n' \"/bin/sh -c 'printf recovered; exit 23'\"\n",
                "exit 97\n",
            ),
            attempts.display(),
            attempts.display(),
            attempts.display(),
        ),
    )
    .expect("write recovering RTK fixture");
    fs::set_permissions(&recovering, fs::Permissions::from_mode(0o700))
        .expect("make recovering RTK fixture executable");

    let degraded = invoke_isolated_rtk_run(scratch.path(), &recovering, None);
    assert!(degraded.status.success());
    assert_eq!(degraded.stdout, b"native");
    assert!(String::from_utf8_lossy(&degraded.stderr).contains("degraded to native"));

    let recovered = invoke_isolated_rtk_run(scratch.path(), &recovering, None);
    assert_eq!(recovered.status.code(), Some(23));
    assert_eq!(recovered.stdout, b"recovered");
    assert!(recovered.stderr.is_empty());
    assert_eq!(fs::read_to_string(&attempts).expect("read attempts"), "2\n");

    let missing = scratch.path().join("missing-rtk");
    let native = invoke_isolated_rtk_run(scratch.path(), &missing, Some(&history));
    assert!(native.status.success());
    assert_eq!(native.stdout, b"native");
    assert!(String::from_utf8_lossy(&native.stderr).contains("RTK rewrite degraded to native"));
    scratch.remove();
}

#[test]
#[cfg(unix)]
fn run_waits_through_exec_and_closes_capture_while_an_unredirected_descendant_lives() {
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
        "sleep 30 & printf '%s' $! > {}; printf child-output; printf child-error >&2",
        descendant.display()
    );
    let started = Instant::now();
    let returned = invoke(&["run", "--", "/bin/sh", "-c", &backgrounded]);
    assert_eq!(returned.code, Some(0));
    assert_eq!(returned.out, b"child-output");
    assert_eq!(returned.err, b"child-error");
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

const RELAY_PAYLOAD_BYTES: usize = 512 * 1024;

#[test]
#[cfg(unix)]
fn run_drains_complete_foreground_output_under_backpressure() {
    let scratch = Scratch::new("run-relay-backpressure");
    let descendant = scratch.path().join("descendant-pid");
    let helper = std::env::current_exe().expect("locate CLI test executable");
    let mut wrapper = Command::new(env!("CARGO_BIN_EXE_spice"));
    wrapper
        .args(["run", "--"])
        .arg(helper)
        .args([
            "--exact",
            "relay_backpressure_child_fixture",
            "--ignored",
            "--nocapture",
        ])
        .current_dir(scratch.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut wrapper = wrapper.spawn().expect("relayed wrapper launches");
    let stdout = slow_capture(
        wrapper
            .stdout
            .take()
            .expect("relayed wrapper stdout is captured"),
    );
    let stderr = slow_capture(
        wrapper
            .stderr
            .take()
            .expect("relayed wrapper stderr is captured"),
    );
    let status = wrapper.wait().expect("relayed wrapper exits");
    let stdout = stdout.join().expect("slow stdout reader joins");
    let stderr = stderr.join().expect("slow stderr reader joins");

    assert!(status.success(), "relayed wrapper failed with {status}");
    assert_eq!(
        stdout.iter().filter(|byte| **byte == 0).count(),
        RELAY_PAYLOAD_BYTES,
        "foreground stdout was truncated behind the throttled capture"
    );
    assert_eq!(
        stderr.iter().filter(|byte| **byte == u8::MAX).count(),
        RELAY_PAYLOAD_BYTES,
        "foreground stderr was truncated behind the throttled capture"
    );

    let pid = fs::read_to_string(&descendant).expect("descendant pid was published");
    let pid = pid.trim();
    assert!(
        Command::new("/bin/kill")
            .args(["-0", pid])
            .status()
            .expect("descendant liveness probe runs")
            .success(),
        "descendant ended before the wrapper returned"
    );
    assert!(
        Command::new("/bin/kill")
            .args(["-TERM", pid])
            .status()
            .expect("descendant termination runs")
            .success(),
        "descendant could not be terminated"
    );
    scratch.remove();
}

#[cfg(unix)]
fn slow_capture(mut stream: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut captured = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = stream.read(&mut buffer).expect("capture remains readable");
            if read == 0 {
                return captured;
            }
            captured.extend_from_slice(&buffer[..read]);
            std::thread::sleep(Duration::from_millis(5));
        }
    })
}

#[test]
#[ignore = "launched as the direct child of the relay backpressure test"]
#[cfg(unix)]
fn relay_backpressure_child_fixture() {
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    socket2::SockRef::from(&stdout)
        .set_send_buffer_size(1024 * 1024)
        .expect("stdout socket buffer grows");
    socket2::SockRef::from(&stderr)
        .set_send_buffer_size(1024 * 1024)
        .expect("stderr socket buffer grows");
    let descendant = Command::new("/bin/sleep")
        .arg("10")
        .spawn()
        .expect("background descendant launches");
    let pid_path = std::env::current_dir()
        .expect("helper working directory resolves")
        .join("descendant-pid");
    fs::write(pid_path, descendant.id().to_string()).expect("descendant pid publishes");
    drop(descendant);

    let stdout = std::thread::spawn(|| {
        std::io::stdout()
            .lock()
            .write_all(&vec![0; RELAY_PAYLOAD_BYTES])
            .expect("foreground stdout writes")
    });
    let stderr = std::thread::spawn(|| {
        std::io::stderr()
            .lock()
            .write_all(&vec![u8::MAX; RELAY_PAYLOAD_BYTES])
            .expect("foreground stderr writes")
    });
    stdout.join().expect("foreground stdout writer joins");
    stderr.join().expect("foreground stderr writer joins");
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
