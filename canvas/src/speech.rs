//! Typed speech configuration shared by the resident and operator canvas.

use std::error::Error;
use std::fmt;

use serde::de::{Deserializer, Error as _};
use serde::{Deserialize, Serialize};

/// Installed system voice retained from the predecessor configuration.
pub const DEFAULT_VOICE: &str = "Zoe (Premium)";
/// External renderer media type retained from the predecessor configuration.
pub const DEFAULT_CONTENT_TYPE: &str = "audio/wav";
/// Installed base speaking rate.
pub const DEFAULT_WORDS_PER_MINUTE: u32 = 175;

const MAX_WORDS_PER_MINUTE: u32 = 1_000;
const MAX_VOICE_BYTES: usize = 256;
const MAX_CONTENT_TYPE_BYTES: usize = 127;
const MAX_COMMAND_WORDS: usize = 64;
const MAX_COMMAND_WORD_BYTES: usize = 4_096;

/// One speech path resolved for an admitted agent worktree.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpeechBackend {
    /// Do not expose playback controls or render audio.
    Disabled,
    /// Use the browser's system speech synthesis implementation.
    #[default]
    System,
    /// Render bytes through one configured non-shell argv contract.
    External,
}

/// Effective speech configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SpeechConfig {
    backend: SpeechBackend,
    command: Vec<String>,
    voice: String,
    content_type: String,
    words_per_minute: u32,
}

impl SpeechConfig {
    /// Admit one complete effective configuration.
    ///
    /// An empty external command is representable so the one-leaf config
    /// command can select the backend before installing its local argv. The
    /// runtime reports that state as unavailable and never executes it.
    ///
    /// # Errors
    ///
    /// Refuses incoherent backends, unbounded values, and malformed media
    /// types before the resident starts.
    pub fn new(
        backend: SpeechBackend,
        command: Vec<String>,
        voice: String,
        content_type: String,
        words_per_minute: u64,
    ) -> Result<Self, SpeechConfigError> {
        if backend != SpeechBackend::External && !command.is_empty() {
            return Err(SpeechConfigError::InactiveCommand { backend });
        }
        if command.len() > MAX_COMMAND_WORDS {
            return Err(SpeechConfigError::CommandWords {
                actual: command.len(),
            });
        }
        if command.first().is_some_and(String::is_empty) {
            return Err(SpeechConfigError::EmptyProgram);
        }
        for (index, word) in command.iter().enumerate() {
            if word.len() > MAX_COMMAND_WORD_BYTES {
                return Err(SpeechConfigError::CommandWordBytes {
                    index,
                    actual: word.len(),
                });
            }
            if word.contains('\0') {
                return Err(SpeechConfigError::CommandNul { index });
            }
        }
        if voice.is_empty() || voice.len() > MAX_VOICE_BYTES {
            return Err(SpeechConfigError::VoiceBytes {
                actual: voice.len(),
            });
        }
        if !valid_content_type(&content_type) {
            return Err(SpeechConfigError::ContentType);
        }
        let words_per_minute =
            u32::try_from(words_per_minute).map_err(|_| SpeechConfigError::WordsPerMinute {
                actual: words_per_minute,
            })?;
        if words_per_minute == 0 || words_per_minute > MAX_WORDS_PER_MINUTE {
            return Err(SpeechConfigError::WordsPerMinute {
                actual: u64::from(words_per_minute),
            });
        }
        Ok(Self {
            backend,
            command,
            voice,
            content_type,
            words_per_minute,
        })
    }

    /// Selected playback backend.
    #[must_use]
    pub const fn backend(&self) -> SpeechBackend {
        self.backend
    }

    /// External renderer argv, or `None` while another backend is selected or
    /// the external backend has not been configured yet.
    #[must_use]
    pub fn command(&self) -> Option<&[String]> {
        (self.backend == SpeechBackend::External && !self.command.is_empty())
            .then_some(self.command.as_slice())
    }

    /// System voice selected for browser synthesis.
    #[must_use]
    pub fn voice(&self) -> &str {
        &self.voice
    }

    /// Declared media type for external renderer output.
    #[must_use]
    pub fn content_type(&self) -> &str {
        &self.content_type
    }

    /// Base speaking rate shared by system and external backends.
    #[must_use]
    pub const fn words_per_minute(&self) -> u32 {
        self.words_per_minute
    }

    /// Exact external argv with the one documented rate slot resolved.
    #[must_use]
    pub fn resolved_command(&self) -> Option<Vec<String>> {
        let rate = self.words_per_minute.to_string();
        self.command().map(|command| {
            command
                .iter()
                .map(|word| word.replace("{words_per_minute}", &rate))
                .collect()
        })
    }

    /// Browser-safe fold that never reveals the local renderer argv.
    #[must_use]
    pub fn playback(&self) -> wire::SpeechPlayback {
        match self.backend {
            SpeechBackend::Disabled => wire::SpeechPlayback::Disabled {},
            SpeechBackend::System => wire::SpeechPlayback::System {
                voice: self.voice.clone(),
                words_per_minute: self.words_per_minute,
            },
            SpeechBackend::External => wire::SpeechPlayback::External {
                content_type: self.content_type.clone(),
                words_per_minute: self.words_per_minute,
                available: self.command().is_some(),
            },
        }
    }
}

