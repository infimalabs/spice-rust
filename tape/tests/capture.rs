//! Capture fixtures: append, rotation, interrupted sealing, indexed reads, and
//! concurrent reader visibility.
//!
//! Interrupted sealing is exercised by reconstructing the on-disk states a
//! crash leaves behind rather than by mocking the filesystem. A crash between
//! two publications is precisely a directory holding one of them, so the
//! fixture builds that directory and then asks tape to carry on.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use tape::{Index, Locus, Refusal, Seal, Tape};

/// Bytes chosen to contain a NUL, a newline, and invalid UTF-8, so that any
/// layer tempted to treat capture as text is caught by the fixture.
const VERBATIM: &[u8] = b"prose\x00\xff\xfe line\ntool call\r\n{\"partial\": ";

#[test]
fn one_source_identity_survives_reopen_and_member_sealing() {
    let scratch = Scratch::new("source-stable");
    let path = scratch.path().join("owned");
    let tape = Tape::open(&path).expect("tape opens and publishes its source");
    let source = tape.source_id().expect("source identity reads");
    let locus = tape.tee(VERBATIM).expect("bytes are captured");

    tape.seal().expect("the member seals");
    let reopened = Tape::open_existing(&path).expect("observer reopens the tape");

    assert_eq!(reopened.source_id().expect("source reopens"), source);
    assert_eq!(
        reopened.read(locus).expect("locus survives sealing"),
        VERBATIM
    );
    assert_private(&path.join("source"));
    scratch.remove();
}

#[test]
fn replacing_a_tape_at_the_same_path_mints_another_source_identity() {
    let scratch = Scratch::new("source-replaced");
    let path = scratch.path().join("selected");
    let first = Tape::open(&path)
        .expect("first tape opens")
        .source_id()
        .expect("first source reads");

    fs::remove_dir_all(&path).expect("first generation is removed");
    let second = Tape::open(&path)
        .expect("replacement tape opens")
        .source_id()
        .expect("replacement source reads");

    assert_ne!(second, first, "a path is not a source-generation identity");
    scratch.remove();
}

#[test]
fn the_writer_door_is_the_only_identity_less_tape_migration() {
    let scratch = Scratch::new("source-migration");
    let path = scratch.path().join("legacy");
    let tape = Tape::open(&path).expect("fixture tape opens");
    let locus = tape.tee(VERBATIM).expect("legacy bytes are captured");
    fs::remove_file(path.join("source")).expect("fixture removes the new-format identity");

    let observer = Tape::open_existing(&path).expect("read-only door opens the directory");
    assert!(
        observer.source_id().is_err(),
        "the observer door must not silently adopt an identity-less tape"
    );
    assert!(
        !path.join("source").exists(),
        "observing identity-less state performed no migration"
    );

    let adopted = Tape::open(&path).expect("the owning writer performs the forward migration");
    assert!(
        path.join("source").is_file(),
        "migration publishes identity"
    );
    assert_eq!(
        adopted.read(locus).expect("legacy locus remains readable"),
        VERBATIM,
        "adoption changes neither members nor logical loci"
    );
    scratch.remove();
}

#[test]
fn the_observer_door_never_creates_or_follows_a_tape() {
    let scratch = Scratch::new("observer-door");
    let missing = scratch.path().join("missing");
    assert!(Tape::open_existing(&missing).is_err());
    assert!(!missing.exists(), "opening a missing tape is read-only");

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let real = scratch.path().join("real");
        fs::create_dir(&real).expect("real tape directory exists");
        let alias = scratch.path().join("alias");
        symlink(&real, &alias).expect("fixture symlink exists");
        assert!(
            Tape::open_existing(&alias).is_err(),
            "an observer does not cross a selected symlink"
        );
    }

    scratch.remove();
}

