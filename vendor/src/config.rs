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
/// oh-my-pi's compiled model selection, matched fuzzily by omp itself.
pub const DEFAULT_OMP_MODEL: &str = "claude-opus-5";
/// Input tokens beyond which Anthropic bills a request at long-context rates.
const ANTHROPIC_LONG_CONTEXT_TOKENS: u64 = 200_000;
/// Input tokens beyond which OpenAI bills a request at long-context rates.
const OPENAI_LONG_CONTEXT_TOKENS: u64 = 272_000;
/// The least Claude has been seen to compact short of the window it is told,
/// rounded down: 69 lane compactions against 200,000 landed between 166,418
/// and 175,600 tokens.
const CLAUDE_LEAST_SHORTFALL_TOKENS: u64 = 20_000;
/// The window Claude is asked to compact against.
///
/// Every vendor is told a window smaller than its model's so that no request
/// crosses the provider's long-context price boundary, whatever the model
/// really holds. Claude's trigger lands under Anthropic's boundary with the
/// rounding and the last exchange's growth to spare.
pub const DEFAULT_CLAUDE_AUTO_COMPACT_WINDOW_TOKENS: u64 =
    ANTHROPIC_LONG_CONTEXT_TOKENS + CLAUDE_LEAST_SHORTFALL_TOKENS;
/// The window Codex is asked to compact against: OpenAI's boundary itself,
/// since Codex triggers at 95% of it.
pub const DEFAULT_CODEX_AUTO_COMPACT_WINDOW_TOKENS: u64 = OPENAI_LONG_CONTEXT_TOKENS;
/// The window oh-my-pi is asked to compact against for an Anthropic model,
/// the kind it selects by default; omp triggers 15% short of it. Left to
/// itself omp compacts against the model's full window, 850,000 tokens into
/// a million-token model, and its registry prices no Anthropic long context.
pub const DEFAULT_OMP_AUTO_COMPACT_WINDOW_TOKENS: u64 = DEFAULT_CLAUDE_AUTO_COMPACT_WINDOW_TOKENS;
/// The window oh-my-pi is asked to compact against for an OpenAI model:
/// OpenAI's boundary itself, the window omp's own registry declares for the
/// Codex-served GPT models whose long-context price it does know.
pub const DEFAULT_OMP_OPENAI_AUTO_COMPACT_WINDOW_TOKENS: u64 = OPENAI_LONG_CONTEXT_TOKENS;
/// Provider prefixes omp resolves ahead of the slash in a `provider/model`
/// spelling, and bare model families it matches fuzzily, that reach OpenAI.
const OMP_OPENAI_PROVIDERS: [&str; 2] = ["openai", "openai-codex"];
const OMP_OPENAI_FAMILIES: [&str; 2] = ["gpt", "codex"];
/// The share of a window omp holds back for the reply, as omp computes it.
const OMP_RESERVE_PERCENT: u64 = 15;
/// The least reserve omp holds back, whatever the window.
const OMP_RESERVE_FLOOR_TOKENS: u64 = 16_384;

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
    #[serde(deserialize_with = "codex_window")]
    auto_compact_window_tokens: u64,
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
            auto_compact_window_tokens: DEFAULT_CODEX_AUTO_COMPACT_WINDOW_TOKENS,
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

    /// The window Codex is asked to compact against.
    #[must_use]
    pub const fn auto_compact_window_tokens(&self) -> u64 {
        self.auto_compact_window_tokens
    }

    /// Automatic compaction threshold: Codex's own 95% of the window it is
    /// asked to compact against, not of the model's full context window.
    ///
    /// Dividing before multiplying makes `floor(window * 95 / 100)` exact
    /// without allowing an admitted signed-64-bit Codex value to overflow.
    #[must_use]
    pub const fn model_auto_compact_token_limit(&self) -> u64 {
        codex_auto_compact_token_limit(self.auto_compact_window_tokens)
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
    #[serde(
        deserialize_with = "omp_window",
        skip_serializing_if = "Option::is_none"
    )]
    auto_compact_window_tokens: Option<u64>,
}

