//! Which toolchain owns the staged app bytes, and which one is running.
//!
//! Cargo folds all of `rustc --version --verbose` into every crate's
//! `-Cmetadata`, and that one report names both dimensions the staged bytes are
//! owned on: the release, and the host triple the compiler emits for. Neither
//! is a fact about this binary. What this xtask was compiled for is not what
//! the compiler on `PATH` will compile, and it is the latter that emits the
//! bytes, so both fields are read here from a single invocation -- a report
//! cannot answer for the release and leave the host to be settled from
//! somewhere else. This module reads them; [`crate::app`] decides on the
//! values.

use std::path::Path;
use std::process::Command;

use crate::app::AppBuildError;

/// The document every toolchain in this workspace is selected by.
///
/// Compiled in rather than read at run time, so the pin this binary enforces
/// is part of the binary: a check-out edited after the build cannot answer for
/// it. `proof::DEFAULT_IMAGE` is sealed into a build for the same reason.
const TOOLCHAIN_DOCUMENT: &str = include_str!("../../rust-toolchain.toml");

/// The release `rust-toolchain.toml` selects for this workspace.
///
/// Read out of the pin document rather than restated, so there is no second
/// spelling here to drift from it. The release is declared in two other places
/// -- `rust-version` in the workspace manifest and `RUSTUP_TOOLCHAIN` in the
/// sealed image -- and neither is read here, because only this document decides
/// which compiler rustup hands a build. They are proven equal to it instead.
pub(crate) fn pinned_release() -> &'static str {
    channel(TOOLCHAIN_DOCUMENT).expect("rust-toolchain.toml declares the channel it pins")
}

/// The channel a toolchain document names.
///
/// One key out of a document this repository writes, read as text rather than
/// by admitting a TOML parser into this crate to reach a single line. What it
/// accepts is exactly the shape the document has, which the test below holds it
/// to by parsing the compiled-in document rather than a restated sample.
fn channel(document: &str) -> Option<&str> {
    document
        .lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("channel"))
        .filter_map(|rest| rest.trim_start().strip_prefix('='))
        .find_map(|value| value.trim().strip_prefix('"')?.strip_suffix('"'))
}

/// What the compiler a build started here reports about itself.
///
/// Both fields come from one invocation because they describe one compiler:
/// asking twice, or asking here and settling the other half elsewhere, admits
/// a pair that no single `rustc` ever reported.
pub(crate) struct ObservedToolchain {
    release: String,
    host: String,
}

impl ObservedToolchain {
    /// The release this compiler names.
    pub(crate) fn release(&self) -> &str {
        &self.release
    }

    /// The host triple this compiler emits for.
    pub(crate) fn host(&self) -> &str {
        &self.host
    }
}

/// What the compiler a build started here would report about itself.
///
/// `--version --verbose` is asked for rather than `--version` because the
/// release and the host are named fields there instead of words inside a
/// sentence, and because that whole report is what Cargo hashes into
/// `-Cmetadata`. It is asked from the workspace, so a rustup shim resolves the
/// same toolchain file the build itself will resolve.
///
/// A report missing either field yields neither: the caller decides staging
/// rights on both, and a compiler that will not say what it is is refused
/// rather than filled in.
pub(crate) fn observed(rustc: &Path, root: &Path) -> Result<ObservedToolchain, AppBuildError> {
    let output = Command::new(rustc)
        .args(["--version", "--verbose"])
        .current_dir(root)
        .output()
        .map_err(|error| {
            AppBuildError::UnreadableToolchainReport(format!(
                "{} did not run: {error}",
                rustc.display()
            ))
        })?;
    if !output.status.success() {
        return Err(AppBuildError::UnreadableToolchainReport(format!(
            "{} reported {}",
            rustc.display(),
            output.status
        )));
    }
    let report = String::from_utf8(output.stdout).map_err(|_| {
        AppBuildError::UnreadableToolchainReport(format!(
            "{} answered with bytes that are not UTF-8",
            rustc.display()
        ))
    })?;
    let named = |name: &str| {
        field(&report, name).map(str::to_owned).ok_or_else(|| {
            AppBuildError::UnreadableToolchainReport(format!(
                "{} named no {name}: {report}",
                rustc.display()
            ))
        })
    };

    Ok(ObservedToolchain {
        release: named("release")?,
        host: named("host")?,
    })
}

