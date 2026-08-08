use super::*;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use sill::Deadlines;

static NEXT_PROBE_FIXTURE: AtomicU64 = AtomicU64::new(0);

/// The program name every fixture declares, requests, and plants.
const PROGRAM: &str = "probe-tool";

/// The declaration name every fixture files its evidence under.
const NAME: &str = "probe tool";

/// What the copy planted in the rim prints.
const RIM_VERSION: &str = "probe-tool 1.2.3";

/// What the copy planted in the selected tree prints.
///
/// Planting a different program under the same relative entry is the whole
/// point: evidence carrying this version would prove the probe reached the
/// tree's file while the digest described the rim's.
const TREE_VERSION: &str = "probe-tool 9.9.9";

/// One rim directory and one selected tree, each able to hold a stub.
///
/// The two are siblings under a scratch root, which is what lets a single
/// relative search-path entry name a real file in either of them.
struct ProbeFixture {
    root: PathBuf,
    rim: PathBuf,
    tree: PathBuf,
}

impl ProbeFixture {
    fn new(label: &str) -> Self {
        let sequence = NEXT_PROBE_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "gate-probe-{label}-{}-{sequence}",
            std::process::id()
        ));
        if root.exists() {
            fs::remove_dir_all(&root).expect("remove stale probe fixture");
        }
        let rim = root.join("rim");
        let tree = root.join("tree");
        fs::create_dir_all(rim.join("bin")).expect("create the rim fixture directory");
        fs::create_dir_all(tree.join("bin")).expect("create the tree fixture directory");
        Self { root, rim, tree }
    }

    /// Plant one executable shell stub with the given body.
    fn script(&self, path: &Path, body: &str) -> PathBuf {
        fs::write(path, format!("#!/bin/sh\n{body}"))
            .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|error| panic!("make {} executable: {error}", path.display()));
        path.to_path_buf()
    }

    /// Plant one stub that prints `version` on its own line and succeeds.
    fn version_stub(&self, path: &Path, version: &str) -> PathBuf {
        assert!(!version.contains('\''), "a stub version must be shell safe");
        self.script(path, &format!("printf '%s\\n' '{version}'\n"))
    }

    /// Probe the declared program against one captured search path.
    fn evidence(&self, search_path: &str, bounds: &Bounds) -> ToolEvidence {
        self.evidence_with(search_path, bounds, read_program)
    }

    /// Probe through one selected byte reader.
    fn evidence_with(
        &self,
        search_path: &str,
        bounds: &Bounds,
        read_program: ProgramReader,
    ) -> ToolEvidence {
        let source = ToolchainSource {
            root: &self.tree,
            repository: &self.root,
            rim: RimCapture::new(
                self.rim.clone(),
                Some(OsString::from(search_path)),
                Vec::new(),
            )
            .expect("the fixture rim directory is absolute"),
            baselines: None,
        };
        probe_with(&requirement(), &source, bounds, read_program)
    }
}

impl Drop for ProbeFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove probe fixture");
    }
}

/// The one declaration these fixtures probe.
const fn requirement() -> ToolRequirement {
    ToolRequirement {
        name: NAME,
        program: PROGRAM,
        version_argv: &["--version"],
        exact_version: RIM_VERSION,
        role: "focused acquisition fixture",
    }
}

/// The evidence one resolved-but-unusable program is expected to produce.
fn unprobed(resolved: &Path, failure: ProbeFailure) -> ToolEvidence {
    ToolEvidence::Unprobed {
        name: NAME.to_owned(),
        requested_program: PROGRAM.to_owned(),
        resolved_program: resolved.to_string_lossy().into_owned(),
        failure,
    }
}

/// Bounds for the fixtures whose outcome is decided by the probe's own exit.
///
/// Every fixture here but the one that deliberately outlives its bound
/// asserts evidence the probe produces by finishing: a version it printed, a
/// status it exited with, a file that was never launchable at all. A runtime
/// deadline cannot improve any of those answers, only replace them with the
/// timeout refusal, so a loaded lane would report a stub that ran correctly
/// as one that hung. A runtime that cannot elapse removes that race by
/// construction rather than lengthening it, leaving the blocking wait on the
/// probe's exit as the only way these fixtures end.
fn probe_exit_bounds() -> Bounds {
    probe_bounds(Duration::MAX)
}

fn probe_bounds(runtime: Duration) -> Bounds {
    Bounds::new(
        "toolchain probe test",
        Deadlines::new(runtime, Duration::from_secs(5), Duration::from_secs(5))
            .expect("valid probe deadlines"),
        Duration::from_millis(50),
        16 * 1024,
    )
    .expect("valid probe bounds")
}

