//! Fixtures that measure the boundary the runner composes.
//!
//! They sit beside `main.rs` rather than inside it because one file holding
//! both the composition and every measurement of it grew past the shape this
//! repository keeps source at. Nothing here restates the runner: every fixture
//! reaches it through `super::`, so what is measured is the shipped
//! composition and not a copy that could drift away from it.

use std::path::{Path, PathBuf};

use super::{Digest, Domain, Pins, Temporary, Tool, allowed_cargo_arguments};

/// A pin naming a path and no bytes worth checking.
///
/// Nothing below validates a pin: what these fixtures exercise composes the
/// profile, which reads paths out of the document and never opens them.
fn pinned(path: &str) -> Tool {
    Tool {
        path: PathBuf::from(path),
        digest: Digest::compute(Domain::GateTool, b""),
    }
}

/// A pin document written here rather than admitted.
///
/// A workspace check-out carries none, and the two tools these fixtures
/// actually run are this machine's for the same reason. Every other path is
/// absent on purpose, and all of them sit outside any workspace parent used
/// below, so a refusal from [`denied_parent`] is one the caller asked for.
fn fixture_pins() -> Pins {
    Pins {
        source_commit: "0".repeat(40),
        vendor_digest: Digest::compute(Domain::ProofNativeClosure, b""),
        sdk_path: PathBuf::from("/nonexistent/sdk"),
        developer_dir: PathBuf::from("/nonexistent/developer"),
        cargo_home: PathBuf::from("/nonexistent/cargo-home"),
        cargo: pinned("/nonexistent/cargo"),
        rustc: pinned("/nonexistent/rustc"),
        cargo_mutants: pinned("/nonexistent/cargo-mutants"),
        git: pinned("/usr/bin/git"),
        sandbox_exec: pinned("/usr/bin/sandbox-exec"),
    }
}

/// The scratch root layout, spelled without creating any of it.
fn fixture_temporary() -> Temporary {
    let root = PathBuf::from("/private/tmp/spice-proof-native-profile-fixture");
    Temporary {
        cargo_home: root.join("cargo-home"),
        cargo_target: root.join("cargo-target"),
        home: root.join("home"),
        mutants: root.join("mutants"),
        temporary: root.join("tmp"),
        root,
    }
}

/// The checkout this test binary was compiled from.
///
/// The repository root, not the crate directory: the sealed run is handed
/// the workspace, and the neighbourhood the profile denies is the one the
/// root sits in. On this repository that is a directory of peer worktrees.
fn repository_workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the proof crate sits inside the repository it is proving")
}

/// The bytes a sealed run of this checkout would be sandboxed with.
#[cfg(target_os = "macos")]
fn shipped_profile() -> String {
    super::sandbox_profile(
        repository_workspace(),
        &fixture_pins(),
        &fixture_temporary(),
    )
    .expect("the sealed profile composes for a workspace check-out")
}

/// A workspace, a checkout beside it and a scratch root, built somewhere
/// else entirely.
///
/// The read proof aims at a real neighbour because reading one costs it
/// nothing. This proof destroys what it aims at, so it cannot: eleven lanes
/// of this repository share the directory holding it, and a fixture that
/// truncated a file there would be corrupting a peer to prove it could not.
/// Putting a checkout there instead, to aim at safely, is no better -- the
/// read proof beside this one discovers its neighbour by looking, and would
/// pick up a directory that vanishes underneath it whenever the two lanes
/// ran at once.
///
/// The scratch root sits inside the denied neighbourhood on purpose. It is
/// the one path a sealed run writes, the rule handing it back is the last in
/// the profile, and a denial that reached it would be exactly the failure a
/// neighbourhood built without one could not see.
///
/// The workspace is a linked worktree of a repository outside the
/// neighbourhood, which is the layout that separates the two write denials.
/// A repository under the parent is unwritable whichever rule is asked, so a
/// fixture built that way cannot tell that the older denial naming the
/// common directory is still doing anything -- measured, dropping that name
/// from it left every test here green.
/// How many neighbourhoods this process has built, so no two share a name.
#[cfg(target_os = "macos")]
static NEIGHBOURHOODS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(target_os = "macos")]
struct Neighbourhood {
    root: PathBuf,
    origin: PathBuf,
    workspace: PathBuf,
    peer_source: PathBuf,
    temporary: Temporary,
}

