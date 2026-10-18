//! Steering rendering and hook installation boundaries.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use steer::ack::keys::Key;
use steer::directives::{Ledger, Steering};
use steer::hook::{Hooks, Refusal, STAGES};
use steer::inbox;
use wire::CanonicalUuid;
use wire::body::SteerSent;

/// The lane most of these tests speak to.
const LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116302";

/// A second lane, which must hear none of the first lane's direction.
const OTHER: &str = "018f82d4-6f59-7d06-92e3-bce913116303";

/// The key a rendering is sealed under.
const SEAL: &str = "1kJK6N3R";

/// The instant renderings are made at.
fn now() -> wire::Instant {
    "2026-08-01T07:00:00.000Z".parse().expect("fixture now")
}

/// A lane's rendering carries its own pending direction, sealed, and nobody
/// else's.
#[test]
fn a_rendering_carries_one_lane_sealed() {
    let ledger = ledger();
    let lane = lane(LANE);
    let rendered = inbox::render(&seal(), &ledger.pending(lane), now());
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
            from: None,
            room: None,
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

        let rendered = inbox::render(&seal(), &ledger.pending(lane(LANE)), now());
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
                from: None,
                room: None,
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