/// One named field of a `rustc --version --verbose` report.
///
/// The colon is required rather than assumed, so a longer field name starting
/// with a shorter one cannot answer for it.
fn field<'a>(report: &'a str, name: &str) -> Option<&'a str> {
    report
        .lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
        .map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `RUSTUP_TOOLCHAIN` the sealed image compiles every crate under.
    const IMAGE_DOCUMENT: &str = include_str!("../../dist/proof-image/Dockerfile");

    /// A release named in three documents is enforced from one of them.
    ///
    /// `rust-toolchain.toml` selects the compiler, the workspace manifest
    /// declares the release every crate is compiled by, and the image pins the
    /// toolchain its `rustup` shims resolve. Only the first is read by the
    /// guard, so the other two are held equal to it here rather than trusted:
    /// a bump that moves one of them and not the others would otherwise leave
    /// the image compiling the stage under a release this build still admits.
    #[test]
    fn every_declaration_of_the_pinned_release_names_the_same_one() {
        assert_eq!(
            env!("CARGO_PKG_RUST_VERSION"),
            pinned_release(),
            "rust-toolchain.toml and the workspace manifest name different releases, so the \
             compiler a build resolves is not the one every crate declares",
        );

        let image = IMAGE_DOCUMENT
            .split_once("RUSTUP_TOOLCHAIN=")
            .map(|(_, rest)| rest.trim_end().lines().next().unwrap_or_default())
            .map(|value| value.trim().trim_end_matches('\\').trim())
            .expect("the image document pins RUSTUP_TOOLCHAIN");
        assert_eq!(
            image,
            pinned_release(),
            "the sealed image pins a toolchain rust-toolchain.toml does not select, so the one \
             environment allowed to write the stage would compile it under another release",
        );
    }

    /// The pin is parsed out of the document the workspace actually carries.
    ///
    /// A sample written out here would prove the parser reads a document shaped
    /// the way this test imagines. What has to hold is that it reads the file
    /// beside it, so the compiled-in document is what gets parsed.
    #[test]
    fn the_pinned_release_is_read_out_of_the_toolchain_document() {
        assert!(
            TOOLCHAIN_DOCUMENT.contains(&format!("channel = \"{}\"", pinned_release())),
            "the parsed channel is not the one rust-toolchain.toml declares: {TOOLCHAIN_DOCUMENT}",
        );
        assert_eq!(channel("[toolchain]\nprofile = \"minimal\"\n"), None);
    }

    /// A toolchain report is read for both fields the stage is owned on.
    ///
    /// The host line is read here and nowhere else. It is the tuple whose
    /// `-Cmetadata` the staged bytes carry, and it is named by the same
    /// invocation as the release, so taking it from here is what stops the two
    /// halves of one identity from describing two different compilers. Each
    /// field is asked for by name, so a report carrying one and not the other
    /// yields neither rather than the wrong one, and a name that is only the
    /// start of a field this report does carry answers with nothing rather than
    /// with that field's value.
    #[test]
    fn a_toolchain_report_yields_both_fields_the_stage_is_owned_on() {
        let report = "rustc 1.97.1 (0000000 2026-01-01)\nbinary: rustc\ncommit-hash: 0000000\n\
                      release: 1.97.1\nhost: x86_64-unknown-linux-gnu\nLLVM version: 20.1.0\n";

        assert_eq!(field(report, "release"), Some("1.97.1"));
        assert_eq!(field(report, "host"), Some("x86_64-unknown-linux-gnu"));
        assert_eq!(
            field("rustc 1.97.1\nhost: x86_64-apple-darwin\n", "release"),
            None
        );
        assert_eq!(field("rustc 1.97.1\nrelease: 1.97.1\n", "host"), None);
        assert_eq!(field(report, "commit"), None);
    }
}