impl Default for SpeechConfig {
    fn default() -> Self {
        Self {
            backend: SpeechBackend::System,
            command: Vec::new(),
            voice: DEFAULT_VOICE.to_owned(),
            content_type: DEFAULT_CONTENT_TYPE.to_owned(),
            words_per_minute: DEFAULT_WORDS_PER_MINUTE,
        }
    }
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SpeechInput {
    backend: SpeechBackend,
    command: Vec<String>,
    voice: String,
    content_type: String,
    words_per_minute: u64,
}

impl Default for SpeechInput {
    fn default() -> Self {
        let speech = SpeechConfig::default();
        Self {
            backend: speech.backend,
            command: speech.command,
            voice: speech.voice,
            content_type: speech.content_type,
            words_per_minute: u64::from(speech.words_per_minute),
        }
    }
}

impl<'de> Deserialize<'de> for SpeechConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let input = SpeechInput::deserialize(deserializer)?;
        Self::new(
            input.backend,
            input.command,
            input.voice,
            input.content_type,
            input.words_per_minute,
        )
        .map_err(D::Error::custom)
    }
}

fn valid_content_type(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CONTENT_TYPE_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'!' | b'#'..=b'+' | b'-'..=b'/' | b'^'..=b'~')
        })
        && value.split_once('/').is_some_and(|(kind, subtype)| {
            !kind.is_empty() && !subtype.is_empty() && !subtype.contains('/')
        })
}

/// Why one speech configuration was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SpeechConfigError {
    /// A dormant executable arm was present for a non-external backend.
    InactiveCommand {
        /// Backend that cannot consume the command.
        backend: SpeechBackend,
    },
    /// External argv contained too many words.
    CommandWords {
        /// Refused word count.
        actual: usize,
    },
    /// External argv had no executable.
    EmptyProgram,
    /// One argv word exceeded the configured byte bound.
    CommandWordBytes {
        /// Zero-based argv index.
        index: usize,
        /// Refused byte count.
        actual: usize,
    },
    /// One argv word contained a process API terminator.
    CommandNul {
        /// Zero-based argv index.
        index: usize,
    },
    /// Voice was empty or wider than the browser contract.
    VoiceBytes {
        /// Refused byte count.
        actual: usize,
    },
    /// External media type was not one bounded `type/subtype` token.
    ContentType,
    /// Base rate was zero or outside the supported range.
    WordsPerMinute {
        /// Refused value.
        actual: u64,
    },
}

impl fmt::Display for SpeechConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InactiveCommand { backend } => write!(
                formatter,
                "say.command must be empty while say.backend is {backend:?}"
            ),
            Self::CommandWords { actual } => write!(
                formatter,
                "say.command exceeds {MAX_COMMAND_WORDS} words: {actual}"
            ),
            Self::EmptyProgram => formatter.write_str("say.command executable must be nonempty"),
            Self::CommandWordBytes { index, actual } => write!(
                formatter,
                "say.command[{index}] exceeds {MAX_COMMAND_WORD_BYTES} bytes: {actual}"
            ),
            Self::CommandNul { index } => {
                write!(formatter, "say.command[{index}] contains a NUL byte")
            }
            Self::VoiceBytes { actual } => write!(
                formatter,
                "say.voice must contain 1..={MAX_VOICE_BYTES} bytes: {actual}"
            ),
            Self::ContentType => {
                formatter.write_str("say.content_type must be one bounded ASCII type/subtype token")
            }
            Self::WordsPerMinute { actual } => write!(
                formatter,
                "say.words_per_minute must be in 1..={MAX_WORDS_PER_MINUTE}: {actual}"
            ),
        }
    }
}

impl Error for SpeechConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_preserves_installed_system_contract() {
        let speech = SpeechConfig::default();
        assert_eq!(speech.backend(), SpeechBackend::System);
        assert_eq!(speech.voice(), "Zoe (Premium)");
        assert_eq!(speech.content_type(), "audio/wav");
        assert_eq!(speech.words_per_minute(), 175);
        assert!(speech.command().is_none());
        assert_eq!(
            speech.playback(),
            wire::SpeechPlayback::System {
                voice: "Zoe (Premium)".to_owned(),
                words_per_minute: 175,
            }
        );
        assert_eq!(
            wire::decode::<SpeechConfig>(&wire::encode(&speech).expect("speech encodes"))
                .expect("speech decodes"),
            speech
        );
    }

    #[test]
    fn disabled_system_and_external_are_one_strict_shape() {
        let disabled: SpeechConfig =
            toml::from_str("backend = 'disabled'").expect("disabled backend is admitted");
        assert_eq!(disabled.backend(), SpeechBackend::Disabled);

        let system: SpeechConfig = toml::from_str("voice = 'Samantha'\nwords_per_minute = 210")
            .expect("system backend is admitted");
        assert_eq!(system.voice(), "Samantha");
        assert_eq!(system.words_per_minute(), 210);

        let external: SpeechConfig = toml::from_str(concat!(
            "backend = 'external'\n",
            "command = ['espeak-ng', '--stdout', '-s', '{words_per_minute}']\n",
            "content_type = 'audio/wav'\n",
        ))
        .expect("external backend is admitted");
        assert_eq!(
            external.resolved_command().expect("configured command"),
            ["espeak-ng", "--stdout", "-s", "175"]
        );
    }

    #[test]
    fn malformed_and_dormant_execution_values_are_refused() {
        for document in [
            "backend = 'mystery'",
            "command = ['echo']",
            "backend = 'external'\ncommand = ['']",
            "voice = ''",
            "content_type = 'audio/wav\\nmalicious: true'",
            "words_per_minute = 0",
            "unknown = true",
        ] {
            assert!(
                toml::from_str::<SpeechConfig>(document).is_err(),
                "malformed speech config was admitted: {document:?}"
            );
        }
    }
}
