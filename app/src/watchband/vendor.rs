//! Vendor identity a composer paints: the emblem class, and for oh-my-pi the
//! provider family its model runs on.

/// Browser composer emblem class for a vendor with a known visual identity.
#[must_use]
pub fn composer_vendor_icon_class(vendor: &str) -> Option<&'static str> {
    match vendor {
        "claude" => Some("composer-vendor-icon--claude"),
        "codex" | "openai" => Some("composer-vendor-icon--openai"),
        "omp" => Some("composer-vendor-icon--omp"),
        _ => None,
    }
}

/// Provider family behind an oh-my-pi model, named by the vendor it badges.
///
/// omp names a model as `provider/model` or by the bare id it resolves
/// itself; both forms carry the family, and any other family stays unbadged.
#[must_use]
pub fn omp_provider(model: &str) -> Option<&'static str> {
    match model.split_once('/') {
        Some(("anthropic", _)) => Some("claude"),
        Some(("openai" | "openai-codex", _)) => Some("openai"),
        Some(_) => None,
        None if model.starts_with("claude") => Some("claude"),
        None if model.starts_with("gpt-") || model.starts_with("codex") => Some("openai"),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{composer_vendor_icon_class, omp_provider};

    #[test]
    fn composer_icons_admit_only_known_vendor_identities() {
        for vendor in ["claude", "codex", "openai", "omp"] {
            assert!(composer_vendor_icon_class(vendor).is_some(), "{vendor}");
        }
        for vendor in ["", "agent", "gemini", "Claude", "open-ai"] {
            assert_eq!(composer_vendor_icon_class(vendor), None, "{vendor}");
        }
    }

    #[test]
    fn omp_badges_the_provider_family_from_either_model_spelling() {
        for model in [
            "anthropic/claude-opus-5",
            "claude-opus-5",
            "claude-sonnet-5",
        ] {
            assert_eq!(omp_provider(model), Some("claude"), "{model}");
        }
        for model in [
            "openai/gpt-5.3",
            "openai-codex/gpt-5.3-codex",
            "gpt-5.3-codex",
            "codex-mini-latest",
        ] {
            assert_eq!(omp_provider(model), Some("openai"), "{model}");
        }
        for model in [
            "",
            "google/gemini-3-pro",
            "gemini-3-pro",
            "Claude-opus-5",
            "/claude",
        ] {
            assert_eq!(omp_provider(model), None, "{model}");
        }
    }
}
