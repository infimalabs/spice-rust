//! A design menu's Nudge: push an open Design to proposal before its room
//! steeps. The resident readies it and messages the one free lane whose
//! retained session best matches it; the answer, reported to the design
//! composer, names that lane or says none could take it.

use std::rc::Rc;

use wire::intent::{DesignStanding, TextAnswer};
use wire::{CommandKind, TaskStamp};
use yew::prelude::*;

use super::statement_witness;
use crate::AppRuntime;
use crate::browser::sender::{SenderHandle, admit_action};
use crate::client_action::{ClientAction, PreparationRequest};

/// One open Design's Nudge control; any other task shows nothing.
pub(super) fn nudge_control(
    runtime: &AppRuntime,
    report: &Callback<String>,
    sender: &SenderHandle,
    room: &TaskStamp,
) -> Html {
    let Some(anchor) = runtime.board().and_then(|board| board.row(room)) else {
        return Html::default();
    };
    if !matches!(
        anchor.design_standing(),
        Some(DesignStanding::Active | DesignStanding::Passive)
    ) {
        return Html::default();
    }
    let design = anchor.statement.body.handle.clone();
    let expected = vec![
        statement_witness(anchor),
        wire::Witness::Workflow {
            task: anchor.statement.body.born.clone(),
            head: anchor.workflow.position,
            phase: anchor.workflow.body.phase.clone(),
        },
    ];
    let report = report.clone();
    let sender = sender.clone();
    let onclick = Callback::from(move |_: MouseEvent| {
        let report = report.clone();
        let respond = Rc::new(move |answer: Result<TextAnswer, String>| {
            report.emit(answer.map_or_else(|error| error, |answer| answer.text));
        });
        let _ = admit_action(
            &sender,
            ClientAction::Preparation {
                request: PreparationRequest::Nudge {
                    request: wire::rooms::DesignNudge {
                        design: design.clone(),
                    },
                    expected: expected.clone(),
                },
                respond,
            },
        );
    });
    html! {
        <div class="room-inline-actions" data-design-nudge={room.to_string()}>
            <button
                type="button"
                title="Push this Design to proposal now and message the best-matching free lane"
                disabled={!runtime.offers(CommandKind::DesignNudge)}
                {onclick}
            >
                {"Nudge"}
            </button>
        </div>
    }
}
