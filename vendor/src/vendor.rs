//! Declarative agent-vendor values used by the compiled extension registry.
//!
//! A vendor is the only value that binds a launch dialect to its transcript
//! dialect. Consumers select one row by name and ask it questions; adding a
//! vendor adds one row rather than another mode branch outside this module.

use std::ffi::OsString;
use std::fmt;
use std::path::Path;

use listen::{Decoded, Dialect, Locus, LocusLengthError};

use crate::{
    ContractError, Effort, Launch, Record, SessionDeclaration, SessionId, SessionIdError, claude,
    codex,
};

/// Environment and filesystem vocabulary used to find native transcripts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TranscriptLayout {
    /// Optional environment override for the agent's state root.
    pub home_environment: &'static str,
    /// Directory beneath the user's home when the override is absent.
    pub default_home: &'static str,
    /// Directory beneath the state root containing transcript files.
    pub sessions_directory: &'static str,
    /// Optional state index used before filesystem discovery.
    pub state_index: Option<TranscriptIndex>,
}

/// Native transcript index understood by a declared vendor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TranscriptIndex {
    /// Codex's current SQLite thread-to-rollout mapping.
    CodexStateV5 { file: &'static str },
}

/// How a vendor's `exec` stdout carries assistant activity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StdoutFormat {
    /// Plain sections delimited by marker lines.
    Markers,
    /// One JSON record per line.
    JsonLines,
}

/// Vendor-owned stdout section vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StdoutContract {
    /// Marker opening an assistant prose section.
    pub assistant: &'static str,
    /// Markers closing an assistant prose section.
    pub sections: &'static [&'static str],
    /// Marker announcing compaction.
    pub compaction: &'static str,
    /// Markers proving first-turn activity before prose arrives.
    pub activity: &'static [&'static str],
    /// Physical stdout representation.
    pub format: StdoutFormat,
}

/// Whether a shell tool command has a vendor-specific outer envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolEnvelope {
    /// The command reaches the shell without a vendor-owned envelope.
    Direct,
    /// Claude's Bash tool embeds the command in an `eval` shell word.
    ClaudeEval,
}

/// Borrowed launch values selected from the typed vendor configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LaunchValues<'a> {
    /// Configured executable.
    pub binary: &'a str,
    /// Configured model.
    pub model: &'a str,
    /// Configured reasoning effort.
    pub effort: Effort,
    /// Optional personality supported by this vendor.
    pub personality: Option<&'a str>,
    /// Optional fast-mode disposition supported by this vendor.
    pub fast_mode: Option<bool>,
    /// Optional full model context window supported by this vendor.
    pub model_context_window: Option<u64>,
    /// Optional child auto-compaction window.
    pub auto_compact_window_tokens: Option<u64>,
}

type RecordRenderer = fn(OsString, &Launch) -> Result<Record, ContractError>;
/// One complete agent CLI extension value.
#[derive(Clone, Copy)]
pub struct Vendor {
    name: &'static str,
    label: &'static str,
    dialect: Dialect,
    thread_id_environment: &'static str,
    transcript: TranscriptLayout,
    stdout: StdoutContract,
    envelope: ToolEnvelope,
    render: RecordRenderer,
}

impl fmt::Debug for Vendor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Vendor")
            .field("name", &self.name)
            .field("label", &self.label)
            .field("dialect", &self.dialect)
            .field("thread_id_environment", &self.thread_id_environment)
            .field("transcript", &self.transcript)
            .field("stdout", &self.stdout)
            .field("envelope", &self.envelope)
            .finish_non_exhaustive()
    }
}

impl PartialEq for Vendor {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for Vendor {}

impl Vendor {
    /// Declare another selected name with the same argv and transcript
    /// contract as an existing vendor value.
    #[must_use]
    pub const fn named(name: &'static str, contract: &'static Self) -> Self {
        Self {
            name,
            label: name,
            dialect: contract.dialect,
            thread_id_environment: contract.thread_id_environment,
            transcript: contract.transcript,
            stdout: contract.stdout,
            envelope: contract.envelope,
            render: contract.render,
        }
    }