#[test]
fn an_append_is_readable_before_anything_interprets_it() {
    let scratch = Scratch::new("append");
    let tape = Tape::open(scratch.path()).expect("tape opens");

    let locus = tape.tee(VERBATIM).expect("bytes are captured");

    assert_eq!(locus.member, 1, "capture begins at member one");
    assert_eq!(locus.offset, 0);
    assert_eq!(locus.length, VERBATIM.len() as u64);
    assert_eq!(tape.read(locus).expect("locus resolves"), VERBATIM);
    scratch.remove();
}

#[test]
fn each_append_names_only_its_own_bytes() {
    let scratch = Scratch::new("ranges");
    let tape = Tape::open(scratch.path()).expect("tape opens");
    let chunks: [&[u8]; 4] = [b"alpha", b"", b"gamma-gamma", b"d"];

    let loci: Vec<Locus> = chunks
        .iter()
        .map(|chunk| tape.tee(chunk).expect("bytes are captured"))
        .collect();

    let mut expected_offset = 0;
    for (locus, chunk) in loci.iter().zip(chunks) {
        assert_eq!(locus.offset, expected_offset, "appends do not overlap");
        assert_eq!(tape.read(*locus).expect("locus resolves"), chunk);
        expected_offset += chunk.len() as u64;
    }
    scratch.remove();
}

#[test]
fn a_locus_survives_the_rotation_of_its_member() {
    let scratch = Scratch::new("rotation");
    let tape = Tape::open(scratch.path()).expect("tape opens");
    let first = tape.tee(VERBATIM).expect("bytes are captured");
    let second = tape.tee(b"after").expect("bytes are captured");

    let sealed = tape.seal().expect("the member seals");

    assert!(
        matches!(sealed, Seal::Sealed { member: 1, .. }),
        "the active member rotated, got {sealed:?}"
    );
    assert_eq!(
        tape.read(first).expect("the locus still resolves"),
        VERBATIM,
        "a locus means the same bytes after its member is sealed"
    );
    assert_eq!(
        tape.read(second).expect("the locus still resolves"),
        b"after"
    );
    scratch.remove();
}

#[test]
fn sealing_publishes_an_indexed_private_gzip_member() {
    let scratch = Scratch::new("sealed");
    let tape = Tape::open(scratch.path()).expect("tape opens");
    tape.tee(VERBATIM).expect("bytes are captured");

    tape.seal().expect("the member seals");

    let member = scratch.path().join("000000000001.gz");
    let bytes = fs::read(&member).expect("the sealed member is published");
    assert_eq!(&bytes[..2], b"\x1f\x8b", "a sealed member is a gzip member");
    assert!(
        !scratch.path().join("000000000001.plain").exists(),
        "plaintext is removed once its bytes are readable elsewhere"
    );

    let index = tape.index().expect("the index parses");
    assert_eq!(index.members().len(), 1);
    assert_eq!(index.members()[0].member, 1);
    assert_eq!(
        index.members()[0].length,
        VERBATIM.len() as u64,
        "the index records plaintext length, not storage length"
    );

    assert_private(&member);
    assert_private(&scratch.path().join("index"));
    scratch.remove();
}

#[test]
fn a_seal_interrupted_before_its_index_republishes_identical_bytes() {
    let scratch = Scratch::new("interrupted-index");
    let tape = Tape::open(scratch.path()).expect("tape opens");
    let locus = tape.tee(VERBATIM).expect("bytes are captured");
    tape.seal().expect("the member seals");
    let published = fs::read(scratch.path().join("000000000001.gz")).expect("member readable");

    // A crash after the member was published and before the index named it:
    // the member is on disk, unreferenced, and the plaintext never went away.
    fs::remove_file(scratch.path().join("index")).expect("index removed");
    fs::write(scratch.path().join("000000000001.plain"), VERBATIM).expect("plaintext restored");

    let resumed = tape.seal().expect("the interrupted seal completes");

    assert!(matches!(resumed, Seal::Sealed { member: 1, .. }));
    assert_eq!(
        fs::read(scratch.path().join("000000000001.gz")).expect("member readable"),
        published,
        "re-sealing the same plaintext republishes byte-identical bytes"
    );
    assert_eq!(tape.read(locus).expect("locus resolves"), VERBATIM);
    scratch.remove();
}

