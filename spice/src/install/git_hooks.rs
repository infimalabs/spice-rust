//! Repository-bound Git-hook launcher rendering.

use std::path::{Path, PathBuf};

use super::{BINARY_PATH, InstallError};

/// Installed Spice gate selected by one generated Git hook.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GitHook {
    PreCommit,
    CommitMessage,
    ReferenceTransaction,
}

impl GitHook {
    const fn command(self) -> &'static str {
        match self {
            Self::PreCommit => "pre-commit",
            Self::CommitMessage => "commit-msg",
            Self::ReferenceTransaction => "reference-transaction",
        }
    }
}

/// Render one repository-bound launcher for the installed Spice.
pub(super) fn launcher(common_git: &Path, hook: GitHook) -> Result<Vec<u8>, InstallError> {
    let installed = common_git.join(BINARY_PATH);
    if !installed.is_absolute() {
        return Err(unrepresentable(&installed, "is not absolute"));
    }
    let spelling = git_spelling(&installed)?;
    if spelling.chars().any(is_line_separator) {
        return Err(unrepresentable(&installed, "contains a line separator"));
    }
    Ok(format!(
        "#!/bin/sh -eu\nexec {} gate {} \"$@\"\n",
        quote(&spelling),
        hook.command(),
    )
    .into_bytes())
}

/// `path` as Git and the hooks' shell name it.
pub(super) fn git_spelling(path: &Path) -> Result<String, InstallError> {
    crate::repository::git_spelling(path)
        .map(std::borrow::Cow::into_owned)
        .map_err(|reason| unrepresentable(path, reason))
}

fn quote(spelling: &str) -> String {
    let mut quoted = String::with_capacity(spelling.len() + 2);
    quoted.push('\'');
    for character in spelling.chars() {
        if character == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');
    quoted
}

const fn is_line_separator(character: char) -> bool {
    matches!(
        character,
        '\n' | '\r' | '\u{000b}' | '\u{000c}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
    )
}

fn unrepresentable(path: &Path, reason: &'static str) -> InstallError {
    InstallError::HookPathUnrepresentable {
        path: PathBuf::from(path),
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_separator_refuses_before_launcher_bytes_exist() {
        for separator in [
            '\n', '\r', '\u{000b}', '\u{000c}', '\u{0085}', '\u{2028}', '\u{2029}',
        ] {
            let root = if cfg!(windows) { "C:/git" } else { "/git" };
            let path = PathBuf::from(format!("{root}{separator}common"));
            let error = launcher(&path, GitHook::PreCommit).unwrap_err();
            assert!(matches!(
                error,
                InstallError::HookPathUnrepresentable {
                    reason: "contains a line separator",
                    ..
                }
            ));
        }
    }

    #[test]
    fn every_hook_launcher_contains_one_exec_command_and_no_environment_prelude() {
        let root = if cfg!(windows) {
            Path::new("C:/git/common")
        } else {
            Path::new("/git/common")
        };
        for hook in [
            GitHook::PreCommit,
            GitHook::CommitMessage,
            GitHook::ReferenceTransaction,
        ] {
            let launcher = String::from_utf8(launcher(root, hook).expect("render launcher"))
                .expect("launcher is UTF-8");
            let lines = launcher.lines().collect::<Vec<_>>();
            assert_eq!(lines.len(), 2, "{hook:?}: {launcher}");
            assert_eq!(lines[0], "#!/bin/sh -eu");
            assert!(lines[1].starts_with("exec "), "{hook:?}: {launcher}");
            assert_eq!(launcher.matches("exec ").count(), 1, "{hook:?}: {launcher}");
            assert!(!launcher.contains("GIT_EXEC_PATH"), "{hook:?}: {launcher}");
        }
    }
}
