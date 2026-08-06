//! The native Rust check, and the build directories it must not share.
//!
//! The canvas arm in [`super`] already compiles Rust, so the invocation and the
//! diagnostic parse are shared with it rather than restated here. What is not
//! shared is the reason this arm exists, the target set that serves it, and the
//! `target` pair it owns; those are the whole of this file.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{
    Analysis, CARGO, CARGO_BUILD_DIRECTORY, CARGO_TARGET_DIRECTORY, COMPILED_HOME_DIRECTORY,
    CompileInvocation, HOME_DIRECTORY, run_compile_analysis, tool_search_path,
};
use crate::measure::Bounds;
use crate::toolchain::{ArmOutcome, ArmTiming, FindingDetail, ToolchainDeclaration};
use sill::Environment;

/// The native analysis, kept in its declaration order.
///
/// `--all-targets` is the whole point of the arm rather than a thoroughness
/// flourish: a plain `cargo check` builds no test target, and test bodies are
/// exactly where a rename's stale references survive a clean textual merge.
pub(super) const CARGO_CHECK_ARGUMENTS: &[&str] =
    &["check", "--locked", "--workspace", "--all-targets"];

/// Compile the selected workspace natively, including every test target.
///
/// The canvas arm beside this one already compiles Rust, but only for
/// `wasm32-unknown-unknown` and only the default targets, so nothing in the
/// battery ever built a `#[cfg(test)]` body. A rename that lands while a peer's
/// branch was cut before it leaves stale *references* behind, never
/// declarations, and references live in bodies: the vocabulary gate cannot see
/// them and the merge is textually clean. The compiler names each one with its
/// file and line, which is what this arm publishes.
pub(in crate::toolchain) fn rust_typecheck(
    declaration: ToolchainDeclaration,
    root: &Path,
    bounds: &Bounds,
    programs: &BTreeMap<&str, &str>,
) -> Analysis {
    if !root.join("Cargo.toml").is_file() {
        return Analysis::not_applicable(declaration);
    }
    let Some(cargo) = programs.get(CARGO) else {
        return Analysis::terminal(
            ArmOutcome::ToolUnavailable,
            format!("{} declares no {CARGO} program", declaration.id.as_str()),
        );
    };
    let workspace = match RustWorkspace::create(root) {
        Ok(workspace) => workspace,
        Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
    };
    let environment = match rust_environment(cargo, &workspace) {
        Ok(environment) => environment,
        Err(reason) => return Analysis::terminal(ArmOutcome::ParseFailure, reason),
    };

    let invocation = CompileInvocation {
        name: "cargo native check",
        program: cargo,
        arguments: CARGO_CHECK_ARGUMENTS,
        cwd: root,
    };
    let result = run_compile_analysis(
        declaration.id,
        root,
        invocation,
        bounds,
        &environment,
        |code, message| FindingDetail::RustTypeOutput { code, message },
    );
    match result {
        Err(failure) => {
            let (analysis, elapsed) = failure.into_parts();
            match elapsed {
                Some(elapsed) => analysis.timed(timing(elapsed, bounds)),
                None => analysis,
            }
        }
        Ok(elapsed) => Analysis::terminal(
            ArmOutcome::Passed,
            format!(
                "{} completed its declared native analysis",
                declaration.id.as_str()
            ),
        )
        .timed(timing(elapsed, bounds)),
    }
}

fn timing(elapsed: Duration, bounds: &Bounds) -> ArmTiming {
    ArmTiming {
        elapsed_ms: milliseconds(elapsed),
        observation_bound_ms: observation_bound_ms(bounds),
    }
}

fn observation_bound_ms(bounds: &Bounds) -> u64 {
    let deadlines = bounds.deadlines();
    milliseconds(
        deadlines
            .runtime()
            .saturating_add(deadlines.output_drain())
            .saturating_add(deadlines.reap())
            .saturating_add(bounds.grace()),
    )
}

