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
    let spelling = installed
        .to_str()
        .ok_or_else(|| unrepresentable(&installed, "is not UTF-8"))?;
    if !installed.is_absolute() {
        return Err(unrepresentable(&installed, "is not absolute"));
    }
    if spelling.chars().any(is_line_separator) {
        return Err(unrepresentable(&installed, "contains a line separator"));
    }
    let prelude = match hook {
        GitHook::PreCommit => PRE_COMMIT_ENVIRONMENT,
        GitHook::CommitMessage | GitHook::ReferenceTransaction => "",
    };
    Ok(format!(
        "#!/bin/sh -eu\n{prelude}exec {} gate {} \"$@\"\n",
        quote(spelling),
        hook.command(),
    )
    .into_bytes())
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
            let path = PathBuf::from(format!("/git{separator}common"));
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