#[test]
fn a_seal_interrupted_before_cleanup_loses_no_bytes() {
    let scratch = Scratch::new("interrupted-cleanup");
    let tape = Tape::open(scratch.path()).expect("tape opens");
    let locus = tape.tee(VERBATIM).expect("bytes are captured");
    tape.seal().expect("the member seals");

    // A crash after the index named the member and before the plaintext was
    // removed: both copies exist at once.
    fs::write(scratch.path().join("000000000001.plain"), VERBATIM).expect("plaintext restored");

    let next = tape.tee(b"later").expect("capture continues");

    assert_eq!(
        next.member, 2,
        "residue of a completed seal does not reopen its member"
    );
    assert_eq!(next.offset, 0, "the new member starts empty");
    assert!(
        !scratch.path().join("000000000001.plain").exists(),
        "the interrupted seal was completed rather than left behind"
    );
    assert_eq!(
        tape.read(locus).expect("the sealed locus resolves"),
        VERBATIM,
        "no captured byte was lost by finishing the seal"
    );
    assert_eq!(tape.read(next).expect("the new locus resolves"), b"later");
    scratch.remove();
}

#[test]
fn an_empty_member_seals_nothing() {
    let scratch = Scratch::new("empty");
    let tape = Tape::open(scratch.path()).expect("tape opens");

    assert_eq!(tape.seal().expect("sealing is attempted"), Seal::Empty);

    tape.tee(b"").expect("an empty append is accepted");
    assert_eq!(
        tape.seal().expect("sealing is attempted"),
        Seal::Empty,
        "a member holding no bytes is not worth a gzip member"
    );
    scratch.remove();
}

#[test]
fn a_locus_past_the_end_of_its_member_is_refused() {
    let scratch = Scratch::new("range");
    let tape = Tape::open(scratch.path()).expect("tape opens");
    let locus = tape.tee(b"eight!!!").expect("bytes are captured");
    let past = Locus::new(locus.member, locus.offset, locus.length + 1);

    let active = tape.read(past).expect_err("an overlong locus is refused");
    assert!(
        matches!(active.refusal(), Some(Refusal::OutOfRange { .. })),
        "got {active}"
    );

    tape.seal().expect("the member seals");
    let sealed = tape.read(past).expect_err("an overlong locus is refused");
    assert!(
        matches!(sealed.refusal(), Some(Refusal::OutOfRange { .. })),
        "the same locus is refused once sealed, got {sealed}"
    );

    let overflowing = Locus::new(locus.member, u64::MAX, 1);
    assert!(
        matches!(
            tape.read(overflowing).expect_err("refused").refusal(),
            Some(Refusal::OutOfRange { .. })
        ),
        "a locus whose end overflows is refused rather than wrapping"
    );
    scratch.remove();
}

#[test]
fn a_locus_naming_an_unissued_member_is_refused() {
    let scratch = Scratch::new("unissued");
    let tape = Tape::open(scratch.path()).expect("tape opens");
    tape.tee(b"present").expect("bytes are captured");

    for member in [2, 9_999] {
        let error = tape
            .read(Locus::new(member, 0, 1))
            .expect_err("an unissued member is refused");
        assert!(
            matches!(error.refusal(), Some(Refusal::UnknownMember { .. })),
            "got {error}"
        );
    }
    scratch.remove();
}

