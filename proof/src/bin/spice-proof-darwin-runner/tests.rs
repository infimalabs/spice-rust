//! In-process checks of the boundary the runner composes.
//!
//! They sit beside `main.rs` rather than inside it to keep each file within
//! the shape this repository keeps source at. Every fixture reaches the runner
//! through `super::`, so what is checked is the shipped composition and not a
//! copy that could drift away from it.

use std::path::{Path, PathBuf};

use super::{Digest, Domain, Pins, Temporary, Tool, allowed_cargo_arguments, release_output_path};

#[test]
fn release_output_admits_the_manifest_pointer_and_no_neighbour() {
    assert!(release_output_path("dist/stage/release/manifest.json"));
    assert!(release_output_path("dist/stage/objects/object"));
    assert!(release_output_path(
        "dist/stage/targets/target/payload.json"
    ));
    assert!(!release_output_path("dist/stage/release/notes.md"));
    assert!(!release_output_path(
        "dist/stage/release/manifest.json.extra"
    ));
    assert!(!release_output_path("dist/stage/unexpected"));
}

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
/// A workspace check-out carries none. Every path sits outside any workspace
/// parent used below, so a refusal from [`denied_parent`] is one the caller
/// asked for.
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

/// How many neighbourhoods this process has built, so no two share a name.
#[cfg(target_os = "macos")]
static NEIGHBOURHOODS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A workspace, its repository authority and a scratch root, in the
/// linked-worktree shape the runner composes its profile for.
#[cfg(target_os = "macos")]
struct Neighbourhood {
    root: PathBuf,
    origin: PathBuf,
    workspace: PathBuf,
    temporary: Temporary,
}

#[cfg(target_os = "macos")]
impl Neighbourhood {
    fn new() -> Self {
        let stem = format!(
            "{}-{}",
            std::process::id(),
            NEIGHBOURHOODS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let temporary_dir = std::env::temp_dir();
        let root = temporary_dir.join(format!("spice-proof-neighbourhood-{stem}"));
        let origin = temporary_dir.join(format!("spice-proof-origin-{stem}"));
        std::fs::create_dir_all(root.join("workspace"))
            .expect("the fixture workspace is creatable");
        std::fs::create_dir_all(&origin).expect("the fixture repository is creatable");
        // The temporary directory is reached through a symlink on macOS, and
        // the boundary resolves a path before it matches one.
        let root = std::fs::canonicalize(&root).expect("the fixture root resolves");
        let origin = std::fs::canonicalize(&origin).expect("the fixture repository resolves");
        let workspace = root.join("workspace");
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
            temporary,
        }
    }

    fn profile(&self) -> String {
        super::sandbox_profile_with_common_dir(
            &self.workspace,
            &self.origin,
            &fixture_pins(),
            &self.temporary,
        )
        .expect("the sealed profile composes for the fixture neighbourhood")
    }
}

#[cfg(target_os = "macos")]
impl Drop for Neighbourhood {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_dir_all(&self.origin);
    }
}

/// The profile this runner ships carries the host dependency denial.
///
/// Position is asserted as well as presence: the dialect resolves a path by
/// its last matching rule, so a denial spelled above `(allow default)` is in
/// the profile and does nothing.
#[cfg(target_os = "macos")]
#[test]
fn the_shipped_profile_denies_the_host_dependency_roots() {
    use super::host_dependency_denials;

    let neighbourhood = Neighbourhood::new();
    let profile = neighbourhood.profile();
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
