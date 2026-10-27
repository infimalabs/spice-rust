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
const PREFIXES: [&str; 14] = [
    "!", "{", "command", "do", "elif", "else", "env", "exec", "if", "nohup", "then", "time",
    "until", "while",
];

/// `timeout` options that take the following word as their value.
const TIMEOUT_VALUED: [&str; 2] = ["-k", "-s"];

/// `sudo` options that take the following word as their value.
const SUDO_VALUED: [&str; 10] = ["-C", "-D", "-g", "-p", "-R", "-r", "-T", "-t", "-U", "-u"];

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
    let start = command_start(words)?;
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

/// The index of the word a simple command runs: past assignments, options,
/// the words that lead into another command, and the options and duration
/// `timeout` and `sudo` take before theirs.
fn command_start(words: &[String]) -> Option<usize> {
    let mut index = 0;
    while let Some(word) = words.get(index) {
        let word = word.as_str();
        index += 1;
        if is_assignment(word) || word.starts_with('-') || PREFIXES.contains(&word) {
            continue;
        }
        match word {
            "timeout" => index = skip_options(words, index, &TIMEOUT_VALUED) + 1,
            "sudo" => index = skip_options(words, index, &SUDO_VALUED),
            _ => return Some(index - 1),
        }
    }
    None
}

/// The index past the options starting at `index`, a `valued` option with the
/// word after it, through a closing `--`.
fn skip_options(words: &[String], mut index: usize, valued: &[&str]) -> usize {
    while let Some(word) = words.get(index).filter(|word| word.starts_with('-')) {
        index += if valued.contains(&word.as_str()) {
            2
        } else {
            1
        };
        if word == "--" {
            break;
        }
    }
    index
}

/// Split shell text into simple commands of unquoted words. Quotes and
/// backslashes group and escape as the shell does; `;`, `&`, `|`, `(`, `)`,
/// and newlines outside quotes end a simple command. A `#` that starts a word
/// comments out the rest of its line. The body of each `<<` or `<<-` heredoc
/// is data: the lines after the redirecting line, through the one equal to
/// the unquoted delimiter, start nothing, and an unterminated body runs to the
/// end. A `<<<` here-string is one word.
fn simple_commands(script: &str) -> Vec<Vec<String>> {
    let mut commands = Commands::default();
    let mut quote = None;
    let mut characters = script.chars();
    while let Some(character) = characters.next() {
        match (quote, character) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => commands.push(character),
            (Some('"'), '\\') => match characters.next() {
                Some('\n') | None => {}
                Some(escaped @ ('$' | '`' | '"' | '\\')) => commands.push(escaped),
                Some(literal) => {
                    commands.push('\\');
                    commands.push(literal);
                }
            },
            (_, '\\') => {
                if let Some(escaped) = characters.next()
                    && escaped != '\n'
                {
                    commands.push(escaped);
                }
            }
            (Some(_), _) => commands.push(character),
            (None, '\'' | '"') => {
                quote = Some(character);
                commands.word.get_or_insert_default();
            }
            (None, '#') if commands.word.is_none() => {
                let rest = characters.as_str();
                characters = rest[rest.find('\n').unwrap_or(rest.len())..].chars();
            }
            (None, '<') if characters.as_str().starts_with("<<") => {
                characters.nth(1);
                for _ in 0..3 {
                    commands.push('<');
                }
            }
            (None, '<') if characters.as_str().starts_with('<') => {
                characters.next();
                commands.end_word();
                let strip_tabs = characters.as_str().starts_with('-');
                if strip_tabs {
                    characters.next();
                }
                commands.delimiting = Some(strip_tabs);
            }
            (None, '\n') => {
                commands.end_command();
                characters = skip_bodies(characters.as_str(), commands.bodies.drain(..)).chars();
            }
            (None, ';' | '&' | '|' | '(' | ')') => commands.end_command(),
            (None, ' ' | '\t') => commands.end_word(),
            (None, _) => commands.push(character),
        }
    }
    commands.end_command();
    commands.commands
}

/// The simple commands read so far, the one being read, and the heredocs its
/// line opened.
#[derive(Default)]
struct Commands {
    commands: Vec<Vec<String>>,
    words: Vec<String>,
    word: Option<String>,
    /// After an unquoted `<<` (`false`) or `<<-` (`true`), until the
    /// delimiter word that follows it ends.
    delimiting: Option<bool>,
    /// Each delimiter this line opened, and whether its body strips leading
    /// tabs before comparing a line to it.
    bodies: Vec<(String, bool)>,
}

impl Commands {
    fn push(&mut self, character: char) {
        self.word.get_or_insert_default().push(character);
    }

    fn end_word(&mut self) {
        if let Some(word) = self.word.take() {
            match self.delimiting.take() {
                Some(strip_tabs) => self.bodies.push((word, strip_tabs)),
                None => self.words.push(word),
            }
        }
    }

    fn end_command(&mut self) {
        self.end_word();
        self.delimiting = None;
        if !self.words.is_empty() {
            self.commands.push(std::mem::take(&mut self.words));
        }
    }
}

/// The text after each heredoc body in turn, read line by line from `rest`.
fn skip_bodies(mut rest: &str, bodies: impl IntoIterator<Item = (String, bool)>) -> &str {
    for (delimiter, strip_tabs) in bodies {
        while !rest.is_empty() {
            let (line, after) = rest.split_once('\n').unwrap_or((rest, ""));
            rest = after;
            let line = if strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line
            };
            if line == delimiter {
                break;
            }
        }
    }
    rest
}

#[cfg(test)]
mod tests;
