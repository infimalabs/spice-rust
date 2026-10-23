//! macOS voice tier evidence: premium, the tier automatic spreading prefers,
//! and enhanced, the tier it accepts next.
//!
//! Safari publishes the provider's canonical identifier as the voice URI,
//! `com.apple.voice.premium.<language>.<name>` or
//! `com.apple.voice.enhanced.<language>.<name>`. Chromium hides that
//! identifier, publishing the display name as the URI instead, and marks the
//! tier with the exact macOS display-name suffix ` (Premium)` or
//! ` (Enhanced)`. Either piece of evidence admits a voice the device serves
//! locally; compact, novelty, remote, and unmarked voices carry neither tier.

use super::OfferedVoice;

/// The provider's canonical identifier prefix for premium voices.
const PREMIUM_URI_PREFIX: &str = "com.apple.voice.premium.";
/// The exact macOS display-name marker for premium voices.
const PREMIUM_NAME_MARKER: &str = " (Premium)";
/// The provider's canonical identifier prefix for enhanced voices.
const ENHANCED_URI_PREFIX: &str = "com.apple.voice.enhanced.";
/// The exact macOS display-name marker for enhanced voices.
const ENHANCED_NAME_MARKER: &str = " (Enhanced)";

/// Whether `voice` carries macOS premium evidence.
#[must_use]
pub fn is_premium(voice: &OfferedVoice<'_>) -> bool {
    voice.local
        && (tier_uri(voice.uri, PREMIUM_URI_PREFIX) || tier_name(voice.name, PREMIUM_NAME_MARKER))
}

/// Whether `voice` carries macOS enhanced evidence.
#[must_use]
pub fn is_enhanced(voice: &OfferedVoice<'_>) -> bool {
    voice.local
        && (tier_uri(voice.uri, ENHANCED_URI_PREFIX) || tier_name(voice.name, ENHANCED_NAME_MARKER))
}

/// `<prefix><language>.<name>` with both parts present.
fn tier_uri(uri: &str, prefix: &str) -> bool {
    uri.strip_prefix(prefix)
        .and_then(|rest| rest.split_once('.'))
        .is_some_and(|(language, name)| !language.is_empty() && !name.is_empty())
}

/// A non-empty name followed by exactly `marker`.
fn tier_name(name: &str, marker: &str) -> bool {
    name.strip_suffix(marker)
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
