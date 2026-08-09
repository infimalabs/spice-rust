//! Pure admission of the two landed vendor launch configurations.

use serde::de::{Deserializer, Error as _};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::Effort;

/// Claude's compiled model selection.
pub const DEFAULT_CLAUDE_MODEL: &str = "sonnet";
/// Codex's compiled model selection.
pub const DEFAULT_CODEX_MODEL: &str = "gpt-5.3-codex-spark";
/// Claude's compiled auto-compaction window.
pub const DEFAULT_CLAUDE_AUTO_COMPACT_WINDOW_TOKENS: u64 = 200_000;

/// A configured executable spelling.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Binary(String);

impl Binary {
    /// Exact executable spelling supplied by tracked policy.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn new(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl<'de> Deserialize<'de> for Binary {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        admitted_string(deserializer, "vendor binary").map(Self)
    }
}

impl Serialize for Binary {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LaunchValue(String);

impl LaunchValue {
    fn as_str(&self) -> &str {
        &self.0
    }

    fn new(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl<'de> Deserialize<'de> for LaunchValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        admitted_string(deserializer, "vendor launch value").map(Self)
    }
}

impl Serialize for LaunchValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

fn admitted_string<'de, D: Deserializer<'de>>(
    deserializer: D,
    label: &str,
) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.trim().is_empty() {
        return Err(D::Error::custom(format!("{label} must not be empty")));
    }
    if value.contains('\0') {
        return Err(D::Error::custom(format!("{label} must not contain NUL")));
    }
    Ok(value)
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum ClaudeEffort {
    Low,
    Medium,
    #[default]
    High,
    Xhigh,
    Max,
}

impl ClaudeEffort {
    const fn launch(self) -> Effort {
        match self {
            Self::Low => Effort::Low,
            Self::Medium => Effort::Medium,
            Self::High => Effort::High,
            Self::Xhigh => Effort::XHigh,
            Self::Max => Effort::Max,
        }
    }
}

/// Values the current Claude launch dialect can honor.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClaudeConfig {
    binary: Binary,
    model: LaunchValue,
    effort: ClaudeEffort,
    #[serde(deserialize_with = "positive_window")]
    auto_compact_window_tokens: u64,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            binary: Binary::new("claude"),
            model: LaunchValue::new(DEFAULT_CLAUDE_MODEL),
            effort: ClaudeEffort::High,
            auto_compact_window_tokens: DEFAULT_CLAUDE_AUTO_COMPACT_WINDOW_TOKENS,
        }
    }
}

impl ClaudeConfig {
    /// Executable selected for Claude.
    #[must_use]
    pub const fn binary(&self) -> &Binary {
        &self.binary
    }

    /// Concrete Claude model spelling.
    #[must_use]
    pub fn model(&self) -> &str {
        self.model.as_str()
    }

    /// Claude-supported reasoning effort.
    #[must_use]
    pub const fn effort(&self) -> Effort {
        self.effort.launch()
    }

    /// Positive child auto-compaction window.
    #[must_use]
    pub const fn auto_compact_window_tokens(&self) -> u64 {
        self.auto_compact_window_tokens
    }
}

/// Values the current Codex launch dialect can honor.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CodexConfig {
    binary: Binary,
    model: LaunchValue,
    effort: Effort,
    #[serde(skip_serializing_if = "Option::is_none")]
    personality: Option<LaunchValue>,
    fast_mode: bool,
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            binary: Binary::new("codex"),
            model: LaunchValue::new(DEFAULT_CODEX_MODEL),
            effort: Effort::High,
            personality: None,
            fast_mode: false,
        }
    }
}

impl CodexConfig {
    /// Executable selected for Codex.
    #[must_use]
    pub const fn binary(&self) -> &Binary {
        &self.binary
    }

    /// Concrete Codex model spelling.
    #[must_use]
    pub fn model(&self) -> &str {
        self.model.as_str()
    }

    /// Codex reasoning effort.
    #[must_use]
    pub const fn effort(&self) -> Effort {
        self.effort
    }

    /// Optional Codex personality spelling.
    #[must_use]
    pub fn personality(&self) -> Option<&str> {
        self.personality.as_ref().map(LaunchValue::as_str)
    }

    /// Explicit Codex fast-mode disposition.
    #[must_use]
    pub const fn fast_mode(&self) -> bool {
        self.fast_mode
    }
}

/// The closed set of vendor configurations with landed Rust renderers.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    claude: ClaudeConfig,
    codex: CodexConfig,
}

impl Config {
    /// Admitted Claude launch values.
    #[must_use]
    pub const fn claude(&self) -> &ClaudeConfig {
        &self.claude
    }

    /// Admitted Codex launch values.
    #[must_use]
    pub const fn codex(&self) -> &CodexConfig {
        &self.codex
    }
}

fn positive_window<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let value = u64::deserialize(deserializer)?;
    if value == 0 {
        Err(D::Error::custom(
            "Claude auto-compaction window must be positive",
        ))
    } else {
        Ok(value)
    }
}
