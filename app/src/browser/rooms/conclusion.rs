//! The operator's explicit conclusion of an open Design from its read view:
//! preview done or collapse-with-reason, then submit it at the heads the
//! preview was drawn from. The standing moves only when the board does; a
//! refusal, such as a live claim or heads that moved, is reported in place.

use std::rc::Rc;

use wire::body::DesignConclusion;
use wire::intent::{BoardTask, TaskConclude};
use wire::{CommandKind, TaskStamp, Witness};
use yew::prelude::*;

use super::{Controls, input_value, transition_witnesses};
use crate::AppRuntime;
use crate::browser::sender::{SenderHandle, admit_action};
use crate::client_action::ClientAction;

/// One Design's conclusion in progress: the previewed conclusion with the
/// heads it was drawn from, and the outcome of the last submission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Conclusion {
    design: TaskStamp,
    preview: Option<(DesignConclusion, Vec<Witness>)>,
    notice: Option<String>,
}

/// The conclusion section of an open Design's read view; absent once it
/// concludes or while the operator socket does not offer conclusion.
pub(super) fn render_conclusion(
    runtime: &AppRuntime,
    controls: &Controls,
    sender: &SenderHandle,
    row: &BoardTask,
) -> Html {
    if !row
        .design_standing()
        .is_some_and(|standing| standing.is_open())
        || !runtime.offers(CommandKind::TaskConclude)
    {
        return Html::default();
    }
    let design = &row.statement.body.born;
    let state = (*controls.conclusion)
        .as_ref()
        .filter(|state| state.design == *design);
    let reason = NodeRef::default();
    let preview = |conclusion: Option<DesignConclusion>| {
        let (state, reason, design) = (controls.conclusion.clone(), reason.clone(), design.clone());
        let witnesses = transition_witnesses(row);
        Callback::from(move |_: MouseEvent| {
            let conclusion = conclusion
                .clone()
                .unwrap_or_else(|| DesignConclusion::Collapsed {
                    reason: input_value(&reason).unwrap_or_default().trim().to_owned(),
                });
            let empty =
                matches!(&conclusion, DesignConclusion::Collapsed { reason } if reason.is_empty());
            state.set(Some(Conclusion {
                design: design.clone(),
                preview: (!empty).then(|| (conclusion, witnesses.clone())),
                notice: empty.then(|| {
                    "task::collapse-reason-empty: state why its premise did not hold".to_owned()
                }),
            }));
        })
    };
    let (done, collapse) = (preview(Some(DesignConclusion::Done {})), preview(None));
    html! {
        <section class="room-conclusion" aria-label="Conclusion" data-room-conclusion={design.to_string()}>
            <header><strong>{"Conclusion"}</strong></header>
            <p class="room-notice">
                {"Only the operator concludes a Design. Done ends it, keeping outputs you already selected released; collapse ends it with a reason and keeps every output gated. Its room history and retained candidates stay readable."}
            </p>
            <div class="room-inline-actions">
                <button type="button" onclick={done}>{"Preview done"}</button>
            </div>
            <label>
                <span>{"Collapse reason"}</span>
                <input ref={reason} type="text" placeholder="Why its premise did not hold" />
            </label>
            <div class="room-inline-actions">
                <button type="button" onclick={collapse}>{"Preview collapse"}</button>
            </div>
            if let Some((conclusion, witnesses)) = state.and_then(|state| state.preview.as_ref()) {
                {render_preview(controls, sender, row, conclusion, witnesses)}
            }
            if let Some(notice) = state.and_then(|state| state.notice.clone()) {
                <p class="room-notice" role="status" data-room-conclusion-notice="">{notice}</p>
            }
        </section>
    }
}

/// What submitting the previewed conclusion does, and its Submit and Cancel.
fn render_preview(
    controls: &Controls,
    sender: &SenderHandle,
    row: &BoardTask,
    conclusion: &DesignConclusion,
    witnesses: &[Witness],
) -> Html {
    let handle = &row.statement.body.handle;
    let effect = match conclusion {
        DesignConclusion::Done {} => format!(
            "Conclude {handle} done. The resident refuses while an output still owes your selection."
        ),
        DesignConclusion::Collapsed { reason } => {
            format!(
                "Conclude {handle} collapsed: {reason}. Every output it constructed stays gated."
            )
        }
    };
    let held = row
        .claim
        .as_ref()
        .is_some_and(|claim| claim.body.disposition == wire::ClaimDisposition::Active);
    let preview = Some((conclusion.clone(), witnesses.to_vec()));
    let submit = {
        let (state, sender) = (controls.conclusion.clone(), sender.clone());
        let (design, expected) = (row.statement.body.born.clone(), witnesses.to_vec());
        let request = TaskConclude {
            task: handle.clone(),
            conclusion: conclusion.clone(),
        };
        Callback::from(move |_: MouseEvent| {
            // Only the board moves the standing: a landed conclusion clears
            // the preview, and a refusal keeps it beside the refusal.
            let report = {
                let (state, design, preview) = (state.clone(), design.clone(), preview.clone());
                move |notice: String, landed: bool| {
                    state.set(Some(Conclusion {
                        design: design.clone(),
                        preview: if landed { None } else { preview.clone() },
                        notice: Some(notice),
                    }));
                }
            };
            let respond = {
                let (report, handle) = (report.clone(), request.task.clone());
                Rc::new(
                    move |answer: Result<wire::intent::TaskAnswer, String>| match answer {
                        Ok(_) => report(format!("Concluded {handle}."), true),
                        Err(refusal) => report(refusal, false),
                    },
                )
            };
            let action = ClientAction::Conclude {
                request: request.clone(),
                expected: expected.clone(),
                respond,
            };
            if !admit_action(&sender, action) {
                report("The conclusion could not be queued.".to_owned(), false);
            }
        })
    };
    let cancel = {
        let state = controls.conclusion.clone();
        Callback::from(move |_: MouseEvent| state.set(None))
    };
    html! {
        <div class="room-conclusion-preview" data-room-conclusion-preview={conclusion_key(conclusion)}>
            <p>{effect}</p>
            if held {
                <p class="room-notice">{"A lane holds this Design; the resident refuses conclusion until it frees."}</p>
            }
            <div class="room-inline-actions">
                <button type="button" onclick={submit}>{"Submit conclusion"}</button>
                <button type="button" onclick={cancel}>{"Cancel"}</button>
            </div>
        </div>
    }
}

const fn conclusion_key(conclusion: &DesignConclusion) -> &'static str {
    match conclusion {
        DesignConclusion::Done {} => "done",
        DesignConclusion::Collapsed { .. } => "collapsed",
    }
}
