//! Read-only suggestions about authored shell input, never execution policy.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
enum Hint {
    RedundantCd,
    TrailingTail,
}

impl Hint {
    fn text(self) -> &'static str {
        match self {
            Self::RedundantCd => {
                "Already in this working directory; omit the leading `cd` and run the command directly."
            }
            Self::TrailingTail => {
                "A final `tail` is probably unnecessary; rely on the tool's native output wrapping."
            }
        }
    }
}

type Receipts = BTreeMap<String, BTreeMap<String, BTreeSet<Hint>>>;

/// Claim each matching hint once per vendor session across processes and linked
/// worktrees. Receipts precede delivery: a crash can lose optional advice, never
/// repeat it. Missing identity or unavailable storage silently suppresses advice.
/// Neither advice nor receipt failures may change command execution.
pub fn for_session(
    command: &str,
    cwd: Option<&Path>,
    repository_start: &Path,
    vendor: &str,
    session: Option<&str>,
) -> impl Iterator<Item = &'static str> {
    let mut hints = [
        cwd.and_then(|cwd| redundant_cd(command, cwd))
            .map(|()| Hint::RedundantCd),
        trailing_tail(command).then_some(Hint::TrailingTail),
    ];
    let claimed = session
        .filter(|session| !session.is_empty())
        .filter(|_| hints.iter().any(Option::is_some))
        .and_then(|session| claim(repository_start, vendor, session, &mut hints));
    if claimed.is_none() {
        hints.fill(None);
    }
    hints.into_iter().flatten().map(Hint::text)
}

fn claim(
    repository_start: &Path,
    vendor: &str,
    session: &str,
    hints: &mut [Option<Hint>; 2],
) -> Option<()> {
    let repository = crate::repository::Repository::discover(repository_start).ok()?;
    let (state_path, lock_path) = repository.admit_command_advice().ok()?;
    fs::create_dir_all(lock_path.parent()?).ok()?;
    let name = sill::LockName::new("command-advice").expect("fixed advice lock name is valid");
    let sill::Acquisition::Acquired(_possession) =
        sill::LockFile::new(name, lock_path).try_acquire().ok()?
    else {
        return None;
    };
    let mut receipts: Receipts = match fs::read(&state_path) {
        Ok(bytes) => serde_json::from_slice(&bytes).ok()?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
        Err(_) => return None,
    };
    let seen = receipts
        .entry(vendor.to_owned())
        .or_default()
        .entry(session.to_owned())
        .or_default();
    for hint in hints.iter_mut() {
        if hint.is_some_and(|hint| !seen.insert(hint)) {
            *hint = None;
        }
    }
    if hints.iter().any(Option::is_some) {
        let bytes = serde_json::to_vec(&receipts).ok()?;
        atomic::Destination::new(&state_path)
            .ok()?
            .publish(|staged| staged.write_all(&bytes))
            .ok()?;
    }
    Some(())
}

/// Only literal directory operands are comparable. Do not resolve symlinks:
/// their logical spelling can change subsequent relative-path interpretation.
fn redundant_cd(command: &str, cwd: &Path) -> Option<()> {
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
    (Path::new(path).is_absolute() && Path::new(path) == cwd).then_some(())
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
