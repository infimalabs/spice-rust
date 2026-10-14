//! A lane's typed contribution to an open Design from its read view: choose
//! a kind, write the text, cite room or typed sources, preview the
//! non-authorizing effect, and submit once. Contributions are lane-authored
//! evidence, so an operator session reads them but has no control; a
//! concluded Design has none either. The contribution renders only from the
//! committed room fact. A refusal stays beside the retained draft and moves
//! no task, claim, group, candidate, or authorization surface.

use std::rc::Rc;

use wasm_bindgen::JsCast;
use web_sys::{HtmlInputElement, HtmlTextAreaElement};
use wire::intent::BoardTask;
use wire::rooms::{RoomContributionKind, RoomSource};
use wire::{CommandKind, SequenceCode, TaskStamp};
use yew::prelude::*;

use super::Controls;
use crate::AppRuntime;
use crate::browser::sender::{SenderHandle, admit_action};
use crate::client_action::ClientAction;
use crate::contribution_draft::{ContributionDraft, DraftNotice, DraftPhase, Submission};
use crate::runtime::RoomEventKind;

/// Newest room publications offered as citations.
const CITABLE: usize = 8;

/// This page's one contribution draft, keyed by the Design it names.
#[derive(Clone, Default, PartialEq)]
pub(super) struct Composer(Option<ContributionDraft>);

/// One change to the draft for `design`. A change for another Design starts
/// that Design's draft unless it only settles an earlier submission.
pub(super) struct ComposerEdit {
    design: TaskStamp,
    settles: bool,
    change: Box<dyn FnOnce(&mut ContributionDraft)>,
}

impl Reducible for Composer {
    type Action = ComposerEdit;

    fn reduce(self: Rc<Self>, action: ComposerEdit) -> Rc<Self> {
        let names = |this: &Self| {
            this.0
                .as_ref()
                .is_some_and(|draft| *draft.design() == action.design)
        };
        if action.settles && !names(&self) {
            return self;
        }
        // Move the draft out when this is its only owner, so a keystroke
        // does not copy the whole draft.
        let mut draft = Rc::unwrap_or_clone(self)
            .0
            .filter(|draft| *draft.design() == action.design)
            .unwrap_or_else(|| ContributionDraft::new(action.design));
        (action.change)(&mut draft);
        Rc::new(Self(Some(draft)))
    }
}

/// Stable element id of `design`'s composer, which an invitation focuses.
pub(super) fn composer_id(design: &TaskStamp) -> String {
    format!("contribute-{design}")
}

/// The contribution section of an open Design's read view.
pub(super) fn render_composer(
    runtime: &AppRuntime,
    controls: &Controls,
    sender: &SenderHandle,
    row: &BoardTask,
) -> Html {
    if !row
        .design_standing()
        .is_some_and(|standing| standing.is_open())
    {
        return Html::default();
    }
    let design = row.statement.body.born.clone();
    if runtime.principal().is_operator() || !runtime.offers(CommandKind::RoomContribute) {
        return html! {
            <section class="room-contribute" aria-label="Contribute" data-room-contribute="read-only">
                <p class="room-notice">
                    {"Lanes contribute reasons, evidence, and questions here from their own sessions. Each is evidence for your judgment: it moves no task, claim, group, candidate, or authorization."}
                </p>
            </section>
        };
    }
    let empty = ContributionDraft::new(design.clone());
    let draft = controls
        .composer
        .0
        .as_ref()
        .filter(|draft| *draft.design() == design)
        .unwrap_or(&empty);
    let edit = |change: Rc<dyn Fn(&mut ContributionDraft)>| -> Box<dyn Fn()> {
        let (composer, design) = (controls.composer.clone(), design.clone());
        Box::new(move || {
            let change = Rc::clone(&change);
            composer.dispatch(ComposerEdit {
                design: design.clone(),
                settles: false,
                change: Box::new(move |draft| change(draft)),
            });
        })
    };
    let pending = draft.is_pending();
    let text = {
        let composer = controls.composer.clone();
        let design = design.clone();
        Callback::from(move |event: InputEvent| {
            let Some(value) = event
                .target()
                .and_then(|target| target.dyn_into::<HtmlTextAreaElement>().ok())
                .map(|textarea| textarea.value())
            else {
                return;
            };
            composer.dispatch(ComposerEdit {
                design: design.clone(),
                settles: false,
                change: Box::new(move |draft| draft.set_text(value)),
            });
        })
    };
    let preview = {
        let row = row.clone();
        let apply = edit(Rc::new(move |draft| draft.preview(&row)));
        Callback::from(move |_: MouseEvent| apply())
    };
    html! {
        <section
            class="room-contribute"
            id={composer_id(&design)}
            aria-label="Contribute"
            data-room-contribute={design.to_string()}
            tabindex="-1"
        >
            <header><strong>{"Contribute"}</strong></header>
            {render_kinds(&design, draft, &edit, pending)}
            <label class="room-contribute-text">
                <span>{"Text"}</span>
                <textarea
                    value={draft.text().to_owned()}
                    oninput={text}
                    disabled={pending}
                    rows="4"
                    placeholder="What this offers the Design"
                />
            </label>
            {render_citations(runtime, &design, draft, &edit, pending)}
            {render_typed_cite(controls, &design, pending)}
            <div class="room-inline-actions">
                <button type="button" onclick={preview} disabled={pending}>{"Preview contribution"}</button>
            </div>
            {render_phase(controls, sender, row, draft, &edit)}
            {render_notice(draft.notice())}
        </section>
    }
}

