//! Refusal of a direct Chromium launch that would read the macOS Keychain.
//!
//! Chromium on macOS keeps the key that encrypts its profile secrets in the
//! login Keychain unless it starts with `--use-mock-keychain`. A binary the
//! Keychain item does not already trust, such as a Chrome for Testing build
//! with a fresh profile, makes macOS ask the operator for their password.
//! Playwright and Puppeteer pass the flag on every launch; a lane that runs
//! the browser executable itself does not, so a lane shell command that
//! starts one without the flag is refused before it runs.
//!
//! This reads authored shell text lexically, as the shell would split words
//! and simple commands, without evaluating expansions. A browser named only
//! through a variable or started by another program is out of its reach.

use std::ffi::OsString;
use std::path::Path;

/// The switch that keeps Chromium's profile key out of the macOS Keychain.
pub const MOCK_KEYCHAIN: &str = "--use-mock-keychain";

/// Executable names of the Chromium family, compared case-insensitively.
const CHROMIUM_EXECUTABLES: [&str; 14] = [
    "brave browser",
    "chrome",
    "chrome-headless-shell",
    "chromium",
    "chromium-browser",
    "google chrome",
    "google chrome beta",
    "google chrome canary",
    "google chrome dev",
    "google chrome for testing",
    "google-chrome",
    "google-chrome-stable",
    "headless_shell",
    "microsoft edge",
];

/// Words that lead into the command they precede rather than naming it.
const PREFIXES: [&str; 11] = [
    "!", "{", "command", "do", "else", "env", "exec", "nohup", "then", "time", "elif",
];

/// The refusal for an argument vector the lane's shell wrapper is about to
/// run: a shell given `-c SCRIPT` is judged by its script, any other vector
/// as one simple command.
#[must_use]
pub fn argv_refusal(command: &[OsString]) -> Option<String> {
    if let Some(script) = crate::rtk::shell_command_index(command).map(|index| &command[index]) {
        return refusal(&script.to_string_lossy());
    }
    let words = command
        .iter()
        .map(|word| word.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    simple_command_refusal(&words)
}

/// The refusal for one authored shell command that starts a Chromium-family
/// executable without [`MOCK_KEYCHAIN`], or `None` when it starts none.
#[must_use]
pub fn refusal(script: &str) -> Option<String> {
    simple_commands(script)
        .iter()
        .find_map(|words| simple_command_refusal(words))
}

fn simple_command_refusal(words: &[String]) -> Option<String> {
    let start = words.iter().position(|word| {
        !is_assignment(word) && !word.starts_with('-') && !PREFIXES.contains(&word.as_str())
    })?;
    let executable = Path::new(&words[start]).file_name()?.to_str()?;
    let family = CHROMIUM_EXECUTABLES
        .iter()
        .any(|name| executable.eq_ignore_ascii_case(name));
    if !family || words[start + 1..].iter().any(|word| word == MOCK_KEYCHAIN) {
        return None;
    }
    Some(format!(
        "spice: browser::keychain-unmocked; `{executable}` started without {MOCK_KEYCHAIN} \
         reads the macOS login Keychain and asks the operator for their password; launch \
         Chromium through Playwright (chromium.launch passes {MOCK_KEYCHAIN}), or add \
         {MOCK_KEYCHAIN} --password-store=basic to this command"
    ))
}

/// `NAME=value` words that set the environment of the command after them.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|first: char| first.is_ascii_digit())
            && name
                .chars()
                .all(|character| character == '_' || character.is_ascii_alphanumeric())
    })
}

/// Split shell text into simple commands of unquoted words. Quotes and
/// backslashes group and escape as the shell does; `;`, `&`, `|`, `(`, `)`,
/// and newlines outside quotes end a simple command.
fn simple_commands(script: &str) -> Vec<Vec<String>> {
    let mut commands = Vec::new();
    let mut words = Vec::new();
    let mut word = None::<String>;
    let mut quote = None;
    let mut characters = script.chars();
    while let Some(character) = characters.next() {
        match (quote, character) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => word.get_or_insert_default().push(character),
            (Some('"'), '\\') => match characters.next() {
                Some('\n') | None => {}
                Some(escaped @ ('$' | '`' | '"' | '\\')) => {
                    word.get_or_insert_default().push(escaped);
                }
                Some(literal) => word.get_or_insert_default().extend(['\\', literal]),
            },
            (_, '\\') => {
                if let Some(escaped) = characters.next()
                    && escaped != '\n'
                {
                    word.get_or_insert_default().push(escaped);
                }
            }
            (Some(_), _) => word.get_or_insert_default().push(character),
            (None, '\'' | '"') => {
                quote = Some(character);
                word.get_or_insert_default();
            }
            (None, ';' | '&' | '|' | '(' | ')' | '\n') => {
                words.extend(word.take());
                if !words.is_empty() {
                    commands.push(std::mem::take(&mut words));
                }
            }
            (None, ' ' | '\t') => words.extend(word.take()),
            (None, _) => word.get_or_insert_default().push(character),
        }
    }
    words.extend(word);
    if !words.is_empty() {
        commands.push(words);
    }
    commands
}

#[cfg(test)]
mod tests;
