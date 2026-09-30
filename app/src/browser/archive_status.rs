//! The one status row a paged task archive (global Work history, a team's
//! Settled lane) keeps mounted under its finder. A read starting, finishing,
//! or failing only swaps text and enables controls inside this row, and a
//! page replaces the cards below it, so no read moves the controls.

use yew::prelude::*;

/// What one archive's page reads show, drawn from its runtime state.
pub(super) struct ArchiveStatus<'a> {
    /// The loaded page's first and last results and the archive's total.
    pub range: Option<(u64, u64, u64)>,
    pub has_newer: bool,
    pub has_older: bool,
    /// A read is in flight, or none has been answered yet.
    pub loading: bool,
    pub issue: Option<&'a str>,
    /// Names the page controls for assistive technology.
    pub pages_label: &'static str,
    pub newer: Callback<MouseEvent>,
    pub older: Callback<MouseEvent>,
    pub retry: Callback<MouseEvent>,
}

impl ArchiveStatus<'_> {
    fn position(&self) -> &'static str {
        match (self.has_newer, self.has_older) {
            (false, false) => "Newest and oldest page",
            (false, true) => "Newest page",
            (true, false) => "Oldest page",
            (true, true) => "Between newest and oldest",
        }
    }
}

/// The archive's result count, "Not loaded" before any page has answered.
pub(super) fn count(range: Option<(u64, u64, u64)>) -> String {
    range.map_or_else(
        || "Not loaded".to_owned(),
        |(first, last, total)| format!("{first}–{last} of {total}"),
    )
}

/// The status row. Its second line reads the page position, the read in
/// flight, or the failure; Retry holds its slot in the rail either way and is
/// shown and operable only while a read has failed.
pub(super) fn render(status: &ArchiveStatus<'_>) -> Html {
    let failed = status.issue.is_some();
    html! {
        <div class="work-history-pagination" aria-busy={status.loading.to_string()}>
            <div class="work-history-range">
                <strong role="status">{count(status.range)}</strong>
                {match status.issue {
                    Some(issue) => html! {
                        <span class="work-history-issue" role="alert" title={issue.to_owned()}>
                            {issue}
                        </span>
                    },
                    None if status.loading => html! { <span role="status">{"Loading page…"}</span> },
                    None if status.range.is_some() => html! { <span>{status.position()}</span> },
                    None => html! { <span>{"\u{a0}"}</span> },
                }}
            </div>
            <div
                class="work-history-page-actions canvas-control-rail team-mode-rail"
                role="group"
                aria-label={status.pages_label}
            >
                <button
                    type="button"
                    class="team-mode work-history-retry"
                    data-shown={failed.to_string()}
                    onclick={status.retry.clone()}
                >
                    {"Retry"}
                </button>
                <button
                    type="button"
                    class="team-mode"
                    disabled={status.loading || !status.has_newer}
                    onclick={status.newer.clone()}
                >
                    {"Newer"}
                </button>
                <button
                    type="button"
                    class="team-mode"
                    disabled={status.loading || !status.has_older}
                    onclick={status.older.clone()}
                >
                    {"Older"}
                </button>
            </div>
        </div>
    }
}