#[cfg(target_os = "macos")]
impl Neighbourhood {
    /// What the peer's tracked source holds before anything is aimed at it.
    const PEER_SOURCE: &'static str = "source a sealed run was never told about\n";

    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is past the epoch")
            .as_nanos();
        // The names carry the process and a nanosecond both, because every
        // lane runs this and one of them is not enough on its own. The clock
        // is not enough either: it answers in microseconds on this machine, so
        // two of these built at once shared a name and the second `git init`
        // refused an origin the first had already made. The counter separates
        // them within a process the way the process id separates lanes.
        let stem = format!(
            "{}-{nonce}-{}",
            std::process::id(),
            NEIGHBOURHOODS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let temporary_dir = std::env::temp_dir();
        let root = temporary_dir.join(format!("spice-proof-neighbourhood-{stem}"));
        let origin = temporary_dir.join(format!("spice-proof-origin-{stem}"));
        let peer = root.join("peer");
        std::fs::create_dir_all(peer.join("src")).expect("the fixture peer is creatable");
        std::fs::create_dir_all(root.join("scratch")).expect("the fixture scratch is creatable");
        std::fs::create_dir_all(&origin).expect("the fixture repository is creatable");
        let peer_source = peer.join("src/lib.rs");
        std::fs::write(&peer_source, Self::PEER_SOURCE).expect("the peer source is writable");

        // The temporary directory is reached through a symlink on macOS, and
        // the boundary resolves a path before it matches one. A profile
        // naming the unresolved spelling would deny nothing it named.
        let root = std::fs::canonicalize(&root).expect("the fixture root resolves");
        let origin = std::fs::canonicalize(&origin).expect("the fixture repository resolves");
        let peer_source = std::fs::canonicalize(&peer_source).expect("the peer source resolves");
        let workspace = root.join("workspace");

        // The profile asks Git where the workspace keeps its repository, so
        // the fixture workspace has to be one, and a linked worktree is what
        // puts that answer outside the neighbourhood. The identity is passed
        // per-command because a fixture that borrowed the developer's would
        // fail on a machine that has not configured one.
        for arguments in [
            ["init", "--quiet", "--initial-branch", "fixture"].as_slice(),
            &["commit", "--quiet", "--allow-empty", "-m", "fixture"],
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                workspace.to_str().expect("a fixture path is UTF-8"),
            ],
        ] {
            let done = std::process::Command::new("/usr/bin/git")
                .args(["-c", "user.name=proof fixture"])
                .args(["-c", "user.email=proof@fixture.invalid"])
                .args(arguments)
                .current_dir(&origin)
                .status()
                .expect("macOS carries the Git this runner is pinned against");
            assert!(done.success(), "the fixture repository takes {arguments:?}");
        }

        let scratch = root.join("scratch");
        let temporary = Temporary {
            cargo_home: scratch.join("cargo-home"),
            cargo_target: scratch.join("cargo-target"),
            home: scratch.join("home"),
            mutants: scratch.join("mutants"),
            temporary: scratch.join("tmp"),
            root: scratch,
        };
        Self {
            root,
            origin,
            workspace,
            peer_source,
            temporary,
        }
    }

    /// The neighbourhood this workspace sits in, which the profile denies.
    fn parent(&self) -> &Path {
        &self.root
    }

    /// Where this fixture's workspace keeps its repository.
    ///
    /// Asked of the same function the profile asks rather than assumed from
    /// the layout, so a fixture that stopped producing a linked worktree
    /// stops claiming to measure one.
    fn repository(&self) -> PathBuf {
        let common = super::git_common_dir(&self.workspace, Path::new("/usr/bin/git"))
            .expect("the fixture workspace has a repository");
        assert!(
            !common.starts_with(&self.root),
            "the fixture repository is inside the neighbourhood, so the older \
             write denial is not the only thing that could refuse it: {}",
            common.display(),
        );
        common
    }

    fn profile(&self) -> String {
        super::sandbox_profile(&self.workspace, &fixture_pins(), &self.temporary)
            .expect("the sealed profile composes for the fixture neighbourhood")
    }
}

