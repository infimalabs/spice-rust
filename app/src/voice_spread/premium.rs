//! macOS premium voice evidence, the only tier automatic spreading admits.
//!
//! Safari publishes the provider's canonical identifier as the voice URI,
//! `com.apple.voice.premium.<language>.<name>`. Chromium hides that
//! identifier, publishing the display name as the URI instead, and marks the
//! tier with the exact macOS display-name suffix ` (Premium)`. Either piece of
//! evidence admits a voice the device serves locally; enhanced, compact,
//! novelty, remote, and unmarked voices never do.

use super::OfferedVoice;

/// The provider's canonical identifier prefix for premium voices.
const PREMIUM_URI_PREFIX: &str = "com.apple.voice.premium.";
/// The exact macOS display-name marker for premium voices.
const PREMIUM_NAME_MARKER: &str = " (Premium)";

/// Whether `voice` carries macOS premium evidence.
#[must_use]
pub fn is_premium(voice: &OfferedVoice<'_>) -> bool {
    voice.local && (premium_uri(voice.uri) || premium_name(voice.name))
}

/// `com.apple.voice.premium.<language>.<name>` with both parts present.
fn premium_uri(uri: &str) -> bool {
    uri.strip_prefix(PREMIUM_URI_PREFIX)
        .and_then(|rest| rest.split_once('.'))
        .is_some_and(|(language, name)| !language.is_empty() && !name.is_empty())
}

/// A non-empty name followed by exactly ` (Premium)`.
fn premium_name(name: &str) -> bool {
    name.strip_suffix(PREMIUM_NAME_MARKER)
        .is_some_and(|base| !base.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::is_premium;
    use crate::voice_spread::OfferedVoice;

    fn voice<'a>(name: &'a str, uri: &'a str, local: bool) -> OfferedVoice<'a> {
        OfferedVoice {
            name,
            uri,
            lang: "en-US",
            local,
            default: false,
        }
    }

    #[test]
    fn safari_identifiers_admit_only_the_premium_tier() {
        assert!(is_premium(&voice(
            "Zoe",
            "com.apple.voice.premium.en-US.Zoe",
            true
        )));
        for uri in [
            "com.apple.voice.enhanced.en-US.Ava",
            "com.apple.voice.compact.en-US.Samantha",
            "com.apple.speech.synthesis.voice.Bubbles",
            "com.apple.eloquence.en-US.Eddy",
            "com.apple.voice.premium.",
            "com.apple.voice.premium.en-US",
            "com.apple.voice.premium.en-US.",
            "com.apple.voice.Premium.en-US.Zoe",
            "com.apple.voice.premiumx.en-US.Zoe",
            "x.com.apple.voice.premium.en-US.Zoe",
        ] {
            assert!(!is_premium(&voice("Zoe", uri, true)), "{uri} admitted");
        }
    }

    #[test]
    fn chromium_names_admit_only_the_exact_premium_marker() {
        assert!(is_premium(&voice("Ava (Premium)", "Ava (Premium)", true)));
        for name in [
            "Ava (Enhanced)",
            "Ava",
            "Ava Premium",
            "Ava (premium)",
            "Ava (Premium) (Enhanced)",
            "Ava(Premium)",
            " (Premium)",
            "Premium",
        ] {
            assert!(!is_premium(&voice(name, name, true)), "{name} admitted");
        }
    }

    #[test]
    fn a_remote_voice_is_never_premium() {
        assert!(!is_premium(&voice(
            "Zoe",
            "com.apple.voice.premium.en-US.Zoe",
            false
        )));
        assert!(!is_premium(&voice("Ava (Premium)", "Ava (Premium)", false)));
    }
}