/// The kind radio group; arrow keys move between kinds as native radios do.
fn render_kinds(
    design: &TaskStamp,
    draft: &ContributionDraft,
    edit: &impl Fn(Rc<dyn Fn(&mut ContributionDraft)>) -> Box<dyn Fn()>,
    pending: bool,
) -> Html {
    html! {
        <fieldset class="room-contribute-kinds" disabled={pending}>
            <legend>{"Offers"}</legend>
            {for RoomContributionKind::ALL.into_iter().map(|kind| {
                let choose = edit(Rc::new(move |draft| draft.set_kind(kind)));
                html! {
                    <label key={kind.as_str()}>
                        <input
                            type="radio"
                            name={format!("contribution-kind-{design}")}
                            value={kind.as_str()}
                            checked={draft.kind() == Some(kind)}
                            onchange={Callback::from(move |_: Event| choose())}
                        />
                        {super::contributions::kind_label(kind)}
                    </label>
                }
            })}
        </fieldset>
    }
}

/// Cite an artifact digest or journal fact code by its typed spelling.
fn render_typed_cite(controls: &Controls, design: &TaskStamp, pending: bool) -> Html {
    let typed = NodeRef::default();
    let cite = {
        let typed = typed.clone();
        let (composer, design) = (controls.composer.clone(), design.clone());
        Callback::from(move |_: MouseEvent| {
            let Some(input) = typed.cast::<HtmlInputElement>() else {
                return;
            };
            let spelling = input.value();
            input.set_value("");
            composer.dispatch(ComposerEdit {
                design: design.clone(),
                settles: false,
                change: Box::new(move |draft| draft.cite(&spelling)),
            });
        })
    };
    html! {
        <div class="room-inline-actions room-contribute-cite">
            <input
                ref={typed}
                type="text"
                aria-label="Artifact digest or journal fact code"
                placeholder="Artifact digest or journal fact code"
                disabled={pending}
            />
            <button type="button" onclick={cite} disabled={pending}>{"Cite"}</button>
        </div>
    }
}

/// The draft's cited sources, removable, and the newest room publications
/// offered as transcript or journal citations.
fn render_citations(
    runtime: &AppRuntime,
    design: &TaskStamp,
    draft: &ContributionDraft,
    edit: &impl Fn(Rc<dyn Fn(&mut ContributionDraft)>) -> Box<dyn Fn()>,
    pending: bool,
) -> Html {
    let mut offered = Vec::new();
    if let Some(room) = runtime.room_projection().room(design) {
        for (publication, event) in room.in_journal_order().into_iter().rev().take(CITABLE) {
            let (kind, summary) = super::event_headline(runtime, publication, event);
            if let RoomEventKind::Messaged { source, .. } = &event.kind {
                offered.push((
                    RoomSource::Transcript(source.clone()),
                    format!("Transcript of {kind}: {summary}"),
                ));
            }
            offered.push((
                RoomSource::Journal {
                    fact: SequenceCode::new(event.position.get()),
                },
                format!("Journal fact of {kind}: {summary}"),
            ));
        }
    }
    let typed = draft
        .sources()
        .iter()
        .filter(|source| !offered.iter().any(|(offer, _)| offer == *source))
        .map(|source| (source.clone(), typed_label(source)))
        .collect::<Vec<_>>();
    html! {
        <fieldset class="room-contribute-sources" disabled={pending}>
            <legend>{format!("Sources ({} cited)", draft.sources().len())}</legend>
            if offered.is_empty() && typed.is_empty() {
                <p class="room-empty">{"Nothing in this room to cite yet."}</p>
            }
            {for typed.into_iter().chain(offered).map(|(source, label)| {
                let cited = draft.sources().contains(&source);
                let toggle = edit(Rc::new({
                    let source = source.clone();
                    move |draft| draft.toggle_source(source.clone())
                }));
                html! {
                    <label data-source-kind={source_kind(&source)}>
                        <input
                            type="checkbox"
                            checked={cited}
                            onchange={Callback::from(move |_: Event| toggle())}
                        />
                        <span>{label}</span>
                    </label>
                }
            })}
        </fieldset>
    }
}

