//! Read-only suggestions about authored shell input, never execution policy.

use std::path::Path;

/// Suggest omitting a leading literal `cd` to the command's admitted working
/// directory. The caller supplies the cwd before execution, not a shell's
/// post-command cwd or the resident process's directory.
///
/// Only a plain `cd [--] ABSOLUTE_PATH` followed by `&&`, `;`, or a newline is
/// admitted. Expansions, redirections, pipelines, options, and uncertain syntax
/// remain silent. In particular, do not resolve symlinks: changing the logical
/// directory spelling can change what subsequent relative paths mean.
///
/// There is deliberately no `tail` suggestion: the admitted shell tools expose
/// no verified native last-N-lines selector. A token cap is not equivalent.
#[must_use]
pub fn for_shell(command: &str, cwd: &Path) -> Option<&'static str> {
    if !cwd.is_absolute() {
        return None;
    }
    let command = command.trim_start_matches([' ', '\t']);
    let after_cd = command.strip_prefix("cd")?;
    if !after_cd.starts_with([' ', '\t']) {
        return None;
    }
    let prefix = literal_prefix(command)?;
    let words = shell_words::split(prefix).ok()?;
    let path = match words.as_slice() {
        [cd, path] if cd == "cd" => path,
        [cd, option, path] if cd == "cd" && option == "--" => path,
        _ => return None,
    };
    (Path::new(path).is_absolute() && Path::new(path) == cwd).then_some(
        "Already in this working directory; omit the leading `cd` and run the command directly.",
    )
}

/// Find one literal simple-command prefix without interpreting any shell input.
/// `shell_words` owns unquoting after this scanner excludes expansion and syntax.
fn literal_prefix(command: &str) -> Option<&str> {
    let mut chars = command.char_indices();
    let mut quote = None;
    while let Some((offset, character)) = chars.next() {
        match (quote, character) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => {}
            (_, '\\') => {
                let (_, escaped) = chars.next()?;
                if escaped == '\n' || escaped == '\r' {
                    return None;
                }
            }
            (_, '$' | '`') => return None,
            (Some('"'), _) => {}
            (None, '\'' | '"') => quote = Some(character),
            (None, '&') => {
                let rest = command[offset..].strip_prefix("&&")?;
                return (!rest.trim().is_empty()).then_some(&command[..offset]);
            }
            (None, ';' | '\n') => {
                let rest = &command[offset + character.len_utf8()..];
                return (!rest.trim().is_empty()).then_some(&command[..offset]);
            }
            (None, '|' | '<' | '>' | '(' | ')' | '*' | '?' | '[' | ']' | '{' | '}' | '~' | '#') => {
                return None;
            }
            _ => {}
        }
    }
    None
}
