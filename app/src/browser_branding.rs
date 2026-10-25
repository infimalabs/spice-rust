//! Product-branded browser title and favicon state.
//!
//! The icon keeps the spice pepper mark in its upper-left corner and
//! gives the changing state one large lower-right badge, so a count stays
//! readable at the 16px tab size. Counts are exact through 99; larger values
//! use `99+` in the tiny icon while the title retains the exact total.

/// The predecessor's brand: short, lowercase, and last, so the tab's few visible
/// characters carry the changing state first.
const BRAND: &str = "spice";
const MAX_EXACT_BADGE_COUNT: usize = 99;
/// Badge rectangle width in icon units; every label must fit inside it.
const BADGE_WIDTH: usize = 49;
/// Font size and baseline in icon units for one-, two-, and three-character labels.
const LABEL_METRICS: [(usize, usize); 3] = [(42, 57), (34, 54), (24, 51)];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConnectionState {
    Starting,
    Connecting,
    Online,
    Offline,
}

/// The journal cursor the connection label's hover names while that connection
/// proves the retained app runtime is current.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JournalCursor {
    pub(crate) value: Option<String>,
    pub(crate) accessible: String,
}

impl JournalCursor {
    #[must_use]
    pub(crate) fn resolve(connection: ConnectionState, cursor: Option<wire::Seq>) -> Option<Self> {
        if connection != ConnectionState::Online {
            return None;
        }
        let value = cursor.map(|cursor| cursor.to_string());
        Some(Self {
            accessible: value.as_ref().map_or_else(
                || "journal cursor not yet admitted".to_owned(),
                |cursor| format!("journal cursor {cursor}"),
            ),
            value,
        })
    }
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

/// Badge colour: green current, amber stale, red disconnected, grey loading.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tone {
    Loading,
    Current,
    Stale,
    Disconnected,
}