fn milliseconds(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Build directories the native check owns, kept off the lane's own `target`.
///
/// Cargo takes an exclusive lock on a build directory. Sharing the lane's
/// `target` would make this arm block on, and be blocked by, any `cargo` the
/// occupant is already running, so the arm gets its own pair exactly as the
/// canvas arm does.
struct RustWorkspace {
    root: PathBuf,
    cargo_target: PathBuf,
    cargo_build: PathBuf,
}

impl RustWorkspace {
    fn create(selected: &Path) -> Result<Self, String> {
        let Some(name) = selected.file_name() else {
            return Err(format!(
                "rust selected tree {selected:?} has no name for an analysis workspace"
            ));
        };
        let mut workspace_name = name.to_os_string();
        workspace_name.push(".spice-rust-analysis");
        let root = selected.with_file_name(workspace_name);
        fs::create_dir(&root).map_err(|error| {
            format!(
                "rust analysis workspace {} could not be created: {error}",
                root.display()
            )
        })?;
        let cargo_target = root.join("cargo-target");
        let cargo_build = root.join("cargo-build");
        if let Err(error) =
            fs::create_dir(&cargo_target).and_then(|()| fs::create_dir(&cargo_build))
        {
            let _ = fs::remove_dir_all(&root);
            return Err(format!(
                "rust analysis output directories for {} could not be created: {error}",
                selected.display()
            ));
        }
        Ok(Self {
            root,
            cargo_target,
            cargo_build,
        })
    }
}

impl Drop for RustWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn rust_environment(cargo: &str, workspace: &RustWorkspace) -> Result<Environment, String> {
    let path = tool_search_path("rust-typecheck", &[cargo, "/usr/bin/cc", "/bin/sh"])?;
    Environment::new([
        ("PATH", path),
        (HOME_DIRECTORY, COMPILED_HOME_DIRECTORY.into()),
        (
            CARGO_TARGET_DIRECTORY,
            workspace.cargo_target.as_os_str().to_owned(),
        ),
        (
            CARGO_BUILD_DIRECTORY,
            workspace.cargo_build.as_os_str().to_owned(),
        ),
    ])
    .map_err(|error| format!("native typecheck tool environment was refused: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::toolchain::{ArmFinding, GateId, declaration};
    use sill::Deadlines;

    static NEXT_RUST_FIXTURE: AtomicU64 = AtomicU64::new(0);

    /// The manifest of a workspace whose only crate is the planted one.
    const FIXTURE_MANIFEST: &str = "\
[workspace]

[package]
name = \"planted\"
version = \"0.0.0\"
edition = \"2024\"

[lib]
path = \"src/lib.rs\"
";

    /// A lock for a crate with no dependencies, written rather than generated.
    ///
    /// `--locked` refuses to create one, so the fixture must ship it. Nothing
    /// is resolved here: a package with no dependency section locks to itself,
    /// which is why this can be a literal and needs no registry.
    const FIXTURE_LOCK: &str = "\
version = 4

[[package]]
name = \"planted\"
version = \"0.0.0\"
";

    /// The planted source: a call, in a test body, to a spelling nothing defines.
    ///
    /// Both halves of the placement are load-bearing. It is a *reference*, not a
    /// declaration, because `gate/tests/retired_names.rs` judges declarations
    /// only: its `scan_item` reads `Item::Fn`'s `sig.ident` and `attrs` and
    /// never descends into `node.block`. Widening that walk is refused from
    /// inside by a law the same suite pins at line 219,
    /// `a_local_binding_is_not_a_coined_name`, which asserts that a `let driver`
    /// binding coins nothing while `driver` is a retired spelling; judging every
    /// path expression turns that law red, and judging only qualified paths
    /// still misses a bare call exactly like this one.
    ///
    /// And it sits in a `#[cfg(test)]` body because that is what `--all-targets`
    /// buys. A plain `cargo check` builds no test target, so the canvas arm
    /// beside this one would compile this same crate and report nothing.
    const PLANTED_REFERENCE: &str = "\
pub fn caller() -> u32 {
    7
}

#[cfg(test)]
mod tests {
    #[test]
    fn planted() {
        let _ = dev_app_at();
    }
}
";

    /// The same source with the plant removed and nothing else changed.
    const REMOVED_PLANT: &str = "\
pub fn caller() -> u32 {
    7
}

#[cfg(test)]
mod tests {
    #[test]
    fn planted() {
        assert_eq!(super::caller(), 7);
    }
}
";

    /// The one-based line the planted call occupies in [`PLANTED_REFERENCE`].
    const PLANTED_LINE: u32 = 9;

    /// The one-based column the planted call's first character occupies.
    const PLANTED_COLUMN: u32 = 17;

    /// A workspace the real compiler is run over.
    struct RustFixture {
        root: PathBuf,
    }

    impl RustFixture {
        fn new(label: &str) -> Self {
            let sequence = NEXT_RUST_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "gate-rust-{label}-{}-{sequence}",
                std::process::id()
            ));
            if root.exists() {
                fs::remove_dir_all(&root).expect("remove stale native fixture");
            }
            fs::create_dir_all(root.join("src")).expect("create native fixture tree");
            fs::write(root.join("Cargo.toml"), FIXTURE_MANIFEST).expect("write fixture manifest");
            fs::write(root.join("Cargo.lock"), FIXTURE_LOCK).expect("write fixture lock");
            Self { root }
        }

        fn source(&self, contents: &str) {
            fs::write(self.root.join("src/lib.rs"), contents).expect("write fixture source");
        }

        /// Analyze the fixture with the compiler that built these tests.
        ///
        /// `CARGO` names the exact binary Cargo invoked this build with, which
        /// is the same resolution `gate/src/input/declaration.rs` already makes
        /// for the mutation battery. Nothing is stubbed: a stub echoing a canned
        /// diagnostic would prove the parser and never that the compiler finds a
        /// reference the vocabulary gate cannot see.
        fn analyze(&self) -> Analysis {
            let cargo = env!("CARGO");
            let programs = BTreeMap::from([(CARGO, cargo)]);
            rust_typecheck(
                declaration(GateId::RustTypecheck).expect("native arm is declared"),
                &self.root,
                &rust_bounds(),
                &programs,
            )
        }
    }

    impl Drop for RustFixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).expect("remove native fixture");
        }
    }

    /// Bounds whose runtime cannot elapse, so only the compiler ends the run.
    ///
    /// This fixture asserts nothing about the deadline. A budget that can expire
    /// would only give it a second way to finish, reporting `Timeout` instead of
    /// what the compiler said whenever the lane is loaded enough for a real
    /// build to outrun the number.
    fn rust_bounds() -> Bounds {
        Bounds::new(
            "native analyzer test",
            Deadlines::new(
                Duration::MAX,
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .expect("valid native deadlines"),
            Duration::from_millis(25),
            64 * 1024,
        )
        .expect("valid native bounds")
    }

    /// The declared analysis and the compiled one are the same argument vector.
    ///
    /// `--all-targets` is the only reason this arm reaches a body the canvas arm
    /// does not, so a deletion here has to fail somewhere that names it.
    #[test]
    fn native_argument_vector_is_exactly_the_declared_analysis() {
        let declared = declaration(GateId::RustTypecheck)
            .expect("native arm is declared")
            .analysis;

        assert_eq!(
            [format!("{CARGO} {}", CARGO_CHECK_ARGUMENTS.join(" "))],
            declared
        );
    }

    /// The arm refuses a planted body reference, then passes once it is gone.
    ///
    /// This is the non-vacuity proof: the same fixture, the same compiler and
    /// the same arm answer differently for the one line that differs between
    /// [`PLANTED_REFERENCE`] and [`REMOVED_PLANT`], and the refusal carries the
    /// file and the line rather than an opaque exit code.
    #[test]
    fn native_check_names_a_planted_body_reference_and_passes_once_it_is_removed() {
        let fixture = RustFixture::new("planted");

        fixture.source(PLANTED_REFERENCE);
        let refusal = fixture.analyze();

        assert_eq!(refusal.outcome, ArmOutcome::Findings);
        let refusal_timing = refusal
            .timing
            .expect("the compiler refusal reports wall time");
        assert!(refusal_timing.elapsed_ms <= refusal_timing.observation_bound_ms);
        let [finding] = refusal.findings.as_slice() else {
            panic!(
                "the planted reference is one located diagnostic, not {}",
                refusal.findings.len()
            );
        };
        assert_eq!(finding.path.as_deref(), Some("src/lib.rs"));
        assert_eq!(finding.line, Some(PLANTED_LINE));
        assert_eq!(finding.column, Some(PLANTED_COLUMN));
        let FindingDetail::RustTypeOutput { code, message } = &finding.detail else {
            panic!("the native arm publishes its own detail: {finding:?}");
        };
        assert_eq!(code, "E0425");
        assert!(
            message.contains("dev_app_at"),
            "the refusal names the missing spelling: {message}"
        );

        fixture.source(REMOVED_PLANT);
        let cleared = fixture.analyze();

        assert_eq!(cleared.outcome, ArmOutcome::Passed);
        let cleared_timing = cleared
            .timing
            .expect("the successful compiler run reports wall time");
        assert!(cleared_timing.elapsed_ms <= cleared_timing.observation_bound_ms);
        assert_eq!(cleared.findings, Vec::<ArmFinding>::new());
    }
}