/// A relative entry names one file rather than one file per reader.
///
/// Both the rim and the selected tree hold `bin/probe-tool`, and they are
/// different programs. Reading and hashing happen in whatever directory
/// this process was started in, while the probe runs in the selected tree,
/// so an unanchored entry has no single meaning: the digest would describe
/// one file and the version another, or neither would be found at all.
/// Anchoring to the captured rim is what makes path, digest, and version
/// one identity.
#[test]
fn a_relative_search_path_entry_resolves_one_identity_at_the_rim() {
    let fixture = ProbeFixture::new("relative");
    let planted = fixture.version_stub(&fixture.rim.join("bin").join(PROGRAM), RIM_VERSION);
    fixture.version_stub(&fixture.tree.join("bin").join(PROGRAM), TREE_VERSION);

    let evidence = fixture.evidence("bin", &probe_exit_bounds());

    let ToolEvidence::Identified {
        resolved_program,
        version,
        digest,
        ..
    } = &evidence
    else {
        panic!("a relative entry the rim can resolve produced {evidence:?}");
    };
    assert_eq!(
        Path::new(resolved_program),
        planted,
        "the recorded path is not the file the rim's entry names"
    );
    assert_eq!(
        version,
        &format!("{RIM_VERSION}\n"),
        "the probe answered from the selected tree and not from the rim"
    );
    let bytes = fs::read(&planted).expect("read the planted rim stub");
    assert_eq!(
        digest,
        Digest::compute(Domain::GateTool, &bytes).as_str(),
        "the digest describes a file the probe never ran"
    );
}

/// An empty entry is the rim's own directory, never the selected tree's.
#[test]
fn an_empty_search_path_entry_resolves_at_the_rim_and_not_the_tree() {
    let fixture = ProbeFixture::new("empty-entry");
    let planted = fixture.version_stub(&fixture.rim.join(PROGRAM), RIM_VERSION);
    fixture.version_stub(&fixture.tree.join(PROGRAM), TREE_VERSION);

    let evidence = fixture.evidence("", &probe_exit_bounds());

    let ToolEvidence::Identified {
        resolved_program,
        version,
        ..
    } = &evidence
    else {
        panic!("an empty entry the rim can resolve produced {evidence:?}");
    };
    assert_eq!(
        Path::new(resolved_program),
        planted,
        "an empty entry resolved somewhere other than the captured rim"
    );
    assert_eq!(
        version,
        &format!("{RIM_VERSION}\n"),
        "the probe answered from the selected tree and not from the rim"
    );
}

/// The anchor those entries are joined onto is absolute or it is nothing.
///
/// Anchoring turns a relative entry into one absolute file only when the
/// rim itself is absolute, which is what the two tests above rely on. A
/// relative rim would join a relative candidate that this process reads and
/// hashes against the directory it was started in while the probe runs it
/// against the selected tree, so the digest and the version would be free to
/// describe two different programs. That reading is refused where it is
/// written down, so no capture ever carries it as far as `resolve`.
#[test]
fn a_relative_rim_directory_never_becomes_a_capture() {
    let relative = Path::new("rim");

    assert_eq!(
        RimCapture::new(relative, Some(OsString::from("bin")), Vec::new()).unwrap_err(),
        RimDirectoryError(relative.to_path_buf()),
        "a relative directory became a capture whose program has two readings"
    );
}

/// A program no entry holds is still absent, which is a different fact.
///
/// This is what keeps the unusable observations honest: if everything that
/// failed said "resolved", the one state that means "install this" would
/// have no way left to be said.
#[test]
fn a_program_no_entry_holds_is_still_missing() {
    let fixture = ProbeFixture::new("absent");

    let evidence = fixture.evidence("bin", &probe_exit_bounds());

    assert_eq!(
        evidence,
        ToolEvidence::Missing {
            name: NAME.to_owned(),
            requested_program: PROGRAM.to_owned(),
        },
        "a program that was never found earned a row naming a file"
    );
}

/// A file that shadows a program name but has no executable mode cannot be
/// launched, and says so before an emulator can claim its bytes.
#[test]
fn a_resolved_file_that_cannot_be_launched_is_unprobed_rather_than_missing() {
    let fixture = ProbeFixture::new("unlaunchable");
    let planted = fixture.rim.join(PROGRAM);
    fs::write(&planted, "not a program\n").expect("plant a file that is not executable");

    let evidence = fixture.evidence("", &probe_exit_bounds());

    assert_eq!(
        evidence,
        unprobed(&planted, ProbeFailure::NotStarted),
        "a file that shadows the declared program was reported as absent"
    );
}

/// An executable whose bytes cannot be read has no digest, and says so.
#[test]
fn a_resolved_program_that_cannot_be_read_is_unprobed_rather_than_missing() {
    let fixture = ProbeFixture::new("unreadable");
    let planted = fixture.version_stub(&fixture.rim.join(PROGRAM), RIM_VERSION);

    let evidence = fixture.evidence_with("", &probe_exit_bounds(), |_| {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "injected executable read refusal",
        ))
    });

    assert_eq!(
        evidence,
        unprobed(&planted, ProbeFailure::Unreadable),
        "an installed program this machine cannot hash was reported as absent"
    );
}

