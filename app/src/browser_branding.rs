//! Product-branded browser title and favicon state.
//!
//! The icon keeps Canvas's pepper product mark and confines changing state to
//! one lower-right badge. Counts are exact through 99; larger values use `99+`
//! in the tiny icon while the title retains the exact total.

const PRODUCT_TITLE: &str = "Spice Canvas";
const CURRENT_BADGE: &str = "#68c89d";
const STALE_BADGE: &str = "#e8b84e";
const DISCONNECTED_BADGE: &str = "#ef7358";
const LOADING_BADGE: &str = "#9aa29b";
const MAX_EXACT_BADGE_COUNT: usize = 99;
const SINGLE_CHARACTER: usize = 1;
const DOUBLE_CHARACTERS: usize = 2;
const SINGLE_CHARACTER_FONT_SIZE: usize = 18;
const DOUBLE_CHARACTER_FONT_SIZE: usize = 15;
const COMPACT_FONT_SIZE: usize = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConnectionState {
    Starting,
    Connecting,
    Online,
    Offline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CountState {
    Loading,
    Current(usize),
    Stale(usize),
    Disconnected,
}

impl CountState {
    #[must_use]
    pub(crate) const fn resolve(connection: ConnectionState, count: Option<usize>) -> Self {
        match (connection, count) {
            (ConnectionState::Online, Some(count)) => Self::Current(count),
            (ConnectionState::Online, None)
            | (ConnectionState::Starting, None)
            | (ConnectionState::Connecting, None) => Self::Loading,
            (ConnectionState::Offline, None) => Self::Disconnected,
            (
                ConnectionState::Starting | ConnectionState::Connecting | ConnectionState::Offline,
                Some(count),
            ) => Self::Stale(count),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BrowserBranding {
    title: String,
    icon: String,
}

impl BrowserBranding {
    #[must_use]
    pub(crate) fn for_state(state: CountState) -> Self {
        let (title, badge, color) = match state {
            CountState::Loading => (
                format!("{PRODUCT_TITLE} — Loading"),
                "…".to_owned(),
                LOADING_BADGE,
            ),
            CountState::Disconnected => (
                format!("{PRODUCT_TITLE} — Disconnected"),
                "!".to_owned(),
                DISCONNECTED_BADGE,
            ),
            CountState::Current(count) => (
                remaining_title(count, false),
                badge_count(count),
                CURRENT_BADGE,
            ),
            CountState::Stale(count) => (
                remaining_title(count, true),
                badge_count(count),
                STALE_BADGE,
            ),
        };
        Self {
            title,
            icon: icon_svg(&badge, color),
        }
    }

    #[must_use]
    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub(crate) fn icon(&self) -> &str {
        &self.icon
    }
}

fn remaining_title(count: usize, stale: bool) -> String {
    let noun = if count == 1 { "task" } else { "tasks" };
    let suffix = if stale { " (stale)" } else { "" };
    format!("{PRODUCT_TITLE} — {count} {noun} remaining{suffix}")
}

fn badge_count(count: usize) -> String {
    if count > MAX_EXACT_BADGE_COUNT {
        format!("{MAX_EXACT_BADGE_COUNT}+")
    } else {
        count.to_string()
    }
}

fn icon_svg(label: &str, badge: &str) -> String {
    let font_size = if label.len() <= SINGLE_CHARACTER {
        SINGLE_CHARACTER_FONT_SIZE
    } else if label.len() == DOUBLE_CHARACTERS {
        DOUBLE_CHARACTER_FONT_SIZE
    } else {
        COMPACT_FONT_SIZE
    };
    format!(
        concat!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 64 64\">",
            "<rect width=\"64\" height=\"64\" rx=\"14\" fill=\"#101411\"/>",
            "<text x=\"25\" y=\"40\" text-anchor=\"middle\" font-family=\"Apple Color Emoji,Segoe UI Emoji,sans-serif\" font-size=\"34\">🌶️</text>",
            "<rect x=\"28\" y=\"32\" width=\"34\" height=\"30\" rx=\"15\" fill=\"{badge}\" stroke=\"#101411\" stroke-width=\"3\"/>",
            "<text x=\"45\" y=\"52\" text-anchor=\"middle\" font-family=\"ui-monospace,monospace\" font-size=\"{font_size}\" font-weight=\"800\" fill=\"#101411\">{label}</text>",
            "</svg>"
        ),
        badge = badge,
        font_size = font_size,
        label = label
    )
}

#[cfg(target_arch = "wasm32")]
#[yew::hook]
pub(crate) fn use_browser_branding(state: CountState) {
    use yew::prelude::use_effect_with;

    let branding = BrowserBranding::for_state(state);
    use_effect_with(branding, |branding| {
        apply(branding);
    });
}

#[cfg(target_arch = "wasm32")]
fn apply(branding: &BrowserBranding) {
    use base64::Engine as _;

    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    document.set_title(branding.title());
    let icon = document
        .query_selector("link[data-spice-canvas-icon]")
        .ok()
        .flatten()
        .or_else(|| {
            let icon = document.create_element("link").ok()?;
            icon.set_attribute("data-spice-canvas-icon", "").ok()?;
            document
                .query_selector("head")
                .ok()
                .flatten()?
                .append_child(&icon)
                .ok()?;
            Some(icon)
        });
    let Some(icon) = icon else {
        return;
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(branding.icon().as_bytes());
    let _ = icon.set_attribute("rel", "icon");
    let _ = icon.set_attribute("type", "image/svg+xml");
    let _ = icon.set_attribute("href", &format!("data:image/svg+xml;base64,{encoded}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    const KNOWN_COUNT: usize = 7;
    const LARGE_TITLE_COUNT: usize = 1_234;

    #[test]
    fn connection_and_snapshot_state_define_freshness() {
        assert_eq!(
            CountState::resolve(ConnectionState::Starting, None),
            CountState::Loading
        );
        assert_eq!(
            CountState::resolve(ConnectionState::Connecting, Some(KNOWN_COUNT)),
            CountState::Stale(KNOWN_COUNT)
        );
        assert_eq!(
            CountState::resolve(ConnectionState::Online, Some(KNOWN_COUNT)),
            CountState::Current(KNOWN_COUNT)
        );
        assert_eq!(
            CountState::resolve(ConnectionState::Offline, Some(KNOWN_COUNT)),
            CountState::Stale(KNOWN_COUNT)
        );
        assert_eq!(
            CountState::resolve(ConnectionState::Offline, None),
            CountState::Disconnected
        );
    }

    #[test]
    fn titles_keep_product_branding_and_exact_count_state() {
        for (state, expected) in [
            (CountState::Loading, "Spice Canvas — Loading"),
            (CountState::Disconnected, "Spice Canvas — Disconnected"),
            (CountState::Current(0), "Spice Canvas — 0 tasks remaining"),
            (CountState::Current(1), "Spice Canvas — 1 task remaining"),
            (
                CountState::Current(LARGE_TITLE_COUNT),
                "Spice Canvas — 1234 tasks remaining",
            ),
            (
                CountState::Stale(KNOWN_COUNT),
                "Spice Canvas — 7 tasks remaining (stale)",
            ),
        ] {
            assert_eq!(BrowserBranding::for_state(state).title(), expected);
        }
    }

    #[test]
    fn icon_badges_are_distinct_and_bound_large_counts() {
        let zero = BrowserBranding::for_state(CountState::Current(0));
        let one = BrowserBranding::for_state(CountState::Current(1));
        let large = BrowserBranding::for_state(CountState::Current(MAX_EXACT_BADGE_COUNT + 1));
        let stale = BrowserBranding::for_state(CountState::Stale(1));
        let disconnected = BrowserBranding::for_state(CountState::Disconnected);

        assert!(zero.icon().contains(">0</text>"));
        assert!(zero.icon().contains("🌶️"));
        assert!(one.icon().contains(">1</text>"));
        assert!(large.icon().contains(">99+</text>"));
        assert!(!large.icon().contains(">100</text>"));
        assert_ne!(one.icon(), stale.icon());
        assert!(disconnected.icon().contains(">!</text>"));
        assert!(one.icon().contains("#68c89d"));
    }
}
