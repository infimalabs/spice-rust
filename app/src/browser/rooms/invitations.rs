//! A lane session's organic Design invitations. Following one opens that
//! exact Design's read view and its contribution affordance; it joins no
//! working group, acquires nothing, and answers no steering.

use wire::Actor;
use yew::prelude::*;

use crate::AppRuntime;

/// The invitations told to this session's lane, newest first; nothing for an
/// operator session or before room authority loads.
pub(in crate::browser) fn render_invitations(runtime: Option<&AppRuntime>) -> Html {
    let Some(runtime) = runtime else {
        return Html::default();
    };
    let Actor::Lane(lane) = runtime.principal().actor() else {
        return Html::default();
    };
    let invitations = runtime
        .room_projection()
        .invitations(*lane, runtime.board());
    html! {
        <section class="room-invitations" aria-label="Design invitations" data-room-invitations="">
            <header>
                <h3>{"Design invitations"}</h3>
                <span>{invitations.len()}</span>
            </header>
            if invitations.is_empty() {
                <p class="room-empty">{"No open Design has invited this lane."}</p>
            }
            {for invitations.into_iter().map(|notice| {
                let design = &notice.value.design;
                let label = super::anchor_label(runtime, design);
                html! {
                    <article
                        class="room-card room-invitation-card"
                        key={notice.position.get()}
                        data-invitation={design.to_string()}
                        data-invitation-statement={notice.value.statement.get().to_string()}
                    >
                        <a href={crate::view::design_href(design)}>
                            {format!("Contribute to {label}")}
                        </a>
                        <p>
                            {format!(
                                "Organic match with {} on {}. Opening it joins no group and claims nothing.",
                                super::anchor_label(runtime, &notice.value.task),
                                wire::rooms::shown_notice_terms(&notice.value.terms).join(", "),
                            )}
                        </p>
                    </article>
                }
            })}
        </section>
    }
}