    /// Stable registry name and preserved-tape directory.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// Human-facing vendor spelling used in diagnostics.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        self.label
    }

    /// Pure transcript decoder selected by this vendor.
    #[must_use]
    pub const fn dialect(&self) -> Dialect {
        self.dialect
    }

    /// Live process variable carrying this vendor's session identity.
    #[must_use]
    pub const fn thread_id_environment(&self) -> &'static str {
        self.thread_id_environment
    }

    /// Native transcript discovery vocabulary.
    #[must_use]
    pub const fn transcript_layout(&self) -> TranscriptLayout {
        self.transcript
    }

    /// Stdout section vocabulary.
    #[must_use]
    pub const fn stdout(&self) -> StdoutContract {
        self.stdout
    }

    /// Vendor-specific tool-envelope policy.
    #[must_use]
    pub const fn tool_envelope(&self) -> ToolEnvelope {
        self.envelope
    }

    /// Rewrite the command nested in a vendor-specific tool envelope.
    ///
    /// A direct command has no vendor envelope and returns `None`; its caller
    /// may rewrite the command itself. Claude's Bash envelope retains its
    /// snapshot prelude and cwd-capture suffix byte-for-byte while replacing
    /// only the single shell word handed to `eval`.
    #[must_use]
    pub fn rewrite_tool_envelope(
        &self,
        command: &str,
        mut rewrite: impl FnMut(&str) -> Option<String>,
    ) -> Option<String> {
        match self.envelope {
            ToolEnvelope::Direct => None,
            ToolEnvelope::ClaudeEval => rewrite_claude_eval(command, &mut rewrite),
        }
    }

    /// Render the exact binary and argv contract selected by this value.
    ///
    /// # Errors
    ///
    /// Returns the vendor's path-specific contract refusal or the common argv
    /// refusal.
    pub fn record(
        &self,
        binary: impl Into<OsString>,
        launch: &Launch,
    ) -> Result<Record, ContractError> {
        (self.render)(binary.into(), launch)
    }

    /// Decode one complete transcript record under this vendor's mapping.
    ///
    /// # Errors
    ///
    /// Returns [`LocusLengthError`] when `locus` does not cover `bytes`.
    pub fn decode(&self, bytes: &[u8], locus: Locus) -> Result<Decoded, LocusLengthError> {
        listen::decode(self.dialect, bytes, locus)
    }

    /// Admit the session declaration carried by one decoded record.
    ///
    /// # Errors
    ///
    /// Returns [`SessionIdError`] when the vendor declared an empty identity.
    pub fn session_declaration(
        &self,
        decoded: &Decoded,
    ) -> Result<Option<SessionDeclaration>, SessionIdError> {
        decoded
            .events()
            .iter()
            .find_map(|event| match event {
                listen::Event::Session(declaration) => Some(declaration),
                _ => None,
            })
            .map(|declaration| {
                Ok(SessionDeclaration {
                    session: SessionId::new(declaration.identity.clone())?,
                    locus: declaration.locus,
                })
            })
            .transpose()
    }

    /// Whether this vendor owns a preserved transcript directory.
    #[must_use]
    pub fn owns_preserved_transcript(&self, path: &Path) -> bool {
        path.file_name().is_some_and(|name| name == self.name)
    }

    /// The neutral start prompt, containing a skill invocation and no operator
    /// prose.
    #[must_use]
    pub fn neutral_launch_prompt(&self, skill: &Path) -> String {
        format!("[$spice](<{}>)", skill.display())
    }
}

/// Built-in Codex extension value.
pub const CODEX: Vendor = Vendor {
    name: "codex",
    label: "Codex",
    dialect: Dialect::Codex,
    thread_id_environment: "CODEX_THREAD_ID", // env-policy: allow
    transcript: TranscriptLayout {
        home_environment: "CODEX_HOME",
        default_home: ".codex",
        sessions_directory: "sessions",
        state_index: Some(TranscriptIndex::CodexStateV5 {
            file: "state_5.sqlite",
        }),
    },
    stdout: StdoutContract {
        assistant: "",
        sections: &[],
        compaction: "",
        activity: &[],
        format: StdoutFormat::JsonLines,
    },
    envelope: ToolEnvelope::Direct,
    render: codex_record,
};

