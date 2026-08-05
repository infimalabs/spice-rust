//! Which toolchain release owns the staged app bytes, and which one is running.
//!
//! The owning host is a compile-time fact about this binary, but the compiler
//! it starts is not. Cargo folds all of `rustc --version --verbose` into every
//! crate's `-Cmetadata`, and that report names the release beside the host
//! triple, so two builds on the owning tuple under different releases emit
//! different bytes. The tuple is therefore half of the identity that decides
//! who may write the stage -- [`crate::app`] owns that half -- and this module
//! is the other half.

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

/// The release the compiler a build started here would report.
///
/// `--version --verbose` is asked for rather than `--version` because the
/// release is a named field there instead of a word inside a sentence, and
/// because that whole report is what Cargo hashes into `-Cmetadata`. It is
/// asked from the workspace, so a rustup shim resolves the same toolchain file
/// the build itself will resolve.
pub(crate) fn observed_release(rustc: &Path, root: &Path) -> Result<String, AppBuildError> {
    let output = Command::new(rustc)
        .args(["--version", "--verbose"])
        .current_dir(root)
        .output()
        .map_err(|error| {
            AppBuildError::UnreadableToolchainRelease(format!(
                "{} did not run: {error}",
                rustc.display()
            ))
        })?;
    if !output.status.success() {
        return Err(AppBuildError::UnreadableToolchainRelease(format!(
            "{} reported {}",
            rustc.display(),
            output.status
        )));
    }
    let report = String::from_utf8(output.stdout).map_err(|_| {
        AppBuildError::UnreadableToolchainRelease(format!(
            "{} answered with bytes that are not UTF-8",
            rustc.display()
        ))
    })?;
    release(&report).map(str::to_owned).ok_or_else(|| {
        AppBuildError::UnreadableToolchainRelease(format!(
            "{} named no release: {report}",
            rustc.display()
        ))
    })
}

/// The release field of one `rustc --version --verbose` report.
fn release(report: &str) -> Option<&str> {
    report
        .lines()
        .find_map(|line| line.strip_prefix("release:"))
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

    /// A toolchain report is read for its release and nothing beside it.
    ///
    /// The host line is the neighbour that matters: it is the dimension the
    /// tuple guard already owns, it is spelled the same way, and reading it
    /// here would make this guard answer a question it was not asked.
    #[test]
    fn a_toolchain_report_yields_its_release_field() {
        let report = "rustc 1.97.1 (0000000 2026-01-01)\nbinary: rustc\ncommit-hash: 0000000\n\
                      release: 1.97.1\nhost: x86_64-unknown-linux-gnu\nLLVM version: 20.1.0\n";

        assert_eq!(release(report), Some("1.97.1"));
        assert_eq!(release("rustc 1.97.1\nhost: x86_64-apple-darwin\n"), None);
    }
}