/// The previewed effect with Submit and Cancel, or the pending submission.
fn render_phase(
    controls: &Controls,
    sender: &SenderHandle,
    row: &BoardTask,
    draft: &ContributionDraft,
    edit: &impl Fn(Rc<dyn Fn(&mut ContributionDraft)>) -> Box<dyn Fn()>,
) -> Html {
    let handle = &row.statement.body.handle;
    match draft.phase() {
        DraftPhase::Editing => Html::default(),
        DraftPhase::Pending(_) => html! {
            <p class="room-notice" role="status" aria-busy="true" data-contribution-pending="">
                {"Submitting the contribution…"}
            </p>
        },
        DraftPhase::Previewing(submission) => {
            let kind = super::contributions::kind_label(submission.request.kind);
            let cited = submission.request.sources.len();
            let effect = format!(
                "Records one {} in {handle}'s room as this lane, citing {cited} source{}. It is evidence for the operator's judgment: it moves no task, claim, working group, candidate, or authorization.",
                kind.to_lowercase(),
                if cited == 1 { "" } else { "s" },
            );
            let submit = submit(controls, sender, draft.design(), submission);
            let cancel = edit(Rc::new(ContributionDraft::revise));
            html! {
                <div class="room-contribute-preview" data-contribution-preview={submission.request.kind.as_str()}>
                    <p>{effect}</p>
                    <div class="room-inline-actions">
                        <button type="button" onclick={submit}>{"Submit contribution"}</button>
                        <button type="button" onclick={Callback::from(move |_: MouseEvent| cancel())}>{"Cancel"}</button>
                    </div>
                </div>
            }
        }
    }
}

/// Send the previewed submission once: the in-flight set refuses a second
/// click before the pending state renders, and the draft refuses a second
/// take after it does.
fn submit(
    controls: &Controls,
    sender: &SenderHandle,
    design: &TaskStamp,
    submission: &Submission,
) -> Callback<MouseEvent> {
    let (composer, contributing, sender) = (
        controls.composer.clone(),
        Rc::clone(&controls.contributing),
        sender.clone(),
    );
    let (design, submission) = (design.clone(), submission.clone());
    Callback::from(move |_: MouseEvent| {
        if !contributing.borrow_mut().insert(design.clone()) {
            return;
        }
        composer.dispatch(ComposerEdit {
            design: design.clone(),
            settles: false,
            change: Box::new(|draft| {
                draft.begin_submit();
            }),
        });
        let settle = {
            let (composer, contributing, design) =
                (composer.clone(), Rc::clone(&contributing), design.clone());
            move |outcome: Result<wire::intent::RoomContributeAnswer, String>| {
                contributing.borrow_mut().remove(&design);
                composer.dispatch(ComposerEdit {
                    design: design.clone(),
                    settles: true,
                    change: Box::new(move |draft| draft.settle(outcome)),
                });
            }
        };
        let action = ClientAction::Contribute {
            request: submission.request.clone(),
            expected: submission.expected.clone(),
            respond: Rc::new(settle.clone()),
        };
        if !admit_action(&sender, action) {
            settle(Err("The contribution could not be queued.".to_owned()));
        }
    })
}

fn render_notice(notice: Option<&DraftNotice>) -> Html {
    match notice {
        None => Html::default(),
        Some(DraftNotice::Refused(reason)) => html! {
            <p class="room-notice room-contribute-refusal" role="alert" data-contribution-refusal="">
                {reason.clone()}
            </p>
        },
        Some(DraftNotice::Contributed(publication)) => html! {
            <p class="room-notice" role="status" data-contribution-recorded={publication.to_string()}>
                {format!("Recorded {publication}; it appears in the room as the journal delivers it.")}
            </p>
        },
    }
}

const fn source_kind(source: &RoomSource) -> &'static str {
    match source {
        RoomSource::Transcript(_) => "transcript",
        RoomSource::Journal { .. } => "journal",
        RoomSource::Artifact { .. } => "artifact",
    }
}

fn typed_label(source: &RoomSource) -> String {
    match source {
        RoomSource::Transcript(hunk) => format!(
            "Transcript member {} offset {} bytes {}..{}",
            hunk.locus.member, hunk.locus.offset, hunk.text.start, hunk.text.end
        ),
        RoomSource::Journal { fact } => format!("Journal fact {fact}"),
        RoomSource::Artifact { digest } => format!("Artifact {digest}"),
    }
}
