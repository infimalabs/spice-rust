//! Native guard for Git reference transactions on the current branch.

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PREPARED: &str = "prepared";
const PROTECTED_COMMIT_LIMIT: usize = 5;
const ASCII_ZERO: u8 = 0x30;

pub fn apply(
    repository: &Path,
    state: &OsStr,
    input: &str,
) -> Result<(), ReferenceTransactionError> {
    if state != OsStr::new(PREPARED) {
        return Ok(());
    }

    let Some(current_ref) = git_read(repository, &["symbolic-ref", "--quiet", "HEAD"])? else {
        return Ok(());
    };
    let Some(upstream) = git_read(
        repository,
        &["rev-parse", "--verify", "--quiet", "@{upstream}^{commit}"],
    )?
    else {
        return Ok(());
    };

    for update in parse_updates(input)? {
        if update.reference != current_ref {
            continue;
        }
        let protected = abandoned_upstream_commits(repository, &update, &upstream)?;
        if !protected.is_empty() {
            let listed = protected
                .iter()
                .map(|commit| short_oid(repository, commit))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ");
            return Err(ReferenceTransactionError::ProtectedHistory {
                reference: current_ref,
                commits: listed,
            });
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RefUpdate<'a> {
    old: &'a str,
    new: &'a str,
    reference: &'a str,
}

fn parse_updates(input: &str) -> Result<Vec<RefUpdate<'_>>, ReferenceTransactionError> {
    let mut updates = Vec::new();
    for (index, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err(ReferenceTransactionError::MalformedInput { line: index + 1 });
        }
        updates.push(RefUpdate {
            old: fields[0],
            new: fields[1],
            reference: fields[2],
        });
    }
    Ok(updates)
}

fn abandoned_upstream_commits(
    repository: &Path,
    update: &RefUpdate<'_>,
    upstream: &str,
) -> Result<Vec<String>, ReferenceTransactionError> {
    if is_zero_oid(update.old) {
        return Ok(Vec::new());
    }
    let Some(old) = commit_oid(repository, update.old)? else {
        return Ok(Vec::new());
    };
    let new = if is_zero_oid(update.new) {
        None
    } else {
        let Some(new) = commit_oid(repository, update.new)? else {
            return Ok(Vec::new());
        };
        Some(new)
    };
    if let Some(new) = new.as_deref()
        && is_ancestor(repository, &old, new)?
    {
        return Ok(Vec::new());
    }

    let Some(merge_bases) = git_read(repository, &["merge-base", "--all", &old, upstream])? else {
        return Ok(Vec::new());
    };
    let mut arguments = vec![
        "rev-list".to_owned(),
        format!("--max-count={PROTECTED_COMMIT_LIMIT}"),
    ];
    arguments.extend(merge_bases.lines().map(str::to_owned));
    if let Some(new) = new {
        arguments.push("--not".to_owned());
        arguments.push(new);
    }
    Ok(git_read_owned(repository, &arguments)?
        .map(|output| output.lines().map(str::to_owned).collect())
        .unwrap_or_default())
}

fn is_zero_oid(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte == ASCII_ZERO)
}

fn commit_oid(repository: &Path, value: &str) -> Result<Option<String>, ReferenceTransactionError> {
    if value.starts_with("ref:") {
        return Ok(None);
    }
    git_read_owned(
        repository,
        &[
            "rev-parse".to_owned(),
            "--verify".to_owned(),
            "--quiet".to_owned(),
            format!("{value}^{{commit}}"),
        ],
    )
}

fn short_oid(repository: &Path, value: &str) -> Result<String, ReferenceTransactionError> {
    Ok(git_read(repository, &["rev-parse", "--short", value])?.unwrap_or_else(|| value.to_owned()))
}

fn is_ancestor(
    repository: &Path,
    ancestor: &str,
    descendant: &str,
) -> Result<bool, ReferenceTransactionError> {
    Ok(git(
        repository,
        &["merge-base", "--is-ancestor", ancestor, descendant],
    )?
    .status
    .success())
}

