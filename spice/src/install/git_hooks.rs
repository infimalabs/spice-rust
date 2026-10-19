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
    let prelude = match hook {
        GitHook::PreCommit => PRE_COMMIT_ENVIRONMENT,
        GitHook::CommitMessage | GitHook::ReferenceTransaction => "",
    };
    Ok(format!(
        "#!/bin/sh -eu\n{prelude}exec {} gate {} \"$@\"\n",
        quote(&spelling),
        hook.command(),
    )
    .into_bytes())
}

/// `path` as Git and the hooks' shell name it.
#[cfg(not(windows))]
pub(super) fn git_spelling(path: &Path) -> Result<String, InstallError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| unrepresentable(path, "is not UTF-8"))
}

/// `path` as Git for Windows and its MSYS hook shell name it: a local drive
/// path with `/` separators. Neither reliably resolves verbatim `\\?\C:\`
/// paths, and a UNC share is no repository-local install.
#[cfg(windows)]
pub(super) fn git_spelling(path: &Path) -> Result<String, InstallError> {
    use std::path::{Component, Prefix};
    let mut components = path.components();
    let drive = match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => char::from(drive),
            _ => return Err(unrepresentable(path, "is not on a local drive")),
        },
        _ => return Err(unrepresentable(path, "is not on a local drive")),
    };
    let mut spelling = format!("{drive}:");
    for component in components {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                spelling.push('/');
                spelling.push_str(
                    name.to_str()
                        .ok_or_else(|| unrepresentable(path, "is not UTF-8"))?,
                );
            }
            _ => return Err(unrepresentable(path, "is not a normalized path")),
        }
    }
    Ok(spelling)
}

const PRE_COMMIT_ENVIRONMENT: &str = concat!(
    "SPICE_SHELL_HOOK_INSTALLED_APPLIANCE=1\n",
    "export SPICE_SHELL_HOOK_INSTALLED_APPLIANCE\n",
    "git_exec_path=${GIT_EXEC_PATH-}\n",
    "while [ -n \"$git_exec_path\" ]; do\n",
    "  case ${PATH-} in\n",
    "    \"$git_exec_path\":*) PATH=${PATH#\"$git_exec_path\":}; export PATH ;;\n",
    "    *) break ;;\n",
    "  esac\n",
    "done\n",
    "unset GIT_DIR GIT_EXEC_PATH GIT_PREFIX\n",
    "case $git_exec_path in\n",
    "  /Library/Developer/CommandLineTools/*)\n",
    "    [ \"${CPATH-}\" != /usr/local/include ] || unset CPATH\n",
    "    [ \"${LIBRARY_PATH-}\" != /usr/local/lib ] || unset LIBRARY_PATH\n",
    "    case ${SDKROOT-} in /Library/Developer/CommandLineTools/*) unset SDKROOT ;; esac\n",
    "    case ${MANPATH-} in *Library/Developer/CommandLineTools*) unset MANPATH ;; esac\n",
    "    ;;\n",
    "esac\n",
);

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
}