#[test]
fn the_index_round_trips_through_its_byte_stable_form() {
    let scratch = Scratch::new("index-form");
    let tape = Tape::open(scratch.path()).expect("tape opens");
    for chunk in [b"one".as_slice(), b"two", b"three"] {
        tape.tee(chunk).expect("bytes are captured");
        tape.seal().expect("the member seals");
    }

    let text = fs::read_to_string(scratch.path().join("index")).expect("index readable");
    let parsed = Index::parse(&text).expect("the index parses");

    assert_eq!(parsed.render(), text, "the index has one spelling");
    assert_eq!(parsed.members().len(), 3);
    assert_eq!(parsed.next_member(), 4);
    assert_eq!(
        text.lines().next(),
        Some("tape 1"),
        "the index names its own format first"
    );
    scratch.remove();
}

/// The accept half of "parse admits exactly what render emits".
///
/// The refusal fixture below is the other half. Together they pin the index to
/// one spelling: these forms parse and render back to the identical bytes, and
/// every departure from them named below is refused rather than repaired.
#[test]
fn every_rendered_index_is_admitted_and_renders_back_unchanged() {
    for (sealed, text) in [
        (0, "tape 1\n"),
        (1, "tape 1\nmember 000000000001 3\n"),
        (
            3,
            "tape 1\nmember 000000000001 1\nmember 000000000002 65535\nmember 000000000003 12\n",
        ),
    ] {
        let index = Index::parse(text).expect("a rendered index is admitted");
        assert_eq!(
            index.members().len(),
            sealed,
            "{text:?} named other members"
        );
        assert_eq!(index.render(), text, "{text:?} has a second spelling");
        assert_eq!(
            index.next_member(),
            sealed as u64 + 1,
            "{text:?} appends to the wrong member",
        );
    }
}

#[test]
fn an_index_that_is_not_the_current_form_is_refused_by_line() {
    for (law, broken) in [
        ("text naming no format at all", ""),
        ("a format this build does not write", "tape 2\n"),
        ("a carriage return the renderer never emits", "tape 1\r\n"),
        // A terminal newline is what distinguishes a whole index from one
        // truncated mid-write. `str::lines` yields identical segments either
        // way, so accepting both would accept a partial file as current.
        ("a format line with no terminal newline", "tape 1"),
        (
            "a member line with no terminal newline",
            "tape 1\nmember 000000000001 5",
        ),
        ("a misspelled keyword", "tape 1\nmembers 000000000001 5\n"),
        ("a blank line between members", "tape 1\n\n"),
        ("a member number that is not padded", "tape 1\nmember 1 5\n"),
        (
            "a member number padded to another width",
            "tape 1\nmember 0000000000001 5\n",
        ),
        (
            "a member line carrying no length",
            "tape 1\nmember 000000000001\n",
        ),
        (
            "a length that is not a number",
            "tape 1\nmember 000000000001 five\n",
        ),
        // Rendered lengths are plain decimals, so a leading zero is a second
        // spelling of a number that already has one.
        (
            "a length spelled with a leading zero",
            "tape 1\nmember 000000000001 05\n",
        ),
        (
            "text trailing the length",
            "tape 1\nmember 000000000001 5 5\n",
        ),
        // An empty member seals as `Seal::Empty` and is never named, so a
        // sealed member of no bytes is a member that never existed.
        (
            "a sealed member holding no bytes",
            "tape 1\nmember 000000000001 0\n",
        ),
        // Members are numbered from one and sealed one after another, so the
        // run is contiguous and a locus can name every member it lists.
        ("member zero", "tape 1\nmember 000000000000 5\n"),
        (
            "a run starting past member one",
            "tape 1\nmember 000000000002 5\n",
        ),
        (
            "a gap in the run",
            "tape 1\nmember 000000000001 5\nmember 000000000003 5\n",
        ),
        (
            "one member given two lengths",
            "tape 1\nmember 000000000001 5\nmember 000000000001 6\n",
        ),
        (
            "members out of order",
            "tape 1\nmember 000000000002 5\nmember 000000000001 5\n",
        ),
    ] {
        let error = Index::parse(broken).expect_err(law);
        assert!(
            matches!(error.refusal(), Some(Refusal::MalformedIndex { .. })),
            "{law}: {broken:?} produced {error}",
        );
    }
}

