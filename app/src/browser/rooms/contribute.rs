//! What contributing to an open Design means. Lanes contribute through
//! `spice room contribute` from their own sessions; the browser's human reader
//! has no contribution control, and a concluded Design shows nothing here.

use wire::intent::BoardTask;
use yew::prelude::*;

/// The contribution note of an open Design's read view.
pub(super) fn render_note(row: &BoardTask) -> Html {
    if !row
        .design_standing()
        .is_some_and(|standing| standing.is_open())
    {
        return Html::default();
    }
    html! {
        <section class="room-contribute" aria-label="Contribute">
            <p class="room-notice">
                {"Lanes contribute reasons, evidence, and questions here from their own sessions. Each is evidence for your judgment: it moves no task, claim, group, candidate, or authorization."}
            </p>
        </section>
    }
}
