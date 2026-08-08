//! Per-process Git configuration that keeps a resident lane upstream-blind.
//!
//! The generated system-scope merge points the lane at itself. Git's upstream
//! resolver reads that first value, while `git config --get` reads the final
//! command-scope merge carrying the real integration branch. The repository's
//! shared and local configuration are never changed.

use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use sill::Environment;

use crate::repository::Repository;

const AGENT_GITCONFIG_NAME: &str = "agent.gitconfig";
const GIT_CONFIG_COUNT: &str = "GIT_CONFIG_COUNT"; // env-policy: allow
const GIT_CONFIG_KEY_PREFIX: &str = "GIT_CONFIG_KEY_"; // env-policy: allow
const GIT_CONFIG_SYSTEM: &str = "GIT_CONFIG_SYSTEM"; // env-policy: allow
const GIT_CONFIG_VALUE_PREFIX: &str = "GIT_CONFIG_VALUE_"; // env-policy: allow
const GIT_EDITOR: &str = "GIT_EDITOR"; // env-policy: allow

/// Add the lane's Git shadow to one already admitted child environment.
///
/// A detached head or a path that Git cannot identify stays on the native
/// environment, matching the predecessor's inactive-shadow behavior.
///
/// # Errors
///
/// Returns an error when Git cannot launch, successful probe output is not
/// text, the per-worktree shadow cannot be published atomically, or the final
/// process environment is invalid.
pub fn environment(repository: &Repository, base: Environment) -> Result<Environment, Error> {
    let native = native_environment(&base);
    let branch = git_read(
        repository.worktree_root(),
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        &native,
    )?;
    let git_dir = git_read(
        repository.worktree_root(),
        &["rev-parse", "--absolute-git-dir"],
        &native,
    )?;
    let Some(branch) = branch else {
        if git_dir.is_some() {
            eprintln!(concat!(
                "spice git shadow inactive: no current branch (detached HEAD); ",
                "native upstream config is in effect"
            ),);
        }
        return Ok(base);
    };
    let Some(git_dir) = git_dir else {
        return Ok(base);
    };
    let true_merge = true_branch_merge(repository.worktree_root(), &branch, &native)?;
    let system_config = match system_config_path(repository.worktree_root(), &native)? {
        Some(path) => path,
        None => {
            eprintln!(concat!(
                "spice git shadow: could not resolve git's system config path; ",
                "falling back to /etc/gitconfig (Homebrew/Xcode system git settings ",
                "may be dropped)"
            ),);
            "/etc/gitconfig".to_owned()
        }
    };
    let shadow = publish_shadow(Path::new(&git_dir), &branch, &system_config)?;

    let mut entries = base.entries().iter().cloned().collect::<BTreeMap<_, _>>();
    entries.insert(OsString::from(GIT_CONFIG_SYSTEM), shadow.into_os_string());
    if let Some(merge) = true_merge {
        append_config_pair(&mut entries, &format!("branch.{branch}.merge"), &merge);
    }
    append_config_pair(&mut entries, &format!("branch.{branch}.remote"), ".");
    Environment::new(entries)
        .map_err(|error| Error::new("compose Git shadow environment", error.to_string()))
}

fn native_environment(environment: &Environment) -> BTreeMap<OsString, OsString> {
    environment
        .entries()
        .iter()
        .filter(|(key, _)| !is_shadow_variable(key))
        .cloned()
        .collect()
}

fn is_shadow_variable(key: &OsStr) -> bool {
    if key == GIT_CONFIG_SYSTEM || key == GIT_CONFIG_COUNT {
        return true;
    }
    key.to_str().is_some_and(|key| {
        key.starts_with(GIT_CONFIG_KEY_PREFIX) || key.starts_with(GIT_CONFIG_VALUE_PREFIX)
    })
}

