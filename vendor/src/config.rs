//! Pure admission of the landed vendor launch configurations.

use serde::de::{Deserializer, Error as _};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::Effort;

/// Claude's compiled model selection.
pub const DEFAULT_CLAUDE_MODEL: &str = "claude-opus-5";
/// Codex's compiled model selection.
pub const DEFAULT_CODEX_MODEL: &str = "gpt-5.6-sol";
/// GPT-5.6 Sol's documented full 1,050,000-token context window.
///
/// <https://developers.openai.com/api/docs/models/gpt-5.6-sol>
pub const DEFAULT_CODEX_MODEL_CONTEXT_WINDOW: u64 = 1_050_000;
/// Claude's compiled auto-compaction window.
pub const DEFAULT_CLAUDE_AUTO_COMPACT_WINDOW_TOKENS: u64 = 200_000;
/// oh-my-pi's compiled model selection, matched fuzzily by omp itself.
pub const DEFAULT_OMP_MODEL: &str = "claude-opus-5";

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
    High,
    #[default]
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
            effort: ClaudeEffort::Xhigh,
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
    #[serde(deserialize_with = "codex_context_window")]
    model_context_window: u64,
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
            model_context_window: DEFAULT_CODEX_MODEL_CONTEXT_WINDOW,
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

    /// Full context window stated on every supervised Codex launch.
    #[must_use]
    pub const fn model_context_window(&self) -> u64 {
        self.model_context_window
    }

    /// Automatic compaction threshold derived from the full context window.
    ///
    /// Dividing before multiplying makes `floor(window * 95 / 100)` exact
    /// without allowing an admitted signed-64-bit Codex value to overflow.
    #[must_use]
    pub const fn model_auto_compact_token_limit(&self) -> u64 {
        codex_auto_compact_token_limit(self.model_context_window)
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

/// Values the current oh-my-pi RPC launch dialect can honor.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct OmpConfig {
    binary: Binary,
    model: LaunchValue,
    effort: ClaudeEffort,
    fast_mode: bool,
}

impl Default for OmpConfig {
    fn default() -> Self {
        Self {
            binary: Binary::new("omp"),
            model: LaunchValue::new(DEFAULT_OMP_MODEL),
            effort: ClaudeEffort::Xhigh,
            fast_mode: false,
        }
    }
}

impl OmpConfig {
    /// Executable selected for oh-my-pi.
    #[must_use]
    pub const fn binary(&self) -> &Binary {
        &self.binary
    }

    /// Model spelling handed to omp, which resolves it against its own list.
    #[must_use]
    pub fn model(&self) -> &str {
        self.model.as_str()
    }

    /// Thinking level omp is launched with.
    #[must_use]
    pub const fn effort(&self) -> Effort {
        self.effort.launch()
    }

    /// Explicit oh-my-pi fast-mode disposition: its priority service tier.
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
    omp: OmpConfig,
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

    /// Admitted oh-my-pi launch values.
    #[must_use]
    pub const fn omp(&self) -> &OmpConfig {
        &self.omp
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

fn codex_context_window<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let value = u64::deserialize(deserializer)?;
    if value == 0 {
        return Err(D::Error::custom(
            "Codex model context window must be positive",
        ));
    }
    if value > i64::MAX as u64 {
        return Err(D::Error::custom(
            "Codex model context window must fit Codex's signed 64-bit configuration",
        ));
    }
    if codex_auto_compact_token_limit(value) == 0 {
        return Err(D::Error::custom(
            "Codex model context window must yield a positive 95% auto-compaction limit",
        ));
    }
    Ok(value)
}

/// Derive Codex's automatic compaction threshold without overflowing.
#[must_use]
pub const fn codex_auto_compact_token_limit(context_window: u64) -> u64 {
    let hundreds = context_window / 100;
    let remainder = context_window % 100;
    hundreds * 95 + remainder * 95 / 100
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codex(model_context_window: u64) -> CodexConfig {
        serde_json::from_value(serde_json::json!({
            "model_context_window": model_context_window,
        }))
        .expect("Codex configuration admits")
    }

    #[test]
    fn codex_compaction_limit_is_exact_for_non_round_and_boundary_windows() {
        let non_round = codex(101);
        assert_eq!(non_round.model_auto_compact_token_limit(), 95);

        let boundary = codex(i64::MAX as u64);
        assert_eq!(
            boundary.model_auto_compact_token_limit(),
            8_762_203_435_012_037_016
        );
        assert!(boundary.model_auto_compact_token_limit() < boundary.model_context_window());
    }
}
