//! Canonical compiler environment shared by Cargo-launched and installed rims.

use std::env;
use std::ffi::{OsStr, OsString};
use std::path::Path;

pub(super) const SHELL_LAST_COMMAND: &str = "_";
const SHELL_OLD_DIRECTORY: &str = "OLDPWD";
const HOME: &str = "HOME";
const CARGO_HOME: &str = "CARGO_HOME";
const RUSTUP_HOME: &str = "RUSTUP_HOME";

#[expect(
    clippy::disallowed_methods,
    reason = "the canonical gate command rim owns full-verifier environment acquisition"
)]
pub(super) fn verifier_environment() -> Vec<(OsString, OsString)> {
    verifier_environment_from(env::vars_os())
}

pub(super) fn verifier_environment_from(
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    let mut environment = environment
        .into_iter()
        .filter(|(name, _)| retained(name))
        .collect::<Vec<_>>();
    bind_implicit_home(&mut environment, CARGO_HOME, ".cargo");
    bind_implicit_home(&mut environment, RUSTUP_HOME, ".rustup");
    environment.sort();
    environment
}

fn retained(name: &OsStr) -> bool {
    let name = name.as_encoded_bytes();
    name != SHELL_LAST_COMMAND.as_bytes()
        && name != SHELL_OLD_DIRECTORY.as_bytes()
        && name != b"CARGO"
        && name != b"CARGO_MANIFEST_DIR"
        && name != b"CARGO_MANIFEST_PATH"
        && !name.starts_with(b"CARGO_PKG_")
        && name != b"DYLD_FALLBACK_LIBRARY_PATH"
        && name != b"RUSTUP_TOOLCHAIN"
        && name != b"RUSTUP_TOOLCHAIN_SOURCE"
        && name != b"RUST_RECURSION_COUNT"
        && name != b"GIT_AUTHOR_DATE"
        && name != b"GIT_AUTHOR_EMAIL"
        && name != b"GIT_AUTHOR_NAME"
        && name != b"GIT_COMMITTER_DATE"
        && name != b"GIT_COMMITTER_EMAIL"
        && name != b"GIT_COMMITTER_NAME"
        && name != b"GIT_EDITOR"
        && name != b"GIT_INDEX_FILE"
}

fn bind_implicit_home(environment: &mut Vec<(OsString, OsString)>, name: &str, suffix: &str) {
    if environment
        .iter()
        .any(|(candidate, _)| candidate == OsStr::new(name))
    {
        return;
    }
    let Some((_, home)) = environment
        .iter()
        .find(|(candidate, _)| candidate == OsStr::new(HOME))
    else {
        return;
    };
    environment.push((
        OsString::from(name),
        Path::new(home).join(suffix).into_os_string(),
    ));
}