/// A probe that refuses names the status it refused with.
#[test]
fn a_resolved_program_that_exits_nonzero_is_unprobed_rather_than_missing() {
    let fixture = ProbeFixture::new("nonzero");
    let planted = fixture.script(&fixture.rim.join(PROGRAM), "exit 3\n");

    let evidence = fixture.evidence("", &probe_exit_bounds());

    assert_eq!(
        evidence,
        unprobed(&planted, ProbeFailure::Refused { code: 3 }),
        "an installed program that refused its probe was reported as absent"
    );
}

/// A probe that never finishes is bounded, and the bound is the evidence.
#[test]
fn a_probe_that_outlives_its_bound_is_unprobed_rather_than_missing() {
    let fixture = ProbeFixture::new("timeout");
    let planted = fixture.script(&fixture.rim.join(PROGRAM), "while :; do :; done\n");

    let evidence = fixture.evidence("", &probe_bounds(Duration::from_millis(150)));

    assert_eq!(
        evidence,
        unprobed(&planted, ProbeFailure::TimedOut),
        "an installed program that outran its bound was reported as absent"
    );
}

/// A probe killed by a signal exited on nobody's terms, and says so.
#[test]
fn a_probe_ended_by_a_signal_is_unprobed_rather_than_missing() {
    let fixture = ProbeFixture::new("signaled");
    let planted = fixture.script(&fixture.rim.join(PROGRAM), "kill -TERM $$\n");

    let evidence = fixture.evidence("", &probe_exit_bounds());

    assert_eq!(
        evidence,
        unprobed(&planted, ProbeFailure::Signaled),
        "an installed program killed mid-probe was reported as absent"
    );
}

/// A probe that prints no version identifies nothing, empty string or not.
#[test]
fn a_probe_printing_no_version_is_unprobed_rather_than_identified() {
    let fixture = ProbeFixture::new("no-version");
    let planted = fixture.script(&fixture.rim.join(PROGRAM), "printf '\\n'\n");

    let evidence = fixture.evidence("", &probe_exit_bounds());

    assert_eq!(
        evidence,
        unprobed(&planted, ProbeFailure::Incomplete),
        "a blank line was accepted as a version an exact contract could hold"
    );
}

/// Output that is not text is not decoded into an identity that lies.
///
/// Replacing the undecodable bytes would produce a version string no
/// contract can ever match and no operator can ever find, which reads as
/// drift rather than as the broken tool it actually is.
#[test]
fn a_probe_printing_bytes_that_are_not_text_is_unprobed_rather_than_identified() {
    let fixture = ProbeFixture::new("undecodable");
    let planted = fixture.script(&fixture.rim.join(PROGRAM), "printf 'probe-tool \\377\\n'\n");

    let evidence = fixture.evidence("", &probe_exit_bounds());

    assert_eq!(
        evidence,
        unprobed(&planted, ProbeFailure::Undecodable),
        "bytes that are not text were decoded into an identity"
    );
}

/// The record keeps the terminator; only the comparison removes one.
#[test]
fn identified_evidence_retains_the_line_ending_the_probe_printed() {
    let fixture = ProbeFixture::new("terminator");
    fixture.script(
        &fixture.rim.join(PROGRAM),
        &format!("printf '%s\\r\\n' '{RIM_VERSION}'\n"),
    );

    let evidence = fixture.evidence("", &probe_exit_bounds());

    let ToolEvidence::Identified { version, .. } = &evidence else {
        panic!("a stub that prints its version produced {evidence:?}");
    };
    assert_eq!(
        version,
        &format!("{RIM_VERSION}\r\n"),
        "the record lost bytes the probe printed"
    );
    assert_eq!(
        contracted_version(version),
        RIM_VERSION,
        "the exact-version contract was compared against a line terminator"
    );
}

/// Exactly one terminator is removed, so a blank line is still drift.
#[test]
fn a_trailing_blank_line_survives_the_one_terminator_a_contract_removes() {
    let fixture = ProbeFixture::new("blank-line");
    fixture.script(
        &fixture.rim.join(PROGRAM),
        &format!("printf '%s\\n\\n' '{RIM_VERSION}'\n"),
    );

    let evidence = fixture.evidence("", &probe_exit_bounds());

    let ToolEvidence::Identified { version, .. } = &evidence else {
        panic!("a stub that prints its version produced {evidence:?}");
    };
    assert_eq!(
        version,
        &format!("{RIM_VERSION}\n\n"),
        "the record lost bytes the probe printed"
    );
    assert_eq!(
        contracted_version(version),
        format!("{RIM_VERSION}\n"),
        "every terminator was removed, so a version with one to spare would match"
    );
}