/// Built-in Claude extension value.
pub const CLAUDE: Vendor = Vendor {
    name: "claude",
    label: "Claude",
    dialect: Dialect::Claude,
    thread_id_environment: "CLAUDE_CODE_SESSION_ID", // env-policy: allow
    transcript: TranscriptLayout {
        home_environment: "CLAUDE_CONFIG_DIR",
        default_home: ".claude",
        sessions_directory: "projects",
        state_index: None,
    },
    stdout: StdoutContract {
        assistant: "",
        sections: &[],
        compaction: "",
        activity: &[],
        format: StdoutFormat::JsonLines,
    },
    envelope: ToolEnvelope::ClaudeEval,
    render: claude_record,
};

fn codex_record(binary: OsString, launch: &Launch) -> Result<Record, ContractError> {
    codex::record(binary, launch)
}

fn claude_record(binary: OsString, launch: &Launch) -> Result<Record, ContractError> {
    claude::record(binary, launch)
}

fn rewrite_claude_eval(
    command: &str,
    rewrite: &mut dyn FnMut(&str) -> Option<String>,
) -> Option<String> {
    let (start, end) = claude_eval_word(command)?;
    let nested = shell_word(&command[start..end])?;
    let rewritten = rewrite(&nested)?;
    Some(format!(
        "{}{}{}",
        &command[..start],
        quote_shell_word(&rewritten),
        &command[end..]
    ))
}

fn claude_eval_word(command: &str) -> Option<(usize, usize)> {
    let bytes = command.as_bytes();
    let mut cursor = 0;
    while cursor + 2 <= bytes.len() {
        let relative = command[cursor..].find("&&")?;
        cursor += relative + 2;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if !command[cursor..].starts_with("eval") {
            continue;
        }
        cursor += "eval".len();
        if !bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            continue;
        }
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let end = shell_word_end(command, cursor)?;
        return (end > cursor).then_some((cursor, end));
    }
    None
}

fn shell_word_end(text: &str, start: usize) -> Option<usize> {
    let mut cursor = start;
    let mut quote = None;
    while cursor < text.len() {
        let character = text[cursor..].chars().next()?;
        match quote {
            Some('\'') => {
                if character == '\'' {
                    quote = None;
                }
                cursor += character.len_utf8();
            }
            Some('"') => {
                if character == '\\' {
                    cursor += character.len_utf8();
                    let escaped = text[cursor..].chars().next()?;
                    cursor += escaped.len_utf8();
                } else {
                    if character == '"' {
                        quote = None;
                    }
                    cursor += character.len_utf8();
                }
            }
            Some(_) => unreachable!("the parser admits only shell quote bytes"),
            None if character.is_ascii_whitespace() => break,
            None if matches!(character, '\'' | '"') => {
                quote = Some(character);
                cursor += character.len_utf8();
            }
            None if character == '\\' => {
                cursor += character.len_utf8();
                let escaped = text[cursor..].chars().next()?;
                cursor += escaped.len_utf8();
            }
            None => cursor += character.len_utf8(),
        }
    }
    quote.is_none().then_some(cursor)
}

fn shell_word(text: &str) -> Option<String> {
    let mut value = String::new();
    let mut cursor = 0;
    let mut quote = None;
    while cursor < text.len() {
        let character = text[cursor..].chars().next()?;
        match quote {
            Some('\'') => {
                if character == '\'' {
                    quote = None;
                } else {
                    value.push(character);
                }
                cursor += character.len_utf8();
            }
            Some('"') => {
                if character == '"' {
                    quote = None;
                    cursor += character.len_utf8();
                } else if character == '\\' {
                    cursor += character.len_utf8();
                    let escaped = text[cursor..].chars().next()?;
                    value.push(escaped);
                    cursor += escaped.len_utf8();
                } else {
                    value.push(character);
                    cursor += character.len_utf8();
                }
            }
            Some(_) => unreachable!("the parser admits only shell quote bytes"),
            None if matches!(character, '\'' | '"') => {
                quote = Some(character);
                cursor += character.len_utf8();
            }
            None if character == '\\' => {
                cursor += character.len_utf8();
                let escaped = text[cursor..].chars().next()?;
                value.push(escaped);
                cursor += escaped.len_utf8();
            }
            None => {
                value.push(character);
                cursor += character.len_utf8();
            }
        }
    }
    quote.is_none().then_some(value)
}

