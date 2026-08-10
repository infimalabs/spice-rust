//! What the steering loop does against a real filesystem.
//!
//! The scan and the reducer are pure and prove themselves inline. Everything
//! here is the part that touches disk: a rendering published whole, a wrapper
//! that reads it and hands the command through anyway, and interception that
//! either matches the build that installed it or says which stage did not.
//!
//! These are the paths a lane's hearing actually runs through, so they are
//! exercised through the same calls the door makes rather than through their
//! parts.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use steer::ack::keys::Key;
use steer::directives::{Ledger, Steering};
use steer::hook::{Hooks, Refusal, STAGES};
use steer::{inbox, run};
use wire::CanonicalUuid;
use wire::body::SteerSent;

/// The lane most of these tests speak to.
const LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116302";

/// A second lane, which must hear none of the first lane's direction.
const OTHER: &str = "018f82d4-6f59-7d06-92e3-bce913116303";

/// The key a rendering is sealed under.
const SEAL: &str = "1kJK6N3R";

/// A lane's rendering carries its own pending direction, sealed, and nobody
/// else's.
///
/// The seal is what lets a lane tell spice's direction from prose that merely
/// looks like it, so a rendering that lost its seal would be indistinguishable
/// from anything else that reached the same file.
#[test]
fn a_rendering_carries_one_lane_sealed() {
    let scratch = Scratch::new("rendering");
    let inbox_root = scratch.path().join("inbox");
    let ledger = ledger();
    let lane = lane(LANE);

    inbox::publish(&inbox_root, lane, &seal(), &ledger.pending(lane)).expect("rendering publishes");

    let rendered = fs::read_to_string(inbox::path(&inbox_root, lane)).expect("rendering readable");
    assert_eq!(
        rendered,
        "\
<1kJK6N3R>
key=1kJK6M0V stage=published expires=2026-08-01T07:07:19.000Z control=drive-drain-queue
DRAIN QUEUE ASAP
key=1kJK7Sqy stage=published expires=2026-08-01T08:07:19.000Z
feed the board
</1kJK6N3R>
"
    );
    scratch.remove();
}

/// A lane owed nothing is rendered empty rather than sealed around nothing, and
/// the wrapper reports that there was nothing to hear.
///
/// An empty seal would read as direction the lane could not make out, which is
/// a different thing from silence.
#[test]
fn nothing_pending_renders_empty_and_the_wrapper_stays_quiet() {
    let scratch = Scratch::new("quiet");
    let inbox_root = scratch.path().join("inbox");
    let lane = lane(OTHER);

    inbox::publish(&inbox_root, lane, &seal(), &[]).expect("empty rendering publishes");

    let rendering = inbox::path(&inbox_root, lane);
    assert_eq!(fs::read_to_string(&rendering).expect("readable"), "");

    let mut heard = Vec::new();
    assert!(!run::emit(&rendering, &mut heard).expect("emit reads"));
    assert!(heard.is_empty());
    scratch.remove();
}

/// The wrapper writes exactly what was rendered and reports that it did.
#[test]
fn the_wrapper_writes_what_was_rendered() {
    let scratch = Scratch::new("emit");
    let inbox_root = scratch.path().join("inbox");
    let ledger = ledger();
    let lane = lane(LANE);

    inbox::publish(&inbox_root, lane, &seal(), &ledger.pending(lane)).expect("rendering publishes");

    let rendering = inbox::path(&inbox_root, lane);
    let mut heard = Vec::new();
    assert!(run::emit(&rendering, &mut heard).expect("emit reads"));
    assert_eq!(heard, fs::read(&rendering).expect("rendering readable"));
    scratch.remove();
}

/// An absent rendering is ordinary, not a failure.
///
/// The rendering is disposable and disposition lives in the journal, so a
/// wrapper that treated absence as an error would refuse to run commands over a
/// file nobody promised would be there.
#[test]
fn an_absent_rendering_is_ordinary() {
    let scratch = Scratch::new("absent");
    let mut heard = Vec::new();

    assert!(
        !run::emit(&scratch.path().join("never-written"), &mut heard)
            .expect("absence is not an error")
    );
    assert!(heard.is_empty());
    scratch.remove();
}

