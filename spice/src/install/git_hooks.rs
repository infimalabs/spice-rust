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

/// Render one exactly-two-line launcher for the repository's installed Spice.
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
    Ok(format!(
        "#!/bin/sh -eu\nexec {} gate {} \"$@\"\n",
        quote(spelling),
        hook.command(),
    )
    .into_bytes())
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
    fn launcher_is_exactly_two_lines_and_quotes_shell_metacharacters() {
        let common = Path::new("/tmp/space tab\tquote'$`\\common");
        let bytes = launcher(common, GitHook::PreCommit).expect("special path renders");
        let text = String::from_utf8(bytes).expect("launcher is UTF-8");

        assert_eq!(text.lines().count(), 2);
        assert_eq!(text.lines().next(), Some("#!/bin/sh -eu"));
        assert_eq!(
            text.lines().nth(1),
            Some(concat!(
                "exec '/tmp/space tab\tquote'\\''$`\\common/",
                ".spice-install/bin/spice' gate pre-commit \"$@\""
            ))
        );
    }

    #[test]
    fn every_hook_names_its_explicit_gate_command() {
        for (hook, command) in [
            (GitHook::PreCommit, "pre-commit"),
            (GitHook::CommitMessage, "commit-msg"),
            (GitHook::ReferenceTransaction, "reference-transaction"),
        ] {
            let text = String::from_utf8(launcher(Path::new("/git"), hook).unwrap()).unwrap();
            assert!(text.ends_with(&format!(" gate {command} \"$@\"\n")));
        }
    }

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
