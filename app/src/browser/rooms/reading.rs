//! A Design's room read in place: its standing, positive conclusion control,
//! immutable proposal versions and exact decisions, kind-grouped
//! contributions, and room history for open and concluded Designs alike.
//! Showing the view joins, groups, or wakes nothing.

use std::rc::Rc;

use web_sys::{HtmlInputElement, HtmlTextAreaElement};
use wire::intent::{BoardTask, TaskAnswer};
use wire::rooms::{DesignMaturity, DesignMaturityState};
use wire::{Actor, CommandKind, TaskStamp};
use yew::prelude::*;

use super::{ClientAction, Controls, DraftRequest, RoomProjectionStatus, admit_action, card};
use crate::AppRuntime;
use crate::browser::sender::SenderHandle;
use crate::design_amendment::{DesignAmendment, DraftNotice, DraftPhase, Submission};

/// The read view of one Design's room. `detail` is the Design's row from its
/// task detail; the board's row wins while the board carries it, since only
/// the board follows working-group moves.
pub(in crate::browser) fn render_design_room(
    runtime: &AppRuntime,
    controls: &Controls,
    sender: &SenderHandle,
    detail: &BoardTask,
) -> Html {
    let design = &detail.statement.body.born;
    let row = runtime
        .board()
        .and_then(|board| board.row(design))
        .unwrap_or(detail);
    let projection = runtime.room_projection();
    let body = match projection.status() {
        RoomProjectionStatus::Loading => {
            html! { <p class="room-empty" aria-busy="true">{"Loading room…"}</p> }
        }
        RoomProjectionStatus::RequiresSnapshot => {
            html! { <p class="room-unavailable">{"Room authority unavailable until reconnect."}</p> }
        }
        RoomProjectionStatus::Available => {
            let events = projection.room(design).map(|room| room.in_journal_order());
            html! {
                <>
                    {super::standing::render_standing(runtime, design, Some(row))}
                    {render_maturity(projection.maturity(design), design)}
                    {render_amendment(runtime, controls, sender, row)}
                    {super::conclusion::render_conclusion(runtime, controls, sender, row)}
                    {super::approval::render_approval(runtime, controls, sender, design)}
                    {super::contributions::render_groups(runtime, design)}
                    {super::contribute::render_composer(runtime, controls, sender, row)}
                    <section class="room-history" aria-label="Room history">
                        <header>
                            <strong>{"Room history"}</strong>
                            <span>{events.as_ref().map_or(0, Vec::len)}</span>
                        </header>
                        if let Some(events) = events.filter(|events| !events.is_empty()) {
                            {for events.into_iter().rev().map(|(publication, event)| {
                                card::render_card(runtime, None, publication, event)
                            })}
                        } else {
                            <p class="room-empty">{"No room publications."}</p>
                        }
                    </section>
                </>
            }
        }
    };
    html! {
        <section class="app-sheet-section room-reading" data-design-room={design.to_string()}>
            <div class="app-section-heading">
                <h3>{"Design room"}</h3>
                <a href={crate::view::design_href(design)}>{"Permalink"}</a>
            </div>
            {body}
        </section>
    }
}