impl Default for OmpConfig {
    fn default() -> Self {
        Self {
            binary: Binary::new("omp"),
            model: LaunchValue::new(DEFAULT_OMP_MODEL),
            effort: ClaudeEffort::Xhigh,
            fast_mode: false,
            auto_compact_window_tokens: None,
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

    /// The window omp is asked to compact against: the one policy names, or
    /// else the boundary of the provider the configured model reaches.
    #[must_use]
    pub fn auto_compact_window_tokens(&self) -> u64 {
        self.auto_compact_window_tokens_for(self.model())
    }

    /// The window omp is asked to compact against when a seat selected
    /// `model` over the configured one: the window policy names still wins,
    /// and otherwise the boundary follows the model the seat really runs.
    #[must_use]
    pub fn auto_compact_window_tokens_for(&self, model: &str) -> u64 {
        self.auto_compact_window_tokens
            .unwrap_or_else(|| omp_default_auto_compact_window_tokens(model))
    }

    /// The fixed token limit omp is handed, derived from the window the way
    /// omp derives its own threshold from a model's window.
    #[must_use]
    pub fn auto_compact_threshold_tokens(&self) -> u64 {
        omp_auto_compact_threshold_tokens(self.auto_compact_window_tokens())
    }

    /// [`Self::auto_compact_threshold_tokens`] for the model a seat selected.
    #[must_use]
    pub fn auto_compact_threshold_tokens_for(&self, model: &str) -> u64 {
        omp_auto_compact_threshold_tokens(self.auto_compact_window_tokens_for(model))
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
    Ok(value)
}

fn codex_window<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let value = u64::deserialize(deserializer)?;
    if value > i64::MAX as u64 {
        return Err(D::Error::custom(
            "Codex auto-compaction window must fit Codex's signed 64-bit configuration",
        ));
    }
    if codex_auto_compact_token_limit(value) == 0 {
        return Err(D::Error::custom(
            "Codex auto-compaction window must yield a positive 95% auto-compaction limit",
        ));
    }
    Ok(value)
}

fn omp_window<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    let value = Option::<u64>::deserialize(deserializer)?;
    if value.is_some_and(|window| omp_auto_compact_threshold_tokens(window) == 0) {
        return Err(D::Error::custom(
            "oh-my-pi auto-compaction window must exceed the reserve omp holds back",
        ));
    }
    Ok(value)
}

/// The window omp is asked to compact against when policy names none: the
/// boundary of the provider its fuzzy match reaches from this spelling, or
/// Anthropic's smaller one for a spelling that names no provider spice knows,
/// so an unfamiliar model crosses neither boundary.
#[must_use]
pub fn omp_default_auto_compact_window_tokens(model: &str) -> u64 {
    let spelling = model.trim().to_ascii_lowercase();
    let openai = match spelling.split_once('/') {
        Some((provider, _)) => OMP_OPENAI_PROVIDERS.contains(&provider),
        None => OMP_OPENAI_FAMILIES
            .iter()
            .any(|family| spelling.starts_with(family)),
    };
    if openai {
        DEFAULT_OMP_OPENAI_AUTO_COMPACT_WINDOW_TOKENS
    } else {
        DEFAULT_OMP_AUTO_COMPACT_WINDOW_TOKENS
    }
}

/// Derive the threshold omp would compute for a window of this size: the
/// window less the larger of its proportional and floor reserves.
#[must_use]
pub const fn omp_auto_compact_threshold_tokens(window: u64) -> u64 {
    let hundreds = window / 100;
    let remainder = window % 100;
    let proportional = hundreds * OMP_RESERVE_PERCENT + remainder * OMP_RESERVE_PERCENT / 100;
    let reserve = if proportional > OMP_RESERVE_FLOOR_TOKENS {
        proportional
    } else {
        OMP_RESERVE_FLOOR_TOKENS
    };
    window.saturating_sub(reserve)
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

    fn codex(auto_compact_window_tokens: u64) -> CodexConfig {
        serde_json::from_value(serde_json::json!({
            "auto_compact_window_tokens": auto_compact_window_tokens,
        }))
        .expect("Codex configuration admits")
    }

    fn omp(auto_compact_window_tokens: u64) -> Result<OmpConfig, serde_json::Error> {
        serde_json::from_value(serde_json::json!({
            "auto_compact_window_tokens": auto_compact_window_tokens,
        }))
    }

    #[test]
    fn omp_threshold_is_the_window_less_the_larger_reserve() {
        // The floor wins for windows under roughly 109,000 tokens.
        assert_eq!(omp_auto_compact_threshold_tokens(200_000), 170_000);
        assert_eq!(omp_auto_compact_threshold_tokens(1_000_000), 850_000);
        assert_eq!(omp_auto_compact_threshold_tokens(100_000), 83_616);
        assert_eq!(omp_auto_compact_threshold_tokens(16_385), 1);
        assert_eq!(omp_auto_compact_threshold_tokens(16_384), 0);
        assert_eq!(omp_auto_compact_threshold_tokens(0), 0);
        assert_eq!(
            omp_auto_compact_threshold_tokens(u64::MAX),
            u64::MAX - u64::MAX / 100 * 15 - u64::MAX % 100 * 15 / 100
        );
        assert_eq!(
            omp(16_385)
                .expect("one usable token admits")
                .auto_compact_threshold_tokens(),
            1
        );
        assert!(omp(16_384).is_err());
        assert_eq!(
            OmpConfig::default().auto_compact_threshold_tokens(),
            187_000
        );
    }

    #[test]
    fn omp_window_follows_the_provider_the_model_spelling_reaches() {
        for spelling in [
            "gpt-5.6-sol",
            "GPT-5.2",
            "codex-mini",
            "openai/gpt-5.2",
            "openai-codex/gpt-5.6-sol",
            "openai/anything",
        ] {
            assert_eq!(
                omp_default_auto_compact_window_tokens(spelling),
                272_000,
                "{spelling}"
            );
        }
        for spelling in [
            "claude-opus-5",
            "opus",
            "anthropic/claude-sonnet-5",
            "gemini-2.5-pro",
            "google/gemini-2.5-pro",
            "",
        ] {
            assert_eq!(
                omp_default_auto_compact_window_tokens(spelling),
                220_000,
                "{spelling}"
            );
        }
        let sol: OmpConfig = serde_json::from_value(serde_json::json!({"model": "gpt-5.6-sol"}))
            .expect("an OpenAI model admits");
        assert_eq!(sol.auto_compact_window_tokens(), 272_000);
        assert_eq!(sol.auto_compact_threshold_tokens(), 231_200);
        assert_eq!(
            sol.auto_compact_threshold_tokens_for("claude-opus-5"),
            187_000,
            "a seat's own model selection governs its window"
        );
        assert_eq!(
            OmpConfig::default().auto_compact_threshold_tokens_for("gpt-5.6-sol"),
            231_200
        );
        let pinned: OmpConfig = serde_json::from_value(
            serde_json::json!({"model": "gpt-5.6-sol", "auto_compact_window_tokens": 200_000}),
        )
        .expect("a named window admits");
        assert_eq!(pinned.auto_compact_window_tokens(), 200_000);
        assert_eq!(
            pinned.auto_compact_threshold_tokens_for("claude-opus-5"),
            170_000,
            "a named window wins over every model selection"
        );
        let unset: OmpConfig =
            serde_json::from_value(serde_json::json!({"auto_compact_window_tokens": null}))
                .expect("a null window admits");
        assert_eq!(unset.auto_compact_window_tokens(), 220_000);
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
        assert!(boundary.model_auto_compact_token_limit() < boundary.auto_compact_window_tokens());
        assert_eq!(
            CodexConfig::default().model_auto_compact_token_limit(),
            258_400
        );
    }
}