#[test]
fn readers_resolve_loci_while_a_sealer_publishes_underneath_them() {
    let scratch = Scratch::new("concurrent");
    let tape = Tape::open(scratch.path()).expect("tape opens");

    let mut loci = Vec::new();
    let mut expected = Vec::new();
    for index in 0..64_u32 {
        let chunk = format!("chunk-{index:04}\n").into_bytes();
        loci.push(tape.tee(&chunk).expect("bytes are captured"));
        expected.push(chunk);
    }

    let barrier = Arc::new(Barrier::new(2));
    let reader = {
        let tape = tape.clone();
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            barrier.wait();
            for _ in 0..64 {
                for (locus, want) in loci.iter().zip(&expected) {
                    let got = tape
                        .read(*locus)
                        .expect("a locus resolves while a sealer runs");
                    assert_eq!(&got, want, "a reader never observes a partial member");
                }
            }
        })
    };

    barrier.wait();
    tape.seal().expect("the member seals beneath the reader");
    reader
        .join()
        .expect("no reader observed a torn or missing member");
    scratch.remove();
}

/// Assert a published file is readable and writable only by its owner.
fn assert_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = fs::metadata(path)
            .expect("published file is readable")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "{} is not owner-only", path.display());
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Names tried before a scratch root is declared unobtainable.
const CREATE_ATTEMPTS: usize = 128;

/// The first candidate this call creates, never one it merely found.
fn create_scratch_root(candidates: impl Iterator<Item = PathBuf>) -> PathBuf {
    for root in candidates.take(CREATE_ATTEMPTS) {
        match fs::create_dir(&root) {
            Ok(()) => return root,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => panic!("create a scratch root at {}: {error}", root.display()),
        }
    }
    panic!("no scratch root was free within {CREATE_ATTEMPTS} attempts");
}

/// A scratch directory explicitly removed when the fixture ends.
struct Scratch {
    root: PathBuf,
    removed: bool,
}

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

impl Scratch {
    fn new(label: &str) -> Self {
        let directory = std::env::temp_dir();
        let process = std::process::id();
        let root = create_scratch_root(std::iter::repeat_with(|| {
            let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
            directory.join(format!("spice-tape-{label}-{process}-{sequence}"))
        }));
        Self {
            root,
            removed: false,
        }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn remove(mut self) {
        fs::remove_dir_all(&self.root).unwrap_or_else(|error| {
            panic!(
                "remove the scratch root at {}: {error}",
                self.root.display()
            )
        });
        assert!(
            !self.root.exists(),
            "the scratch root outlived its removal at {}",
            self.root.display()
        );
        self.removed = true;
    }
}

impl Drop for Scratch {
    /// Attempt cleanup during unwinding without replacing the first panic.
    fn drop(&mut self) {
        if !self.removed {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[test]
fn scratch_setup_skips_a_root_that_an_earlier_run_already_owns() {
    let owner = Scratch::new("collision-owner");
    let taken = owner.path().join("taken");
    let free = owner.path().join("free");
    fs::create_dir(&taken).expect("plant a taken scratch root");
    fs::write(taken.join("left-behind"), b"earlier run").expect("plant stale content");

    let created = create_scratch_root([taken.clone(), free.clone()].into_iter());

    assert_eq!(created, free, "a taken scratch root was adopted");
    assert_eq!(
        fs::read(taken.join("left-behind")).expect("stale content remains readable"),
        b"earlier run",
        "the taken scratch root was disturbed"
    );
    assert_eq!(
        fs::read_dir(&created)
            .expect("the created scratch root is readable")
            .count(),
        0,
        "the created scratch root carried content it did not write"
    );

    owner.remove();
}