fn true_branch_merge(
    root: &Path,
    branch: &str,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<Option<String>, Error> {
    if let Some(merge) = git_read(
        root,
        &["config", "--get", &format!("branch.{branch}.merge")],
        environment,
    )? {
        return Ok(Some(merge));
    }
    let Some(head) = git_read(
        root,
        &["symbolic-ref", "refs/remotes/origin/HEAD"],
        environment,
    )?
    else {
        return Ok(None);
    };
    Ok(head
        .strip_prefix("refs/remotes/origin/")
        .map(|branch| format!("refs/heads/{branch}")))
}

fn system_config_path(
    root: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<Option<String>, Error> {
    let mut query = environment.clone();
    query.insert(OsString::from(GIT_EDITOR), OsString::from("echo"));
    git_read(root, &["config", "--system", "--edit"], &query)
}

fn git_read(
    root: &Path,
    arguments: &[&str],
    environment: &BTreeMap<OsString, OsString>,
) -> Result<Option<String>, Error> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env_clear()
        .envs(environment)
        .output()
        .map_err(|error| Error::new("launch Git shadow probe", error.to_string()))?;
    if !output.status.success() {
        return Ok(None);
    }
    let value = String::from_utf8(output.stdout)
        .map_err(|error| Error::new("read Git shadow probe", error.to_string()))?;
    let value = value.trim();
    Ok((!value.is_empty()).then(|| value.to_owned()))
}

/// Writes the whole shadow, or leaves the identical file already there.
///
/// The header says `agent` deliberately. It is not this port's name for the
/// mechanism -- [`crate::repository`] calls that the Git shadow throughout --
/// but the first line of a file the predecessor's `spice/agent/shadow.py` also
/// writes whole. Both read back before writing, so the two only stay out of
/// each other's way while the bytes match exactly; rewording this line on one
/// side makes them overwrite each other on every alternating run, and nothing
/// in the running system notices. `tests/git_shadow.rs` pins it so a sweep
/// that reaches the word reds instead of shipping.
fn publish_shadow(git_dir: &Path, branch: &str, system_config: &str) -> Result<PathBuf, Error> {
    let path = git_dir.join(AGENT_GITCONFIG_NAME);
    let subsection = branch.replace('\\', "\\\\").replace('"', "\\\"");
    let content = format!(
        concat!(
            "# Generated by spice for the per-process agent git shadow.\n",
            "[include]\n",
            "\tpath = {system_config}\n",
            "[branch \"{subsection}\"]\n",
            "\tmerge = refs/heads/{branch}\n",
        ),
        system_config = system_config,
        subsection = subsection,
        branch = branch,
    );
    if fs::read(&path).ok().as_deref() == Some(content.as_bytes()) {
        return Ok(path);
    }
    let destination = atomic::Destination::new(&path)
        .map_err(|error| Error::new("resolve Git shadow", error.to_string()))?;
    destination
        .publish(|staged| staged.write_all(content.as_bytes()))
        .map_err(|error| Error::new("publish Git shadow", error.to_string()))?;
    Ok(path)
}

fn append_config_pair(entries: &mut BTreeMap<OsString, OsString>, key: &str, value: &str) {
    let count = entries
        .get(OsStr::new(GIT_CONFIG_COUNT))
        .and_then(|count| count.to_str())
        .and_then(|count| count.parse::<usize>().ok())
        .unwrap_or(0);
    for index in 0..count {
        let variable = OsString::from(format!("{GIT_CONFIG_KEY_PREFIX}{index}"));
        if entries
            .get(&variable)
            .is_some_and(|candidate| candidate == key)
        {
            entries.insert(
                OsString::from(format!("{GIT_CONFIG_VALUE_PREFIX}{index}")),
                OsString::from(value),
            );
            return;
        }
    }
    entries.insert(
        OsString::from(format!("{GIT_CONFIG_KEY_PREFIX}{count}")),
        OsString::from(key),
    );
    entries.insert(
        OsString::from(format!("{GIT_CONFIG_VALUE_PREFIX}{count}")),
        OsString::from(value),
    );
    entries.insert(
        OsString::from(GIT_CONFIG_COUNT),
        OsString::from((count + 1).to_string()),
    );
}

/// One Git-shadow construction failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    action: &'static str,
    detail: String,
}

impl Error {
    fn new(action: &'static str, detail: String) -> Self {
        Self { action, detail }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.action, self.detail)
    }
}

impl StdError for Error {}
