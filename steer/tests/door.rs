//! Steering rendering, command handoff, and hook installation boundaries.

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
#[test]
fn a_rendering_carries_one_lane_sealed() {
    let ledger = ledger();
    let lane = lane(LANE);
    let rendered = inbox::render(&seal(), &ledger.pending(lane));
    assert_eq!(
        rendered,
        format!(
            "\
<1kJK6N3R>
directive={} key=1kJK6M0V stage=published expires=2026-08-01T07:07:19.000Z control=drive-drain-queue
DRAIN QUEUE ASAP
directive={} key=1kJK7Sqy stage=published expires=2026-08-01T08:07:19.000Z
feed the board
</1kJK6N3R>
",
            directive("1kJK6M0V"),
            directive("1kJK7Sqy"),
        )
    );
}

#[test]
fn typed_media_reaches_the_lane_once_with_its_directive_identity() {
    let attachment =
        wire::Media::from_base64(r"operator](capture)\雪.png", "image/png", "iVBORw0KGgo=")
            .expect("fixture attachment admits");
    let canonical_markdown = attachment.markdown().expect("fixture Markdown renders");
    let reference = wire::AgentMediaReference::from_media(directive("1kJK6M0V"), 0, &attachment)
        .expect("fixture reference derives");
    let encoded_reference =
        String::from_utf8(wire::encode(&reference).expect("fixture reference encodes"))
            .expect("reference JSON is UTF-8");
    let cases = [
        "inspect this capture".to_owned(),
        format!("inspect this capture\n\n{canonical_markdown}"),
    ];

    for text in cases {
        let body = SteerSent {
            directive: directive("1kJK6M0V"),
            fanout: None,
            key: "1kJK6M0V".to_owned(),
            lane: lane(LANE),
            text,
            control: None,
            attachments: vec![attachment.clone()],
            expires: "2026-08-01T07:07:19.000Z".parse().expect("instant"),
        };
        let mut ledger = Ledger::new();
        ledger
            .absorb(Steering::Sent(&body))
            .expect("typed attachment enters standing");

        let rendered = inbox::render(&seal(), &ledger.pending(lane(LANE)));
        assert!(
            rendered.contains(&format!("directive={}", body.directive)),
            "lane rendering lost the durable directive identity: {rendered}",
        );
        assert_eq!(
            rendered.matches(&attachment.data_url).count(),
            0,
            "disposable lane prose must not carry authority bytes: {rendered}",
        );
        assert_eq!(rendered.matches(&encoded_reference).count(), 1);
    }
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

    assert!(
        entry.contains("exec \"${SPICE_RS_WRAPPER_SPICE:?rust spice executable}\" run -- \\\n")
    );
    assert!(
        entry.contains("\"${SPICE_RS_HOOKS:?rust spice hooks directory}/static/stage.sh\" \"$@\"")
    );
    assert!(!entry.contains("spice agent run"));
    let removed_proof_name = ["spice", "proof"].join("_").to_ascii_uppercase();
    assert!(
        !entry.contains(&removed_proof_name),
        "entry stage no longer carries proof routing"
    );
    assert!(!entry.contains("if ["));
}

/// The ambient command stage is a dumb exact-path handoff: it neither searches
/// for the Rust binary nor performs repository discovery of its own.
#[cfg(unix)]
#[test]
fn the_spice_shell_wrapper_execs_only_the_typed_appliance() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let scratch = Scratch::new("spice-wrapper");
    let hooks = Hooks::new(scratch.path().join("hooks"));
    hooks.install().expect("installation");
    let captured = scratch.path().join("captured");
    let appliance = scratch.path().join("exact-appliance");
    fs::write(
        &appliance,
        "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$CAPTURED\"\nexit 7\n",
    )
    .expect("write exact appliance");
    fs::set_permissions(&appliance, fs::Permissions::from_mode(0o700))
        .expect("make exact appliance executable");

    let output = Command::new(hooks.stage(STAGES[4]))
        .args(["task", "next", "two words"])
        .env(steer::hook::EXECUTABLE, &appliance)
        .env("CAPTURED", &captured)
        .env("PATH", scratch.path())
        .output()
        .expect("installed wrapper runs");

    assert_eq!(output.status.code(), Some(7));
    assert_eq!(
        fs::read(&captured).expect("exact appliance captured arguments"),
        b"task\0next\0two words\0"
    );
    scratch.remove();
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
            "printf '<%s><%s><%s>' \"$SHELL\" \"${SPICE_RS_HOOKS-unset}\" \"${SPICE_RS_NATIVE_SHELL-unset}\"",
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
                fanout: None,
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