/// The wrapper hands the command through and reports what it did, rather than
/// standing in for it.
#[cfg(unix)]
#[test]
fn the_wrapper_hands_the_command_through() {
    use std::ffi::OsStr;

    let status = run::hand(OsStr::new("/bin/sh"), ["-c", "exit 3"]).expect("command spawns");

    assert_eq!(status.code(), Some(3));
}

/// A rendering is replaced whole, so a second publication leaves no trace of
/// the first — including when the first was longer.
#[test]
fn publishing_replaces_the_rendering_whole() {
    let scratch = Scratch::new("replace");
    let inbox_root = scratch.path().join("inbox");
    let ledger = ledger();
    let lane = lane(LANE);

    inbox::publish(&inbox_root, lane, &seal(), &ledger.pending(lane)).expect("first publishes");
    inbox::publish(&inbox_root, lane, &seal(), &[]).expect("second publishes");

    assert_eq!(
        fs::read_to_string(inbox::path(&inbox_root, lane)).expect("rendering readable"),
        ""
    );
    scratch.remove();
}

/// Installation places every stage with the bytes this build carries, runnable,
/// and installing again over the same build changes nothing.
#[test]
fn installation_places_every_stage_and_repeats_cleanly() {
    let scratch = Scratch::new("install");
    let hooks = Hooks::new(scratch.path().join("hooks"));

    hooks.install().expect("first installation");
    hooks.install().expect("second installation");

    for stage in STAGES {
        let path = hooks.stage(stage);
        assert_eq!(
            fs::read_to_string(&path).expect("stage readable"),
            stage.bytes(),
            "{} carries this build's bytes",
            path.display()
        );
        assert_executable(&path);
    }
    scratch.remove();
}

#[test]
fn the_entry_stage_invokes_the_registered_rust_run_leaf() {
    let entry = STAGES
        .iter()
        .find(|stage| stage.path() == "entry.sh")
        .expect("the entry stage is registered")
        .bytes();

    assert!(entry.contains(
        "exec spice run -- \"${SPICE_HOOKS:?spice hooks directory}/static/stage.sh\" \"$@\""
    ));
    assert!(!entry.contains("spice agent run"));
    assert!(!entry.contains("SPICE_PROOF"));
    assert!(!entry.contains("if ["));
}

/// The inner stage restores the native shell before the command runs and does
/// not leak either packaged-routing variable across the process boundary.
#[cfg(unix)]
#[test]
fn the_inner_stage_execs_the_native_shell_with_native_descendant_routing() {
    use std::process::Command;

    let scratch = Scratch::new("proof");
    let hooks = Hooks::new(scratch.path().join("hooks"));
    hooks.install().expect("installation");

    let output = Command::new(hooks.stage(STAGES[1]))
        .args([
            "-c",
            "printf '<%s><%s><%s>' \"$SHELL\" \"${SPICE_HOOKS-unset}\" \"${SPICE_NATIVE_SHELL-unset}\"",
        ])
        .env(steer::hook::HOOKS, hooks.root())
        .env(steer::hook::NATIVE_SHELL, "/bin/sh")
        .output()
        .expect("inner stage runs");
    assert!(output.status.success());
    assert_eq!(output.stdout, b"</bin/sh><unset><unset>");
    assert_eq!(output.stderr, b"");
    assert_eq!(
        STAGES[1]
            .bytes()
            .matches("exec \"$native_shell\" \"$@\"")
            .count(),
        1
    );
    scratch.remove();
}

/// Every way interception fails to prove itself is named by the stage that
/// failed, because "interception is broken" does not tell an operator which
/// file to look at.
#[test]
fn a_stage_that_is_missing_or_drifted_is_named() {
    let scratch = Scratch::new("stages");
    let hooks = Hooks::new(scratch.path().join("hooks"));

    assert_eq!(
        hooks.verify(),
        Err(Refusal::Absent {
            stage: hooks.stage(STAGES[0])
        })
    );

    hooks.install().expect("installation");
    for stage in STAGES {
        let path = hooks.stage(stage);
        fs::write(&path, "# drifted\n").expect("stage rewritten");
        assert_eq!(
            hooks.verify(),
            Err(Refusal::Divergent {
                stage: path.clone()
            })
        );
        fs::write(&path, stage.bytes()).expect("stage restored");
    }

    hooks.verify().expect("restored stages verify");
    scratch.remove();
}