#[cfg(target_os = "macos")]
impl Drop for Neighbourhood {
    fn drop(&mut self) {
        // Best effort: a fixture that panicked has already said why, and a
        // second failure from the cleanup would only bury it.
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_dir_all(&self.origin);
    }
}

/// A profile that denies what the shipped one denies and gives nothing back.
///
/// Cut from the shipped bytes rather than written out, so what it measures
/// is the shipped allow going missing and not a second author's idea of the
/// rest of the profile.
#[cfg(target_os = "macos")]
fn without_the_allow(profile: &str) -> String {
    let mut kept = String::new();
    let mut dropped = 0_usize;
    for line in profile.lines() {
        if line.starts_with("(allow file-read*") {
            dropped += 1;
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    assert_eq!(
        dropped, 1,
        "the profile names its readable paths in one allow, and this cut {dropped}:\n{profile}",
    );
    kept
}

/// The shipped bytes with the neighbourhood denial cut back to reads.
///
/// One token narrower rather than one rule shorter: the neighbour stays
/// unreadable under this, so a write measured against it answers whether
/// `file-write*` in that rule is what holds the file up, and not merely
/// whether the rule is there at all. The spelling is rebuilt from the same
/// path the profile was composed with, so a rule that moved or changed shape
/// fails the count below instead of being silently left in place.
#[cfg(target_os = "macos")]
fn without_the_write_denial(profile: &str, parent: &Path) -> String {
    let spelled = super::sandbox_string(parent).expect("a fixture path is sandbox-spellable");
    let denial = format!("(deny file-read* file-write* (subpath \"{spelled}\"))");
    let reads_only = format!("(deny file-read* (subpath \"{spelled}\"))");
    assert_eq!(
        profile.matches(&denial).count(),
        1,
        "the profile denies the neighbourhood once, and this saw another spelling:\n{profile}",
    );
    profile.replace(&denial, &reads_only)
}

/// The shipped bytes with the read allow moved below every later denial.
///
/// Cut and reinserted rather than composed a second way, so what comes back
/// is the shipped profile in a different order and nothing else: every rule
/// is the same bytes and only the allow has moved. It lands immediately above
/// the write allow, which is the last rule the profile carries, so one cut
/// puts it below the warm Cargo caches and the host dependency roots at once.
#[cfg(target_os = "macos")]
fn with_the_allow_below_the_denials(profile: &str) -> String {
    let allow = profile
        .lines()
        .find(|line| line.starts_with("(allow file-read*"))
        .expect("the profile names its readable paths in one allow")
        .to_owned();
    let mut moved = without_the_allow(profile);
    let last = moved
        .rfind("(allow file-write*")
        .expect("the profile ends by handing back the run's own scratch root");
    moved.insert_str(last, &format!("{allow}\n"));
    moved
}

/// A profile that denies nothing, so a path unreadable under it was already
/// unreachable for a reason no rule here can claim.
#[cfg(target_os = "macos")]
const OPEN: &str = "(version 1)\n(allow default)\n";

/// The attestation claims host dependencies are denied, and until the roots
/// below were named this profile denied none of them. The claim is only
/// worth its bytes if a library under one of them is actually unreachable,
/// so this runs the shipped denial through `sandbox-exec` and looks. The
/// binary here is this machine's rather than the pinned one, because a
/// workspace check-out has no pin document to read a path out of; what is
/// being measured is the dialect's answer, which does not vary by caller.
///
/// Each root is measured twice, because a root that is unreadable for any
/// other reason -- absent, or unreadable to this user -- would refuse under
/// the denial too and prove nothing. The open profile establishes that the
/// denial is what took it away.
///
/// The inventory is written out here rather than read from
/// `HOST_DEPENDENCY_ROOTS`, because a test that asks the subject which
/// roots to check loses the one mutation that matters: dropping a root
/// would drop its own check with it and pass. Measured -- deriving the list
/// from the constant, replacing `/opt/homebrew` with an absent path left
/// all tests green.
#[cfg(target_os = "macos")]
#[test]
fn a_host_dependency_root_is_unreachable_inside_the_boundary() {
    use super::host_dependency_denials;

    const MUST_BE_UNREACHABLE: [&str; 6] = [
        "/Library/Frameworks",
        "/nix",
        "/opt/homebrew",
        "/opt/local",
        "/sw",
        "/usr/local",
    ];
    let denied = format!("{OPEN}{}", host_dependency_denials());

    let present = MUST_BE_UNREACHABLE
        .into_iter()
        .filter(|root| Path::new(root).is_dir())
        .collect::<Vec<_>>();
    assert!(
        !present.is_empty(),
        "no host dependency root exists on this machine, so nothing here could fail",
    );

    let mut libraries = 0_usize;
    for root in present {
        assert!(
            readable(OPEN, root),
            "{root} was already unreachable without the denial, so its refusal proves nothing",
        );
        assert!(
            !readable(&denied, root),
            "{root} stayed readable inside the boundary that attests host-dependencies=denied",
        );
        let Some(library) = first_file_under(Path::new(root)) else {
            continue;
        };
        let library = library.to_str().expect("a discovered path is UTF-8");
        assert!(
            contents_readable(OPEN, library),
            "{library} was already unreadable without the denial",
        );
        assert!(
            !contents_readable(&denied, library),
            "{library} could still be read inside the boundary",
        );
        libraries += 1;
    }
    assert!(
        libraries > 0,
        "every present root was empty, so no host library was measured",
    );

    for kept in ["/usr/lib", "/Library/Developer"] {
        if Path::new(kept).is_dir() {
            assert!(
                readable(&denied, kept),
                "the denial reached past the host dependency roots and took {kept}",
            );
        }
    }
}

/// The profile this runner ships carries the denial proved above.
///
/// That proof composes its own profile out of [`host_dependency_denials`],
/// so what it measures is the dialect, not the runner. [`sandbox_profile`]
/// has one other caller and it is the sealed run, so nothing else can
/// observe the bytes that actually seal a build. Measured -- deleting the
/// denial from the profile left every test green, and the boundary would
/// have denied nothing while still attesting `host-dependencies=denied`.
///
/// Position is asserted as well as presence: the dialect resolves a path by
/// its last matching rule, so a denial spelled above `(allow default)` is in
/// the profile and does nothing.
#[cfg(target_os = "macos")]
#[test]
fn the_shipped_profile_denies_the_host_dependency_roots() {
    use super::host_dependency_denials;

    let profile = shipped_profile();
    let Some(denied) = profile.find(&host_dependency_denials()) else {
        panic!("the profile this runner ships denies no host dependency root:\n{profile}");
    };
    let opened = profile
        .find("(allow default)")
        .expect("the profile opens by allowing what it does not deny");
    assert!(
        opened < denied,
        "the denial is spelled above (allow default), which takes it back:\n{profile}",
    );
}

/// A checkout beside this one cannot be read inside the boundary.
///
/// `(allow default)` left every peer worktree on this machine readable, and
/// a sealed build that read one would be reading source no receipt names.
/// The neighbour is measured twice for the reason the host dependency proof
/// above measures twice: a directory unreadable for some other reason would
/// refuse under the denial too and prove nothing about it. Metadata and
/// bytes are both measured, because listing a directory only asks the
/// boundary about metadata and source is read.
///
/// The neighbour is discovered rather than named, so this asks the machine
/// it runs on which checkout sits beside the workspace instead of asserting
/// a lane's own layout.
#[cfg(target_os = "macos")]
#[test]
fn a_checkout_beside_the_workspace_is_unreachable_inside_the_boundary() {
    let workspace = repository_workspace();
    let profile = shipped_profile();
    let parent = workspace.parent().expect("the workspace sits somewhere");
    let repository = super::git_common_dir(workspace, Path::new("/usr/bin/git"))
        .expect("this check-out has a repository");

    let mut neighbours = std::fs::read_dir(parent)
        .expect("the directory holding the workspace is readable")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path != workspace && path != &repository && path.join(".git").exists())
        .collect::<Vec<_>>();
    neighbours.sort();
    let Some((neighbour, file)) = neighbours
        .iter()
        .find_map(|path| first_file_under(path).map(|file| (path, file)))
    else {
        panic!(
            "no checkout sits beside {}, so nothing here could fail",
            parent.display()
        );
    };
    let neighbour = neighbour.to_str().expect("a discovered path is UTF-8");
    let file = file.to_str().expect("a discovered path is UTF-8");

    assert!(
        readable(OPEN, neighbour),
        "{neighbour} was already unreachable, so its refusal proves nothing",
    );
    assert!(
        contents_readable(OPEN, file),
        "{file} was already unreadable, so its refusal proves nothing",
    );
    assert!(
        !readable(&profile, neighbour),
        "{neighbour} stayed listable inside the boundary this runner seals with",
    );
    assert!(
        !contents_readable(&profile, file),
        "{file} could still be read inside the boundary this runner seals with",
    );
}

/// A checkout beside this one cannot be written inside the boundary either.
///
/// Denying reads left the neighbourhood writable, and that asymmetry was
/// the sharper half: a run that could not see a peer's source could still
/// truncate it, and would still attest a clean boundary afterward. The
/// denial covers both operations now, and this measures the second one the
/// way the first is measured -- against live `sandbox-exec`, and against the
/// same bytes with only the `file-write*` token cut out, so what it shows is
/// that the token is what holds the file up rather than the rule's presence.
///
/// The file's contents are read back either way. A refusal that left the
/// bytes alone and a permission that replaced them are the whole claim, and
/// an exit code alone would report `tee` failing for any reason at all.
///
/// The scratch root is measured in the same breath because it sits inside
/// this neighbourhood. It is the one path a sealed run writes, and the rule
/// naming it is the last in the profile, so this is also the proof that the
/// denial two rules above did not reach it.
#[cfg(target_os = "macos")]
#[test]
fn a_checkout_beside_the_workspace_cannot_be_written_inside_the_boundary() {
    let neighbourhood = Neighbourhood::new();
    let profile = neighbourhood.profile();
    let reads_only = without_the_write_denial(&profile, neighbourhood.parent());
    let peer = neighbourhood
        .peer_source
        .to_str()
        .expect("a fixture path is UTF-8");

    assert!(
        !writable(&profile, peer),
        "the boundary let a checkout beside the workspace be truncated",
    );
    assert_eq!(
        std::fs::read_to_string(&neighbourhood.peer_source).expect("the peer source survives"),
        Neighbourhood::PEER_SOURCE,
        "the boundary refused the write and the bytes changed anyway",
    );

    assert!(
        writable(&reads_only, peer),
        "{peer} was already unwritable, so its refusal proves nothing",
    );
    assert!(
        std::fs::read_to_string(&neighbourhood.peer_source)
            .expect("the peer source is still there")
            .is_empty(),
        "the write was permitted and the bytes survived, so nothing was measured",
    );

    let scratch = neighbourhood.temporary.root.join("written-by-the-run");
    assert!(
        writable(&profile, scratch.to_str().expect("a fixture path is UTF-8")),
        "the neighbourhood denial reached the run's own scratch root",
    );
}

/// The workspace and its repository stay unwritable, neighbourhood or not.
///
/// The sealed run is handed the workspace read-only, which was true two
/// rules above the neighbourhood pair before it existed and has to stay true
/// after: the pair reads those paths back, and reading is all it gives back.
/// A repository beside the workspace is the case worth measuring, and on a
/// lane of this repository that is what the common directory is.
///
/// Each probe names a file that does not exist and is removed before
/// anything is asserted, so the only way this leaves something behind is by
/// failing, and the name carries this process because the common directory
/// is shared by every lane of this repository.
///
/// It is measured twice over. On this check-out both paths sit inside the
/// denied neighbourhood, so the shipped bytes are what a real seal uses but
/// two rules could be refusing. The fixture neighbourhood puts the
/// repository outside itself, where the denial naming it is the only rule
/// that reaches it, so what holds there is attributable.
#[cfg(target_os = "macos")]
#[test]
fn the_workspace_and_its_repository_stay_unwritable_inside_the_boundary() {
    let probe = format!(".spice-boundary-write-probe-{}", std::process::id());
    let check_and_clean = |profile: &str, root: &Path| {
        let path = root.join(&probe);
        let spelled = path.to_str().expect("a repository path is UTF-8");
        let opened = writable(OPEN, spelled);
        let sealed = writable(profile, spelled);
        let _ = std::fs::remove_file(&path);
        assert!(
            opened,
            "{spelled} could not be written with nothing denied, so nothing was measured",
        );
        assert!(
            !sealed,
            "the boundary let the sealed run write {spelled}, which it is handed read-only",
        );
    };

    let workspace = repository_workspace();
    let shipped = shipped_profile();
    let repository = super::git_common_dir(workspace, Path::new("/usr/bin/git"))
        .expect("this check-out has a repository");
    check_and_clean(&shipped, workspace);
    check_and_clean(&shipped, &repository);

    let neighbourhood = Neighbourhood::new();
    let profile = neighbourhood.profile();
    check_and_clean(&profile, &neighbourhood.workspace);
    check_and_clean(&profile, &neighbourhood.repository());
}

/// The workspace and its repository survive the denial that takes the rest.
///
/// A linked worktree keeps the repository beside the workspace rather than
/// inside it, which is what makes the allow load-bearing here rather than
/// decorative: measured on a lane of this repository, the common directory
/// is a sibling of the workspace, and the denial would take it.
///
/// Each path is measured against the shipped profile with its allow cut
/// away, so what is proved is that the allow is holding the path up. A plain
/// clone, whose repository sits inside the workspace, answers the same way
/// for the same reason -- the cut takes the workspace with it.
#[cfg(target_os = "macos")]
#[test]
fn the_workspace_and_its_repository_stay_readable_inside_the_boundary() {
    let workspace = repository_workspace();
    let profile = shipped_profile();
    let cut = without_the_allow(&profile);
    let repository = super::git_common_dir(workspace, Path::new("/usr/bin/git"))
        .expect("this check-out has a repository");

    let manifest = workspace.join("Cargo.toml");
    let head = repository.join("HEAD");
    for path in [workspace, repository.as_path(), &manifest, &head] {
        let path = path.to_str().expect("a repository path is UTF-8");
        assert!(
            readable(&profile, path),
            "the boundary took {path}, which the sealed run is handed",
        );
        assert!(
            !readable(&cut, path),
            "{path} survives without the allow, so nothing here measures it",
        );
    }
    for file in [&manifest, &head] {
        let file = file.to_str().expect("a repository path is UTF-8");
        assert!(
            contents_readable(&profile, file),
            "the boundary allows {file} to be listed but not read",
        );
    }
}

/// A Cargo home inside the workspace keeps its warm caches denied.
///
/// The allow above reads the workspace back whole, and it is composed above
/// the warm Cargo cache denial and the host dependency roots so that nothing
/// below it is weakened. The case that claim is for is a workspace which
/// happens to contain one of the paths those denials name, and it is a
/// reachable one: [`denied_parent`] exempts a pin below the workspace from
/// the refusal on purpose, so a Cargo home there composes rather than being
/// turned away. In exactly that layout the allow matches the warm registry
/// too, and the composition order is the only thing leaving the denial as the
/// last rule that matches it.
///
/// So the order is measured rather than argued. The same bytes with the allow
/// cut out and reinserted below the denials go in front of `sandbox-exec` as
/// well, which is what makes the refusal attributable to position: the
/// neighbourhood denial matches this path too, and would refuse it either
/// way, so a fixture that only asked the shipped bytes would pass for a
/// profile whose ordering had stopped mattering.
///
/// The open profile establishes the crate is readable at all, and a workspace
/// source measured beside it establishes the allow is still reaching the
/// workspace -- without that, a profile that had simply lost its allow would
/// deny the warm cache for the wrong reason and pass.
#[cfg(target_os = "macos")]
#[test]
fn a_cargo_home_inside_the_workspace_keeps_its_warm_caches_denied() {
    let neighbourhood = Neighbourhood::new();
    let cargo_home = neighbourhood.workspace.join(".cargo");
    std::fs::create_dir_all(cargo_home.join("registry/cache"))
        .expect("the fixture warm registry is creatable");
    let warm = cargo_home.join("registry/cache/warm.crate");
    std::fs::write(&warm, "a dependency this build was never handed\n")
        .expect("the fixture warm registry holds a crate");
    let source = neighbourhood.workspace.join("main.rs");
    std::fs::write(&source, "fn main() {}\n").expect("the fixture workspace holds a source");

    let mut pins = fixture_pins();
    pins.cargo_home = cargo_home;
    let profile = super::sandbox_profile(&neighbourhood.workspace, &pins, &neighbourhood.temporary)
        .expect("a Cargo home below the workspace is admitted rather than refused");
    let below = with_the_allow_below_the_denials(&profile);

    let warm = warm.to_str().expect("a fixture path is UTF-8");
    let source = source.to_str().expect("a fixture path is UTF-8");
    assert!(
        contents_readable(OPEN, warm),
        "{warm} was already unreadable with nothing denied, so its refusal proves nothing",
    );
    assert!(
        contents_readable(&profile, source),
        "the allow is not reaching {source} at all, so nothing about what sits below it \
         is being measured here",
    );
    assert!(
        !contents_readable(&profile, warm),
        "the allow reads the workspace back over the denial naming its warm caches, and \
         the sealed build would link what the host had already fetched:\n{profile}",
    );
    assert!(
        contents_readable(&below, warm),
        "the warm cache stays denied with the allow moved below the denials, so the order \
         the profile is composed in is not what holds it away:\n{below}",
    );
}

/// A pin the denial would swallow refuses the run and says which one.
///
/// The refusal is asserted by its words, not by being a refusal. This
/// function has two ways to refuse and a bare `is_err` cannot tell them
/// apart, so a check that named the wrong pin, or named none, would pass.
///
/// Two different pins are put under the parent, because one case cannot
/// show a check that stops after the entry it happens to name. The `rustc`
/// case is the toolchain root: the pin document names no such root, and
/// needs none, because the compiler it does name lives inside it.
#[test]
fn a_pin_beside_the_workspace_refuses_instead_of_being_denied() {
    use super::denied_parent;

    let workspace = Path::new("/fixture/workspace");
    assert_eq!(
        denied_parent(workspace, &fixture_pins()),
        Ok(Path::new("/fixture")),
        "a pin document naming nothing beside the workspace has nothing to refuse",
    );

    let mut sdk = fixture_pins();
    sdk.sdk_path = PathBuf::from("/fixture/sdk");
    let refusal = denied_parent(workspace, &sdk).expect_err("the SDK sits beside the workspace");
    for named in ["pinned SDK", "/fixture/sdk", "/fixture"] {
        assert!(
            refusal.contains(named),
            "the refusal does not name {named}: {refusal}"
        );
    }

    let mut toolchain = fixture_pins();
    toolchain.rustc = pinned("/fixture/toolchain/bin/rustc");
    let refusal =
        denied_parent(workspace, &toolchain).expect_err("the toolchain sits beside it too");
    for named in ["pinned rustc", "/fixture/toolchain/bin/rustc"] {
        assert!(
            refusal.contains(named),
            "the refusal does not name {named}: {refusal}"
        );
    }

    let mut inside = fixture_pins();
    inside.cargo_home = PathBuf::from("/fixture/workspace/.cargo");
    assert!(
        denied_parent(workspace, &inside).is_ok(),
        "a pin below the workspace is allowed back with it and refuses nothing",
    );

    let mut adjacent = fixture_pins();
    adjacent.sdk_path = PathBuf::from("/fixtureXX/sdk");
    assert!(
        denied_parent(workspace, &adjacent).is_ok(),
        "a directory whose name merely starts with the parent's is not under it",
    );

    assert!(
        denied_parent(Path::new("/"), &fixture_pins()).is_err(),
        "a workspace with no parent leaves the boundary nothing to take",
    );
}

/// Whether `sandbox-exec` lets a directory be listed under one profile.
#[cfg(target_os = "macos")]
fn readable(profile: &str, directory: &str) -> bool {
    sandboxed(profile, "/bin/ls", directory)
}

/// Whether `sandbox-exec` lets a file's bytes be read under one profile.
///
/// Listing a directory only asks the boundary about metadata. A library is
/// linked by reading it, so the claim is about this.
#[cfg(target_os = "macos")]
fn contents_readable(profile: &str, file: &str) -> bool {
    sandboxed(profile, "/bin/cat", file)
}

/// Whether `sandbox-exec` lets a file be truncated under one profile.
///
/// `tee` opens its argument for writing and truncates it before it copies
/// anything, so an empty standard input is enough to destroy the file it is
/// aimed at. That is the operation the neighbourhood has to refuse: a sealed
/// run does not have to be able to read a peer's source to destroy it, and
/// until this rule existed it could do exactly that.
#[cfg(target_os = "macos")]
fn writable(profile: &str, file: &str) -> bool {
    sandboxed(profile, "/usr/bin/tee", file)
}

#[cfg(target_os = "macos")]
fn sandboxed(profile: &str, tool: &str, path: &str) -> bool {
    std::process::Command::new("/usr/bin/sandbox-exec")
        .args(["-p", profile, tool, path])
        // Nothing here feeds the tool, and `tee` would otherwise sit on the
        // harness's own standard input waiting to be told to stop.
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("macOS carries sandbox-exec, which this runner is pinned against")
        .success()
}

/// The first regular file at or one level below a directory.
///
/// Enough to put a real file under a denied root in front of the boundary
/// without walking a whole prefix, and `None` rather than a panic when a
/// root turns out to be empty, which the caller counts.
///
/// The type comes from the directory entry, which does not follow symlinks,
/// so a link is skipped rather than followed. That is deliberate: a link
/// under a denied root can point outside it, and reading its target would
/// measure a path the denial never named. `Path::is_file` would follow it.
#[cfg(target_os = "macos")]
fn first_file_under(root: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut directories = Vec::new();
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_file() => return Some(path),
            Ok(kind) if kind.is_dir() => directories.push(path),
            _ => {}
        }
    }
    directories.into_iter().find_map(|directory| {
        std::fs::read_dir(directory)
            .ok()?
            .flatten()
            .find_map(|entry| {
                entry
                    .file_type()
                    .is_ok_and(|kind| kind.is_file())
                    .then(|| entry.path())
            })
    })
}

#[test]
fn command_inventory_is_closed() {
    assert!(allowed_cargo_arguments(&["--version".into()]));
    assert!(allowed_cargo_arguments(&[
        "test".into(),
        "--locked".into(),
        "-p".into(),
        "gate".into(),
        "--all-targets".into(),
    ]));
    assert!(!allowed_cargo_arguments(&["test".into()]));
    assert!(!allowed_cargo_arguments(&["metadata".into()]));
}