fn quote_shell_word(value: &str) -> String {
    if !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-'
                )
        })
    {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_vendor_value_owns_every_crossing_shape() {
        assert_eq!(CODEX.stdout().format, StdoutFormat::JsonLines);
        assert_eq!(CLAUDE.stdout().format, StdoutFormat::JsonLines);
        assert_eq!(CODEX.tool_envelope(), ToolEnvelope::Direct);
        assert_eq!(CLAUDE.tool_envelope(), ToolEnvelope::ClaudeEval);
        assert_eq!(
            CODEX.transcript_layout().state_index,
            Some(TranscriptIndex::CodexStateV5 {
                file: "state_5.sqlite"
            })
        );
        assert_eq!(CLAUDE.transcript_layout().sessions_directory, "projects");
        assert_eq!(
            CODEX.neutral_launch_prompt(Path::new(".agents/skills/spice/SKILL.md")),
            CLAUDE.neutral_launch_prompt(Path::new(".agents/skills/spice/SKILL.md")),
        );
    }

    #[test]
    fn claude_rewrites_only_its_nested_eval_word() {
        let envelope = "source '/tmp/snapshot' && eval 'git status' < /dev/null && pwd -P";
        let rewritten = CLAUDE
            .rewrite_tool_envelope(envelope, |command| {
                assert_eq!(command, "git status");
                Some("rtk git status".to_owned())
            })
            .expect("Claude envelope is recognized");
        assert_eq!(
            rewritten,
            "source '/tmp/snapshot' && eval 'rtk git status' < /dev/null && pwd -P"
        );
        assert!(
            CODEX
                .rewrite_tool_envelope(envelope, |command| Some(command.to_owned()))
                .is_none()
        );
        assert!(
            CLAUDE
                .rewrite_tool_envelope("git status", |command| Some(command.to_owned()))
                .is_none()
        );
    }

    #[test]
    fn claude_envelope_words_preserve_unicode_across_every_quoted_shape() {
        let cases = [
            (
                "source 'café-snapshot' && eval 'printf \"café 東京\"' < /dev/null && pwd -P",
                "printf \"café 東京\"",
                "source 'café-snapshot' && eval 'printf \"café 東京\"' < /dev/null && pwd -P",
            ),
            (
                "source '/tmp/snapshot' && eval \"printf \\\"café 東京\\\"\" < /dev/null && pwd -P",
                "printf \"café 東京\"",
                "source '/tmp/snapshot' && eval 'printf \"café 東京\"' < /dev/null && pwd -P",
            ),
            (
                "source '/tmp/snapshot' && eval printf\\ café\\ 東京 < /dev/null && pwd -P",
                "printf café 東京",
                "source '/tmp/snapshot' && eval 'printf café 東京' < /dev/null && pwd -P",
            ),
        ];

        for (envelope, nested, expected) in cases {
            let rewritten = CLAUDE
                .rewrite_tool_envelope(envelope, |command| {
                    assert_eq!(command, nested);
                    Some(command.to_owned())
                })
                .expect("the complete Unicode shell word is recognized");
            assert_eq!(rewritten, expected);
        }
    }

    #[test]
    fn malformed_claude_envelopes_never_offer_a_partial_shell_word() {
        for envelope in [
            "source '/tmp/snapshot' && eval 'printf café",
            "source '/tmp/snapshot' && eval printf\\",
            "source '/tmp/snapshot' && printf 'café'",
        ] {
            assert!(
                CLAUDE
                    .rewrite_tool_envelope(envelope, |_| Some("unreachable".to_owned()))
                    .is_none(),
                "malformed envelope was admitted: {envelope}"
            );
        }
    }
}