/// A taken candidate is left intact and the next root starts empty.
#[test]
fn scratch_setup_creates_a_root_instead_of_adopting_one() {
    let owner = Scratch::new("collision-owner");
    let taken = owner.path().join("taken");
    let free = owner.path().join("free");
    fs::create_dir(&taken).expect("plant a taken candidate");
    let marker = taken.join("left-behind");
    fs::write(&marker, "not this fixture's bytes\n").expect("plant stale contents");

    let scratch = Scratch::from_candidates([taken.clone(), free.clone()].into_iter());

    assert_eq!(scratch.path(), free);
    assert!(
        fs::read_dir(scratch.path())
            .expect("created root is readable")
            .next()
            .is_none(),
        "the created root carried contents this fixture did not write"
    );
    assert_eq!(
        fs::read_to_string(&marker).expect("taken root remains readable"),
        "not this fixture's bytes\n",
        "the taken candidate was disturbed"
    );

    scratch.remove();
    owner.remove();
}

/// The ledger both other tests read their pending direction out of.
fn ledger() -> Ledger {
    let mut ledger = Ledger::new();
    for (key, addressee, text, control, expires) in [
        (
            "1kJK6M0V",
            LANE,
            "DRAIN QUEUE ASAP",
            Some("drive-drain-queue"),
            "2026-08-01T07:07:19.000Z",
        ),
        (
            "1kJK7Sqy",
            LANE,
            "feed the board",
            None,
            "2026-08-01T08:07:19.000Z",
        ),
        (
            "1kJK8KcW",
            OTHER,
            "another lane's direction",
            None,
            "2026-08-01T09:07:19.000Z",
        ),
    ] {
        ledger
            .absorb(Steering::Sent(&SteerSent {
                directive: directive(key),
                key: key.to_owned(),
                lane: lane(addressee),
                text: text.to_owned(),
                control: control.map(str::to_owned),
                attachments: Vec::new(),
                expires: expires.parse().expect("instant"),
            }))
            .expect("directive absorbs");
    }
    ledger
}

fn seal() -> Key {
    SEAL.parse().expect("seal")
}

fn lane(text: &str) -> CanonicalUuid {
    text.parse().expect("lane")
}

fn directive(key: &str) -> CanonicalUuid {
    let suffix = key.bytes().fold(0_u64, |value, byte| {
        value.wrapping_mul(131).wrapping_add(u64::from(byte))
    });
    format!("00000000-0000-4000-8000-{:012x}", suffix & 0xffff_ffff_ffff)
        .parse()
        .expect("fixture directive UUID")
}

#[cfg(unix)]
fn assert_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path)
        .expect("stage metadata")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755, "{} is runnable", path.display());
}

#[cfg(not(unix))]
fn assert_executable(_path: &Path) {}

/// Names tried before a scratch root is declared unobtainable.
const CREATE_ATTEMPTS: usize = 128;

/// A directory this test created and proves it removed.
struct Scratch {
    root: PathBuf,
    removed: bool,
}

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

impl Scratch {
    fn new(label: &str) -> Self {
        let directory = std::env::temp_dir();
        let process = std::process::id();
        Self::from_candidates(std::iter::repeat_with(|| {
            let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
            directory.join(format!("spice-steer-{label}-{process}-{sequence}"))
        }))
    }

    /// Own the first candidate this call creates, never one it merely finds.
    fn from_candidates(candidates: impl Iterator<Item = PathBuf>) -> Self {
        for root in candidates.take(CREATE_ATTEMPTS) {
            match fs::create_dir(&root) {
                Ok(()) => {
                    return Self {
                        root,
                        removed: false,
                    };
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("create scratch root at {}: {error}", root.display()),
            }
        }
        panic!("no scratch root was free within {CREATE_ATTEMPTS} attempts");
    }

    fn path(&self) -> &Path {
        &self.root
    }

    /// Remove the owned root and prove teardown completed.
    fn remove(mut self) {
        fs::remove_dir_all(&self.root).unwrap_or_else(|error| {
            panic!("remove scratch root at {}: {error}", self.root.display())
        });
        assert!(
            !self.root.exists(),
            "scratch root outlived removal at {}",
            self.root.display()
        );
        self.removed = true;
    }
}

impl Drop for Scratch {
    /// Best effort during unwinding; explicit teardown is the observable path.
    fn drop(&mut self) {
        if self.removed {
            return;
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