impl Tone {
    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    const fn color(self) -> &'static str {
        match self {
            Self::Loading => "#9aa29b",
            // The dark theme's standard `--accent` green.
            Self::Current => "#74c69d",
            Self::Stale => "#e8b84e",
            Self::Disconnected => "#ef7358",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BrowserBranding {
    title: String,
    badge: String,
    tone: Tone,
}

impl BrowserBranding {
    #[must_use]
    pub(crate) fn for_state(state: CountState) -> Self {
        let (title, badge, tone) = match state {
            CountState::Loading => (format!("Loading - {BRAND}"), "…".to_owned(), Tone::Loading),
            CountState::Disconnected => (
                format!("Disconnected - {BRAND}"),
                "!".to_owned(),
                Tone::Disconnected,
            ),
            CountState::Current(count) => (
                remaining_title(count, false),
                badge_count(count),
                Tone::Current,
            ),
            CountState::Stale(count) => (
                remaining_title(count, true),
                badge_count(count),
                Tone::Stale,
            ),
        };
        Self { title, badge, tone }
    }

    #[must_use]
    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    /// Standalone SVG document for the favicon.
    #[cfg(any(target_arch = "wasm32", rust_analyzer))]
    #[must_use]
    pub(crate) fn icon(&self) -> String {
        let (font_size, baseline) = label_metrics(&self.badge);
        format!(
            concat!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 64 64\">",
                "<rect width=\"64\" height=\"64\" rx=\"14\" fill=\"#101411\"/>",
                "<text x=\"16\" y=\"29\" text-anchor=\"middle\" font-family=\"Apple Color Emoji,Segoe UI Emoji,sans-serif\" font-size=\"28\">🌶️</text>",
                "<rect x=\"13.5\" y=\"21.5\" width=\"{width}\" height=\"41\" rx=\"13\" fill=\"{color}\" stroke=\"#101411\" stroke-width=\"3\"/>",
                "<text x=\"38\" y=\"{baseline}\" text-anchor=\"middle\" font-family=\"ui-monospace,SFMono-Regular,Menlo,monospace\" font-size=\"{font_size}\" font-weight=\"800\" fill=\"#101411\">{label}</text>",
                "</svg>"
            ),
            width = BADGE_WIDTH,
            color = self.tone.color(),
            baseline = baseline,
            font_size = font_size,
            label = self.badge
        )
    }
}

fn remaining_title(count: usize, stale: bool) -> String {
    let noun = if count == 1 { "task" } else { "tasks" };
    let suffix = if stale { " (stale)" } else { "" };
    format!("{count} {noun}{suffix} - {BRAND}")
}

fn badge_count(count: usize) -> String {
    if count > MAX_EXACT_BADGE_COUNT {
        format!("{MAX_EXACT_BADGE_COUNT}+")
    } else {
        count.to_string()
    }
}

/// Largest font and its baseline for a badge label of this many characters.
fn label_metrics(label: &str) -> (usize, usize) {
    let characters = label.chars().count().clamp(1, LABEL_METRICS.len());
    LABEL_METRICS[characters - 1]
}

#[cfg(any(target_arch = "wasm32", rust_analyzer))]
#[yew::hook]
pub(crate) fn use_browser_branding(state: CountState) {
    use yew::prelude::use_effect_with;

    use_effect_with(state, |state| apply(&BrowserBranding::for_state(*state)));
}

#[cfg(any(target_arch = "wasm32", rust_analyzer))]
fn apply(branding: &BrowserBranding) {
    use base64::Engine as _;

    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    document.set_title(branding.title());
    let icon = document
        .query_selector("link[data-spice-app-icon]")
        .ok()
        .flatten()
        .or_else(|| {
            let icon = document.create_element("link").ok()?;
            icon.set_attribute("data-spice-app-icon", "").ok()?;
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
    /// Monospace advance as a percentage of the font size.
    const MONOSPACE_ADVANCE_PERCENT: usize = 60;
    const PERCENT: usize = 100;

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
    fn journal_cursor_is_current_only_on_an_online_connection() {
        let cursor = wire::Seq::new(42).expect("positive cursor");
        assert_eq!(
            JournalCursor::resolve(ConnectionState::Online, Some(cursor)),
            Some(JournalCursor {
                value: Some("42".to_owned()),
                accessible: "journal cursor 42".to_owned(),
            })
        );
        assert_eq!(
            JournalCursor::resolve(ConnectionState::Online, None),
            Some(JournalCursor {
                value: None,
                accessible: "journal cursor not yet admitted".to_owned(),
            })
        );
        for connection in [
            ConnectionState::Starting,
            ConnectionState::Connecting,
            ConnectionState::Offline,
        ] {
            assert_eq!(JournalCursor::resolve(connection, Some(cursor)), None);
        }
    }

    #[test]
    fn titles_keep_product_branding_and_exact_count_state() {
        for (state, expected) in [
            (CountState::Loading, "Loading - spice"),
            (CountState::Disconnected, "Disconnected - spice"),
            (CountState::Current(0), "0 tasks - spice"),
            (CountState::Current(1), "1 task - spice"),
            (CountState::Current(LARGE_TITLE_COUNT), "1234 tasks - spice"),
            (CountState::Stale(KNOWN_COUNT), "7 tasks (stale) - spice"),
        ] {
            assert_eq!(BrowserBranding::for_state(state).title(), expected);
        }
    }

    #[test]
    fn badges_bound_large_counts_mark_freshness_and_fit_the_icon() {
        for (state, badge, tone) in [
            (CountState::Loading, "…", Tone::Loading),
            (CountState::Disconnected, "!", Tone::Disconnected),
            (CountState::Current(0), "0", Tone::Current),
            (CountState::Current(1), "1", Tone::Current),
            (CountState::Stale(1), "1", Tone::Stale),
            (
                CountState::Current(MAX_EXACT_BADGE_COUNT),
                "99",
                Tone::Current,
            ),
            (
                CountState::Current(MAX_EXACT_BADGE_COUNT + 1),
                "99+",
                Tone::Current,
            ),
            (CountState::Current(LARGE_TITLE_COUNT), "99+", Tone::Current),
        ] {
            let branding = BrowserBranding::for_state(state);
            assert_eq!((branding.badge.as_str(), branding.tone), (badge, tone));
            let (font_size, _) = label_metrics(badge);
            let advance = badge.chars().count() * font_size * MONOSPACE_ADVANCE_PERCENT / PERCENT;
            assert!(advance < BADGE_WIDTH, "{badge:?} overflows the badge");
        }
    }
}
