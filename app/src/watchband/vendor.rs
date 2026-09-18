//! Vendor identity a composer paints: the emblem class, and for oh-my-pi the
//! provider family its model runs on and which mark that family leaves to the
//! badge.

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

/// Provider family behind an oh-my-pi model, named by the vendor whose glyph
/// it wears.
///
/// omp names a model as `provider/model` or by the bare id it resolves
/// itself; both forms carry the family, and any other family stays unnamed.
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

/// What an oh-my-pi composer paints in its emblem position.
///
/// The emblem says which model and architecture answers, not which harness
/// drives it. A model of a known provider family therefore wears that
/// family's glyph as the emblem, exactly as a native lane of that vendor
/// does, and oh-my-pi's pi shrinks to the badge over the glyph's corner. A
/// model of no known family, or a seat that recorded none, keeps the pi as
/// its emblem with no badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OmpEmblem {
    /// The named provider family's glyph, badged with oh-my-pi's pi.
    Provider(&'static str),
    /// oh-my-pi's pi alone.
    Pi,
}

/// Emblem roles for an oh-my-pi composer, from the model its seat recorded.
#[must_use]
pub fn omp_emblem(model: Option<&str>) -> OmpEmblem {
    model
        .and_then(omp_provider)
        .map_or(OmpEmblem::Pi, OmpEmblem::Provider)
}

#[cfg(test)]
mod tests {
    use super::{OmpEmblem, composer_vendor_icon_class, omp_emblem, omp_provider};

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
    fn omp_names_the_provider_family_from_either_model_spelling() {
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

    #[test]
    fn omp_emblem_is_the_provider_glyph_badged_with_the_pi_or_the_pi_alone() {
        assert_eq!(
            omp_emblem(Some("anthropic/claude-opus-5")),
            OmpEmblem::Provider("claude")
        );
        assert_eq!(
            omp_emblem(Some("claude-sonnet-5")),
            OmpEmblem::Provider("claude")
        );
        assert_eq!(
            omp_emblem(Some("openai/gpt-5.3")),
            OmpEmblem::Provider("openai")
        );
        assert_eq!(
            omp_emblem(Some("codex-mini-latest")),
            OmpEmblem::Provider("openai")
        );
        for model in [
            Some("google/gemini-3-pro"),
            Some("gemini-3-pro"),
            Some(""),
            None,
        ] {
            assert_eq!(omp_emblem(model), OmpEmblem::Pi, "{model:?}");
        }
    }
}
