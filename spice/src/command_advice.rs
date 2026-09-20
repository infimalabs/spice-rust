//! Read-only suggestions about authored shell input, never execution policy.

use std::path::Path;

/// Suggestions for each authored command, never a replacement command. Unknown
/// cwd suppresses only the directory hint, not advice about a final pipeline stage.
pub fn for_shell(command: &str, cwd: Option<&Path>) -> impl Iterator<Item = &'static str> {
    [
        cwd.and_then(|cwd| redundant_cd(command, cwd)),
        trailing_tail(command).then_some(
            "A final `tail` is probably unnecessary; rely on the tool's native output wrapping.",
        ),
    ]
    .into_iter()
    .flatten()
}

/// Only literal directory operands are comparable. Do not resolve symlinks:
/// their logical spelling can change subsequent relative-path interpretation.
fn redundant_cd(command: &str, cwd: &Path) -> Option<&'static str> {
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

fn trailing_tail(command: &str) -> bool {
    let mut stages = pipeline_stages(command);
    stages.next();
    let Some(last) = stages.last() else {
        return false;
    };
    // `|&` also pipes stderr. Flags do not gate this advisory.
    let last = last.strip_prefix('&').unwrap_or(last).trim_start();
    shell_words::split(last).is_ok_and(|words| {
        words.first().is_some_and(|word| {
            Path::new(word)
                .file_name()
                .is_some_and(|name| name == "tail")
        })
    })
}

/// Borrow shell pipeline stages, keeping quoted/escaped pipes and `||` intact.
/// Shared with session command projection; this is lexical, not shell evaluation.
pub fn pipeline_stages(command: &str) -> impl Iterator<Item = &str> {
    let mut chars = command.char_indices();
    let mut start = Some(0);
    std::iter::from_fn(move || {
        let beginning = start?;
        let mut quote = None;
        while let Some((index, character)) = chars.next() {
            if character == '\\' && quote != Some('\'') {
                chars.next();
                continue;
            }
            if let Some(open) = quote {
                if character == open {
                    quote = None;
                }
                continue;
            }
            if matches!(character, '\'' | '"') {
                quote = Some(character);
                continue;
            }
            let bytes = command.as_bytes();
            if character == '|'
                && index.checked_sub(1).and_then(|offset| bytes.get(offset)) != Some(&b'|')
                && bytes.get(index + 1) != Some(&b'|')
            {
                start = Some(index + 1);
                return Some(command[beginning..index].trim());
            }
        }
        start = None;
        Some(command[beginning..].trim())
    })
}
