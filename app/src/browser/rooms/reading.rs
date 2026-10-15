//! A Design's room read in place: its standing, positive conclusion control,
//! immutable proposal versions and exact decisions, kind-grouped
//! contributions, and room history for open and concluded Designs alike.
//! Showing the view joins, groups, or wakes nothing.

use std::rc::Rc;

use web_sys::{HtmlInputElement, HtmlTextAreaElement};
use wire::intent::{BoardTask, TaskAnswer};
use wire::{Actor, CommandKind};
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
        Actor::Operator | Actor::Spice => None,
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