fn git_read(
    repository: &Path,
    arguments: &[&str],
) -> Result<Option<String>, ReferenceTransactionError> {
    git_read_owned(
        repository,
        &arguments
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>(),
    )
}

fn git_read_owned(
    repository: &Path,
    arguments: &[String],
) -> Result<Option<String>, ReferenceTransactionError> {
    let output = git(repository, arguments)?;
    if !output.status.success() {
        return Ok(None);
    }
    let output =
        String::from_utf8(output.stdout).map_err(|_| ReferenceTransactionError::NonUtf8Output {
            arguments: arguments.to_vec(),
        })?;
    let output = output.trim().to_owned();
    Ok((!output.is_empty()).then_some(output))
}

fn git<S>(repository: &Path, arguments: &[S]) -> Result<Output, ReferenceTransactionError>
where
    S: AsRef<OsStr>,
{
    let arguments = arguments
        .iter()
        .map(|argument| argument.as_ref().to_owned())
        .collect::<Vec<_>>();
    Command::new("git")
        .args(&arguments)
        .current_dir(repository)
        .output()
        .map_err(|source| ReferenceTransactionError::GitLaunch {
            repository: repository.to_path_buf(),
            arguments: arguments
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect(),
            source,
        })
}

/// A native `reference-transaction` refusal.
#[derive(Debug)]
pub enum ReferenceTransactionError {
    MalformedInput {
        line: usize,
    },
    GitLaunch {
        repository: PathBuf,
        arguments: Vec<String>,
        source: io::Error,
    },
    NonUtf8Output {
        arguments: Vec<String>,
    },
    ProtectedHistory {
        reference: String,
        commits: String,
    },
}

impl fmt::Display for ReferenceTransactionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedInput { line } => {
                write!(formatter, "malformed reference-transaction line {line}")
            }
            Self::GitLaunch {
                repository,
                arguments,
                source,
            } => write!(
                formatter,
                "reference-transaction could not run git -C {} {}: {source}",
                repository.display(),
                arguments.join(" ")
            ),
            Self::NonUtf8Output { arguments } => write!(
                formatter,
                "reference-transaction git output was not UTF-8: git {}",
                arguments.join(" ")
            ),
            Self::ProtectedHistory { reference, commits } => write!(
                formatter,
                concat!(
                    "reference-transaction guard refused to abandon upstream-merged commits ",
                    "on current branch {}: {}. This is expected when a task ",
                    "boundary has advanced origin/upstream; keep those commits and continue ",
                    "with an append-only commit instead of amending or resetting backwards."
                ),
                reference, commits
            ),
        }
    }
}

impl Error for ReferenceTransactionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::GitLaunch { source, .. } => Some(source),
            Self::MalformedInput { .. }
            | Self::NonUtf8Output { .. }
            | Self::ProtectedHistory { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_ignores_blank_lines_and_preserves_each_update() {
        let updates = parse_updates("old new refs/heads/main\n\nzero tip refs/tags/v1\n").unwrap();
        assert_eq!(
            updates,
            [
                RefUpdate {
                    old: "old",
                    new: "new",
                    reference: "refs/heads/main",
                },
                RefUpdate {
                    old: "zero",
                    new: "tip",
                    reference: "refs/tags/v1",
                },
            ]
        );
    }

    #[test]
    fn parser_names_the_first_malformed_line() {
        let error = parse_updates("old new refs/heads/main\nmissing field\n").unwrap_err();
        assert!(matches!(
            error,
            ReferenceTransactionError::MalformedInput { line: 2 }
        ));
    }

    #[test]
    fn non_prepared_states_are_inert_before_input_or_repository_access() {
        apply(
            Path::new("repository-does-not-exist"),
            OsStr::new("committed"),
            "malformed",
        )
        .unwrap();
    }
}
