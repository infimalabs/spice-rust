//! Canonical compiler environment shared by Cargo-launched and installed rims.

use std::env;
use std::ffi::{OsStr, OsString};
use std::path::Path;

use crate::cargo_isolation::{
    BOOTSTRAP_HOME_VARIABLE, Host, IsolationError, NATIVE_CARGO_VARIABLE, prepare_manual,
};
use crate::repository::Repository;

pub(super) const SHELL_LAST_COMMAND: &str = "_";
const SHELL_OLD_DIRECTORY: &str = "OLDPWD";
const SHELL_LEVEL: &str = "SHLVL";
const HOME: &str = "HOME";
const CARGO_HOME: &str = "CARGO_HOME";
const RUSTUP_HOME: &str = "RUSTUP_HOME";
pub(super) const SHELL_HOOK_PREFIX: &[u8] = b"SPICE_SHELL_HOOK_"; // env-policy: allow

#[expect(
    clippy::disallowed_methods,
    reason = "the canonical gate command rim owns full-verifier environment acquisition"
)]
pub(super) fn verifier_environment() -> Vec<(OsString, OsString)> {
    verifier_environment_from(env::vars_os())
}

/// Reconstruct the compiler environment used by an isolated development verifier.
///
/// An installed hook normally observes its lane-private Cargo home, while
/// `spice dev cargo` gives the verifier a worktree-private manual home. The
/// bootstrap/native markers prove that both are views of the same admitted
/// isolation store; resolve that store's manual home before exact receipt
/// admission. Unmarked ordinary shells retain their ambient Cargo authority.
#[expect(
    clippy::disallowed_methods,
    reason = "exact evidence admission captures one ambient isolation authority"
)]
pub(super) fn exact_evidence_environment(
    repository: &Repository,
) -> Result<Vec<(OsString, OsString)>, IsolationError> {
    let ambient = env::vars_os().collect::<Vec<_>>();
    if !has_isolation_markers(&ambient) {
        return Ok(verifier_environment_from(ambient));
    }
    let host = Host::from_environment(ambient.clone())?;
    let prepared = prepare_manual(repository, &host)?;
    Ok(verifier_environment_from_with_cargo_home(
        ambient,
        prepared.home(),
    ))
}

pub(super) fn verifier_environment_from_with_cargo_home(
    environment: impl IntoIterator<Item = (OsString, OsString)>,
    cargo_home: &Path,
) -> Vec<(OsString, OsString)> {
    let mut environment = environment.into_iter().collect::<Vec<_>>();
    set_environment_value(&mut environment, CARGO_HOME, cargo_home);
    verifier_environment_from(environment)
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
        && !name.starts_with(SHELL_HOOK_PREFIX)
        && name != SHELL_LEVEL.as_bytes()
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
        && name != b"GIT_DIR"
        && name != b"GIT_EDITOR"
        && name != b"GIT_EXEC_PATH"
        && name != b"GIT_INDEX_FILE"
        && name != b"GIT_PREFIX"
}

fn has_isolation_markers(environment: &[(OsString, OsString)]) -> bool {
    environment.iter().any(|(name, _)| {
        name == OsStr::new(BOOTSTRAP_HOME_VARIABLE) || name == OsStr::new(NATIVE_CARGO_VARIABLE)
    })
}

fn set_environment_value(environment: &mut Vec<(OsString, OsString)>, name: &str, value: &Path) {
    match environment
        .iter_mut()
        .find(|(candidate, _)| candidate == OsStr::new(name))
    {
        Some((_, current)) => *current = value.as_os_str().to_owned(),
        None => environment.push((OsString::from(name), value.as_os_str().to_owned())),
    }
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