fn render_maturity(maturity: Option<&DesignMaturity>, design: &TaskStamp) -> Html {
    let empty = DesignMaturity::default();
    let maturity = maturity.unwrap_or(&empty);
    let (state, summary, attention) = match maturity.state() {
        DesignMaturityState::Fresh => (
            "fresh",
            "No contribution has been recorded.".to_owned(),
            false,
        ),
        DesignMaturityState::Steeped => (
            "steeped",
            "Substantive evidence spans three or more contributors.".to_owned(),
            true,
        ),
        DesignMaturityState::Steeping { unmet } => (
            "steeping",
            format!(
                "Still needs {}.",
                unmet
                    .into_iter()
                    .map(|requirement| requirement.label())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            false,
        ),
    };
    html! {
        <section
            class="room-maturity"
            aria-label="Design maturity"
            data-design={design.to_string()}
            data-maturity-state={state}
            data-operator-attention={attention.then_some("true")}
        >
            <header>
                <strong>{"Maturity"}</strong>
                <span class="room-maturity-state">{state}</span>
            </header>
            <p>{summary}</p>
            if attention {
                <p class="room-maturity-attention">{"Ready for operator attention."}</p>
            }
            <dl>
                <div>
                    <dt>{"Contributors"}</dt>
                    <dd>{bounded_count(maturity.contributing_lanes.len(), maturity.contributing_lanes_truncated)}</dd>
                </div>
                <div>
                    <dt>{"For"}</dt>
                    <dd>{maturity.contributions.reason_for}</dd>
                </div>
                <div>
                    <dt>{"Against"}</dt>
                    <dd>{maturity.contributions.reason_against}</dd>
                </div>
                <div>
                    <dt>{"Evidence"}</dt>
                    <dd>{maturity.contributions.evidence}</dd>
                </div>
                <div>
                    <dt>{"Questions"}</dt>
                    <dd>{maturity.contributions.question}</dd>
                </div>
                <div>
                    <dt>{"Organic reach"}</dt>
                    <dd>
                        {format!(
                            "{} notices · {} lanes",
                            maturity.organic_notices,
                            bounded_count(
                                maturity.organic_lanes.len(),
                                maturity.organic_lanes_truncated,
                            ),
                        )}
                    </dd>
                </div>
            </dl>
            if !maturity.recent_heads.is_empty() {
                <div class="room-maturity-heads" aria-label="Maturity by statement">
                    {for maturity.recent_heads.iter().rev().map(|head| html! {
                        <p>
                            <code>{head.statement.to_string()}</code>
                            <span>
                                {format!(
                                    "{} of {} invited lanes contributed{}",
                                    head.invited_contributors(),
                                    bounded_count(
                                        head.invited_lanes.len(),
                                        head.invited_lanes_truncated,
                                    ),
                                    if head.yield_truncated() { " (bounded)" } else { "" },
                                )}
                            </span>
                        </p>
                    })}
                </div>
            }
            if maturity.older_heads.statement_heads > 0 {
                <p class="room-maturity-folded">
                    {format!(
                        "{} older statement heads folded · {} invited/contributor lane-head matches",
                        maturity.older_heads.statement_heads,
                        maturity.older_heads.invited_contributor_lane_heads,
                    )}
                </p>
            }
            if maturity.unrecorded_head.contributions > 0 {
                <p class="room-maturity-folded">
                    {format!(
                        "{} historical contributions have an unrecorded statement head",
                        maturity.unrecorded_head.contributions,
                    )}
                </p>
            }
            <small>{"Advisory only; this neither blocks nor authorizes a Design decision."}</small>
        </section>
    }
}

fn bounded_count(count: usize, truncated: bool) -> String {
    format!("{count}{}", if truncated { "+" } else { "" })
}

fn render_amendment(
    runtime: &AppRuntime,
    controls: &Controls,
    sender: &SenderHandle,
    design: &BoardTask,
) -> Html {
    let identity = match runtime.principal().actor() {
        Actor::Lane(lane) => runtime
            .board()
            .and_then(|board| board.seats.occupant(*lane))
            .map(|occupant| (*lane, occupant)),
        Actor::Human(_) | Actor::Operator | Actor::Spice => None,
    };
    if !runtime.offers(CommandKind::TaskEdit)
        || !crate::design_amendment::visible(
            design,
            identity,
            runtime.room_projection().design_notices(),
        )
    {
        return Html::default();
    }

    let draft = controls.amendment(design);
    let callbacks = amendment_callbacks(controls, sender, design);
    let notice = render_amendment_notice(&draft);
    let phase = render_amendment_phase(&draft, &callbacks);
    html! {
        <section class="room-amendment" aria-label="Amend Design statement" data-design-amendment="">
            <header>
                <strong>{"Amend statement"}</strong>
                <span>{"No assignment or response is created."}</span>
            </header>
            {notice}
            {phase}
        </section>
    }
}

struct AmendmentCallbacks {
    title_change: Callback<InputEvent>,
    description_change: Callback<InputEvent>,
    preview: Callback<MouseEvent>,
    revise: Callback<MouseEvent>,
    submit: Callback<MouseEvent>,
}

fn amendment_callbacks(
    controls: &Controls,
    sender: &SenderHandle,
    design: &BoardTask,
) -> AmendmentCallbacks {
    let title_change = {
        let controls = controls.clone();
        let design = design.clone();
        Callback::from(move |event: InputEvent| {
            let input = event.target_unchecked_into::<HtmlInputElement>();
            controls.amend_title(&design, input.value());
        })
    };
    let description_change = {
        let controls = controls.clone();
        let design = design.clone();
        Callback::from(move |event: InputEvent| {
            let input = event.target_unchecked_into::<HtmlTextAreaElement>();
            controls.amend_description(&design, input.value());
        })
    };
    let preview = {
        let controls = controls.clone();
        let design = design.clone();
        Callback::from(move |_: MouseEvent| controls.preview_amendment(&design))
    };
    let revise = {
        let controls = controls.clone();
        let design = design.clone();
        Callback::from(move |_: MouseEvent| controls.revise_amendment(&design))
    };
    let submit = {
        let controls = controls.clone();
        let design = design.clone();
        let sender = sender.clone();
        Callback::from(move |_: MouseEvent| {
            submit_amendment(&sender, &controls, &design);
        })
    };
    AmendmentCallbacks {
        title_change,
        description_change,
        preview,
        revise,
        submit,
    }
}

fn render_amendment_notice(draft: &DesignAmendment) -> Html {
    match draft.notice() {
        Some(DraftNotice::Saved) => {
            html! { <p class="room-notice" role="status">{"Statement amendment saved."}</p> }
        }
        Some(DraftNotice::Refused(reason)) => {
            html! { <p class="room-contribute-refusal" role="status">{reason}</p> }
        }
        None => Html::default(),
    }
}

fn render_amendment_phase(draft: &DesignAmendment, callbacks: &AmendmentCallbacks) -> Html {
    match draft.phase() {
        DraftPhase::Editing => html! {
            <>
                <label>
                    <span>{"Title"}</span>
                    <input
                        type="text"
                        value={draft.title().to_owned()}
                        required=true
                        disabled={draft.is_pending()}
                        oninput={callbacks.title_change.clone()}
                    />
                </label>
                <label>
                    <span>{"Description"}</span>
                    <textarea
                        value={draft.description().to_owned()}
                        disabled={draft.is_pending()}
                        oninput={callbacks.description_change.clone()}
                    />
                </label>
                <div class="room-inline-actions">
                    <button type="button" onclick={callbacks.preview.clone()}>
                        {"Preview amendment"}
                    </button>
                </div>
            </>
        },
        DraftPhase::Previewing(submission) => html! {
            <>
                {render_amendment_changes(submission, false)}
                <div class="room-inline-actions">
                    <button type="button" onclick={callbacks.revise.clone()}>{"Revise"}</button>
                    <button type="button" class="primary" onclick={callbacks.submit.clone()}>
                        {"Save amendment"}
                    </button>
                </div>
            </>
        },
        DraftPhase::Pending(submission) => html! {
            <>
                {render_amendment_changes(submission, true)}
                <div class="room-inline-actions">
                    <button type="button" disabled=true>{"Saving…"}</button>
                </div>
            </>
        },
    }
}

fn render_amendment_changes(submission: &Submission, pending: bool) -> Html {
    html! {
        <p class="room-amendment-preview" aria-busy={pending.then_some("true")}>
            if let Some(title) = &submission.request.title {
                <span><strong>{"Title: "}</strong>{title}</span>
            }
            if let Some(description) = &submission.request.description {
                <span><strong>{"Description: "}</strong>{description}</span>
            }
        </p>
    }
}

fn submit_amendment(sender: &SenderHandle, controls: &Controls, design: &BoardTask) {
    let Some(submission) = controls.begin_amendment(design) else {
        return;
    };
    let task = design.statement.body.born.clone();
    let settle = controls.clone();
    let settled_task = task.clone();
    let respond = Rc::new(move |outcome: Result<TaskAnswer, String>| {
        settle.settle_amendment(&settled_task, outcome.map(|_| ()));
    });
    let admitted = admit_action(
        sender,
        ClientAction::Draft {
            request: DraftRequest::Edit {
                request: submission.request,
                expected: submission.expected,
            },
            respond,
        },
    );
    if !admitted {
        controls.settle_amendment(
            &task,
            Err("The statement amendment could not be queued.".to_owned()),
        );
    }
}
