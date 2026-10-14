//! Every immutable proposal version and the operator's exact decision surface.
//! Selection, Return, and Collapse are previewed before one atomic
//! `preparation decide`; no browser-local standing is invented from a reply.

use std::rc::Rc;

use wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;
use wire::intent::{BoardTask, TextAnswer};
use wire::rooms::{
    CandidateCoverage, CandidateCritique, CandidateRef, CoverageAuthority, DecisionOutcome,
    ExecutionAuthorization, PreparationDecision, RetainedCandidate, RoomSource,
};
use wire::{CommandKind, SequenceCode, TaskStamp};
use yew::prelude::*;

use super::{Controls, conversation::lane_label};
use crate::AppRuntime;
use crate::browser::sender::{SenderHandle, admit_action};
use crate::client_action::{ClientAction, PreparationRequest};
use crate::preparation_decision::{
    DecisionChoice, DecisionDraft, DraftNotice, DraftPhase, Submission,
};
use crate::runtime::{PreparationProjection, RoomPositioned};

type DraftChange = Rc<dyn Fn(&mut DecisionDraft)>;
type DraftCommand = Box<dyn Fn()>;

/// This page's one decision draft, keyed by exact candidate version.
#[derive(Clone, Default, PartialEq)]
pub(super) struct Composer(Option<DecisionDraft>);

/// One edit to one version. A late response cannot settle another version.
pub(super) struct ComposerEdit {
    candidate: CandidateRef,
    settles: bool,
    change: Box<dyn FnOnce(&mut DecisionDraft)>,
}

impl Reducible for Composer {
    type Action = ComposerEdit;

    fn reduce(self: Rc<Self>, action: ComposerEdit) -> Rc<Self> {
        let names = |this: &Self| {
            this.0
                .as_ref()
                .is_some_and(|draft| *draft.candidate() == action.candidate)
        };
        if action.settles && !names(&self) {
            return self;
        }
        let mut draft = Rc::unwrap_or_clone(self)
            .0
            .filter(|draft| *draft.candidate() == action.candidate)
            .unwrap_or_else(|| DecisionDraft::new(action.candidate));
        (action.change)(&mut draft);
        Rc::new(Self(Some(draft)))
    }
}

/// Where one version stands under committed authority.
enum Standing<'a> {
    Undecided,
    Returned,
    Collapse,
    Selected(&'a wire::rooms::DecisionRef),
    Authorized(&'a ExecutionAuthorization),
}

impl Standing<'_> {
    fn label(&self) -> String {
        match self {
            Self::Undecided => "Undecided".to_owned(),
            Self::Returned => "Returned".to_owned(),
            Self::Collapse => "Collapsed".to_owned(),
            Self::Selected(decision) => format!("Selected by decision {}", decision.decision),
            Self::Authorized(authorization) => format!(
                "Selected {} output(s) by decision {}",
                authorization.selected_outputs.len(),
                authorization.decision.decision
            ),
        }
    }
}

struct Context<'a> {
    runtime: &'a AppRuntime,
    controls: &'a Controls,
    sender: &'a SenderHandle,
    preparation: &'a PreparationProjection,
    tasks: &'a [BoardTask],
    /// Current open Design row. Absence makes all proposal versions read-only.
    controller: Option<&'a BoardTask>,
}

impl<'a> Context<'a> {
    fn task(&self, stamp: &TaskStamp) -> Option<&'a BoardTask> {
        self.tasks
            .iter()
            .find(|task| task.statement.body.born == *stamp)
    }

    fn interactive(&self) -> bool {
        self.controller.is_some()
            && self.runtime.principal().is_operator()
            && self.runtime.offers(CommandKind::PreparationDecide)
    }
}

/// Every proposal version retained under one Design, with controls only for an
/// operator looking at the still-open controller.
pub(super) fn render_approval(
    runtime: &AppRuntime,
    controls: &Controls,
    sender: &SenderHandle,
    design: &TaskStamp,
) -> Html {
    let preparation = runtime.room_projection().preparation();
    let Some(board) = runtime.board() else {
        return Html::default();
    };
    let mut candidates = preparation
        .candidates
        .values()
        .filter(|positioned| positioned.value.candidate.controller == *design)
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| candidate.position);
    if candidates.is_empty() {
        return Html::default();
    }
    let context = Context {
        runtime,
        controls,
        sender,
        preparation,
        tasks: &board.tasks,
        controller: board.row(design).filter(|task| {
            task.design_standing()
                .is_some_and(|standing| standing.is_open())
        }),
    };
    html! {
        <section class="room-approval" aria-label="Proposal versions" data-room-proposals="">
            <header>
                <strong>{"Proposal versions"}</strong>
                <span>{candidates.len()}</span>
            </header>
            <p class="room-notice">
                {"Every retained version is immutable. Select an exact output subset, Return it for another pass, or Collapse the Design; every choice is previewed before it records authority."}
            </p>
            {for candidates.into_iter().rev().map(|candidate| render_candidate(&context, candidate))}
        </section>
    }
}

fn render_candidate(context: &Context<'_>, positioned: &RoomPositioned<RetainedCandidate>) -> Html {
    let candidate = &positioned.value;
    let empty = DecisionDraft::new(candidate.candidate.clone());
    let draft = context
        .controls
        .decision
        .0
        .as_ref()
        .filter(|draft| *draft.candidate() == candidate.candidate)
        .unwrap_or(&empty);
    let standing = standing(context.preparation, &candidate.candidate);
    let coverage = context
        .preparation
        .coverage
        .values()
        .filter(|coverage| coverage.value.coverage.candidate == candidate.candidate)
        .map(|coverage| &coverage.value)
        .collect::<Vec<_>>();
    let critiques = context
        .preparation
        .critiques
        .values()
        .filter(|critique| critique.value.critique.candidate == candidate.candidate)
        .collect::<Vec<_>>();
    let decisions = context
        .preparation
        .decisions
        .values()
        .filter(|decision| decision.value.outcome.candidate() == &candidate.candidate)
        .collect::<Vec<_>>();
    let authorizations = context
        .preparation
        .authorizations
        .values()
        .filter(|authorization| authorization.value.candidate == candidate.candidate)
        .collect::<Vec<_>>();
    let code = candidate.candidate.candidate.to_string();
    let fact = SequenceCode::new(positioned.position.get());
    let pending = draft.is_pending();
    let interactive = context.interactive();
    let edit = editor(context.controls, &candidate.candidate);
    html! {
        <article
            class="room-card room-approval-card"
            key={code.clone()}
            data-candidate={code.clone()}
            tabindex="0"
        >
            <header>
                <strong>{format!("Proposal {code}")}</strong>
                <span data-approval-standing="">{standing.label()}</span>
                <code title="Retaining journal fact">{fact.to_string()}</code>
            </header>
            {render_inputs(context, candidate)}
            {render_references(candidate)}
            {render_evidence(context, candidate)}
            {render_critiques(&critiques)}
            {render_coverage(context.runtime, &coverage)}
            <ul class="room-approval-outputs" aria-label="Proposed outputs">
                {for candidate.outputs.iter().map(|output| {
                    render_output(context, output, draft, interactive, pending, &edit)
                })}
            </ul>
            {render_decision_history(&decisions, &authorizations)}
            if interactive {
                {render_controls(context, positioned, draft, &edit)}
            } else if context.controller.is_none() {
                <p class="room-notice">{"This Design is concluded; its proposal history is read-only."}</p>
            }
            {render_notice(draft.notice())}
        </article>
    }
}

fn editor(controls: &Controls, candidate: &CandidateRef) -> impl Fn(DraftChange) -> DraftCommand {
    let (composer, candidate) = (controls.decision.clone(), candidate.clone());
    move |change: DraftChange| {
        let (composer, candidate) = (composer.clone(), candidate.clone());
        Box::new(move || {
            let change = Rc::clone(&change);
            composer.dispatch(ComposerEdit {
                candidate: candidate.clone(),
                settles: false,
                change: Box::new(move |draft| change(draft)),
            });
        }) as DraftCommand
    }
}

fn render_inputs(context: &Context<'_>, candidate: &RetainedCandidate) -> Html {
    if candidate.governed_inputs.is_empty() {
        return html! { <p class="room-approval-inputs">{"Governed inputs: none."}</p> };
    }
    html! {
        <div class="room-approval-inputs">
            <strong>{"Governed inputs"}</strong>
            <ul>
                {for candidate.governed_inputs.iter().map(|input| match context.task(input) {
                    Some(task) => html! {
                        <li key={input.to_string()}>
                            <strong>{task.presentation_title.clone()}</strong>
                            {" "}<code>{task.statement.body.handle.to_string()}</code>
                        </li>
                    },
                    None => html! {
                        <li key={input.to_string()} data-reference-state="unavailable">
                            <code>{input.to_string()}</code>{" · unavailable on the current board"}
                        </li>
                    },
                })}
            </ul>
        </div>
    }
}

fn render_references(candidate: &RetainedCandidate) -> Html {
    html! {
        <div class="room-approval-references">
            <strong>{"Retained references"}</strong>
            <ul>
                {for candidate.references.iter().map(|reference| html! {
                    <li>
                        <span>{reference.relation.to_string()}</span>
                        {" · "}<code>{source_label(&reference.source)}</code>
                        <span class="room-reference-state">{"exact link; availability is not resolved by this view"}</span>
                    </li>
                })}
            </ul>
        </div>
    }
}

fn render_evidence(context: &Context<'_>, candidate: &RetainedCandidate) -> Html {
    if candidate.evidence.is_empty() {
        return html! { <p class="room-approval-evidence">{"Question evidence: none cited."}</p> };
    }
    let conversation = context.runtime.room_projection().conversation();
    html! {
        <div class="room-approval-evidence">
            <strong>{"Question evidence"}</strong>
            <ul>
                {for candidate.evidence.iter().map(|result| match conversation.result(result) {
                    Some(question) => html! {
                        <li key={result.to_string()}>
                            <code>{result.to_string()}</code>
                            {format!(" · {}", question.value.state.name().replace('_', " "))}
                        </li>
                    },
                    None => html! {
                        <li key={result.to_string()} data-reference-state="unavailable">
                            <code>{result.to_string()}</code>{" · unavailable in the current room projection"}
                        </li>
                    },
                })}
            </ul>
        </div>
    }
}

fn render_critiques(critiques: &[&RoomPositioned<CandidateCritique>]) -> Html {
    if critiques.is_empty() {
        return html! { <p class="room-approval-critiques">{"Critiques: none recorded."}</p> };
    }
    html! {
        <div class="room-approval-critiques">
            <strong>{"Critiques"}</strong>
            <ul>
                {for critiques.iter().map(|critique| html! {
                    <li key={critique.value.critique.critique.to_string()}>
                        {format!(
                            "{} · {} · {}",
                            critique.value.critique.critique,
                            critique.value.category,
                            match critique.value.standing {
                                wire::rooms::CritiqueStanding::Clean => "clean",
                                wire::rooms::CritiqueStanding::MaterialFinding => "material finding",
                            },
                        )}
                        {" · "}<code>{source_label(&critique.value.source)}</code>
                    </li>
                })}
            </ul>
        </div>
    }
}

fn render_coverage(runtime: &AppRuntime, coverage: &[&CandidateCoverage]) -> Html {
    if coverage.is_empty() {
        return html! { <p class="room-approval-coverage">{"Review coverage: none recorded."}</p> };
    }
    html! {
        <div class="room-approval-coverage">
            <strong>{"Review coverage"}</strong>
            <ul aria-label="Review coverage">
                {for coverage.iter().map(|coverage| {
                    let by = match &coverage.authority {
                        CoverageAuthority::Operator {} => "operator review".to_owned(),
                        CoverageAuthority::Independent { lane, .. } => {
                            format!("independent review by {}", lane_label(runtime, *lane))
                        }
                        CoverageAuthority::SelfReview { lane, .. } => {
                            format!("self-review by {}", lane_label(runtime, *lane))
                        }
                    };
                    let scopes = coverage.scopes.iter().map(ToString::to_string).collect::<Vec<_>>();
                    html! {
                        <li key={coverage.coverage.coverage.to_string()}>
                            {format!("{} · {by} · {} · ", coverage.coverage.coverage, scopes.join(", "))}
                            <code>{source_label(&coverage.source)}</code>
                        </li>
                    }
                })}
            </ul>
        </div>
    }
}

fn render_output(
    context: &Context<'_>,
    output: &TaskStamp,
    draft: &DecisionDraft,
    interactive: bool,
    pending: bool,
    edit: &impl Fn(DraftChange) -> DraftCommand,
) -> Html {
    let choose = {
        let output = output.clone();
        edit(Rc::new(move |draft| draft.toggle_output(output.clone())))
    };
    let checkbox = interactive && draft.choice() == DecisionChoice::Selected;
    let Some(task) = context.task(output) else {
        return html! {
            <li data-approval-output={output.to_string()} data-reference-state="unavailable">
                if checkbox {
                    <input
                        type="checkbox"
                        aria-label={format!("Select unavailable output {output}")}
                        checked={draft.selected(output)}
                        disabled={pending}
                        onchange={Callback::from(move |_: Event| choose())}
                    />
                }
                <code>{output.to_string()}</code>{" · unavailable on the current board"}
            </li>
        };
    };
    let statement = &task.statement.body;
    let standing = if statement.authorization.is_some() {
        "authorized".to_owned()
    } else {
        task.workflow.body.status.clone()
    };
    html! {
        <li data-approval-output={output.to_string()}>
            <label>
                if checkbox {
                    <input
                        type="checkbox"
                        checked={draft.selected(output)}
                        disabled={pending}
                        onchange={Callback::from(move |_: Event| choose())}
                    />
                }
                <strong>{task.presentation_title.clone()}</strong>
                <code>{statement.handle.to_string()}</code>
                <span>{standing}</span>
            </label>
            if !statement.acceptance.is_empty() {
                <ul class="room-approval-acceptance" aria-label="Acceptance">
                    {for statement.acceptance.iter().map(|criterion| html! { <li>{criterion.clone()}</li> })}
                </ul>
            } else {
                <p class="room-approval-acceptance">{"No acceptance criteria recorded."}</p>
            }
        </li>
    }
}

fn render_decision_history(
    decisions: &[&RoomPositioned<PreparationDecision>],
    authorizations: &[&RoomPositioned<ExecutionAuthorization>],
) -> Html {
    if decisions.is_empty() && authorizations.is_empty() {
        return html! { <p class="room-approval-history">{"Earlier decisions: none."}</p> };
    }
    let mut rows = decisions
        .iter()
        .map(|decision| {
            (
                decision.position,
                format!(
                    "Decision {} · {} · {}",
                    decision.value.decision.decision,
                    outcome_label(&decision.value.outcome),
                    source_label(&decision.value.source),
                ),
            )
        })
        .chain(authorizations.iter().map(|authorization| {
            (
                authorization.position,
                format!(
                    "Authorization for decision {} · {} output(s): {}",
                    authorization.value.decision.decision,
                    authorization.value.selected_outputs.len(),
                    authorization
                        .value
                        .selected_outputs
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            )
        }))
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| row.0);
    html! {
        <div class="room-approval-history">
            <strong>{"Decision history"}</strong>
            <ol>
                {for rows.into_iter().map(|(position, text)| html! {
                    <li key={position.to_string()}><code>{position.to_string()}</code>{" · "}{text}</li>
                })}
            </ol>
        </div>
    }
}

fn render_controls(
    context: &Context<'_>,
    positioned: &RoomPositioned<RetainedCandidate>,
    draft: &DecisionDraft,
    edit: &impl Fn(DraftChange) -> DraftCommand,
) -> Html {
    let candidate = &positioned.value;
    let pending = draft.is_pending();
    let choose = |choice| edit(Rc::new(move |draft| draft.set_choice(choice)));
    let selected = choose(DecisionChoice::Selected);
    let returned = choose(DecisionChoice::Returned);
    let collapse = choose(DecisionChoice::Collapse);
    let reason = {
        let (composer, candidate) = (
            context.controls.decision.clone(),
            candidate.candidate.clone(),
        );
        Callback::from(move |event: InputEvent| {
            let Some(value) = event
                .target()
                .and_then(|target| target.dyn_into::<HtmlInputElement>().ok())
                .map(|input| input.value())
            else {
                return;
            };
            composer.dispatch(ComposerEdit {
                candidate: candidate.clone(),
                settles: false,
                change: Box::new(move |draft| draft.set_reason(value)),
            });
        })
    };
    let mut previewed = draft.clone();
    let next_decision = SequenceCode::new(
        context
            .preparation
            .decisions
            .keys()
            .filter(|decision| decision.controller == candidate.candidate.controller)
            .map(|decision| decision.decision.get())
            .max()
            .unwrap_or(0)
            .saturating_add(1),
    );
    previewed.preview(
        context
            .controller
            .expect("interactive control has an open Design"),
        positioned.position,
        candidate,
        next_decision,
        context.tasks,
    );
    let preview = {
        let previewed = previewed.clone();
        edit(Rc::new(move |draft| *draft = previewed.clone()))
    };
    html! {
        <div class="room-decision" data-proposal-decision={candidate.candidate.candidate.to_string()}>
            <fieldset class="room-decision-choices" disabled={pending}>
                <legend>{"Decision"}</legend>
                <label>
                    <input
                        type="radio"
                        name={format!("proposal-decision-{}", candidate.candidate.candidate)}
                        checked={draft.choice() == DecisionChoice::Selected}
                        onchange={Callback::from(move |_: Event| selected())}
                    />
                    {"Select outputs"}
                </label>
                <label>
                    <input
                        type="radio"
                        name={format!("proposal-decision-{}", candidate.candidate.candidate)}
                        checked={draft.choice() == DecisionChoice::Returned}
                        onchange={Callback::from(move |_: Event| returned())}
                    />
                    {"Return"}
                </label>
                <label>
                    <input
                        type="radio"
                        name={format!("proposal-decision-{}", candidate.candidate.candidate)}
                        checked={draft.choice() == DecisionChoice::Collapse}
                        onchange={Callback::from(move |_: Event| collapse())}
                    />
                    {"Collapse"}
                </label>
            </fieldset>
            if matches!(draft.choice(), DecisionChoice::Returned | DecisionChoice::Collapse) {
                <label class="room-decision-reason">
                    <span>{if draft.choice() == DecisionChoice::Returned { "Return reason" } else { "Collapse reason" }}</span>
                    <input
                        type="text"
                        value={draft.reason().to_owned()}
                        oninput={reason}
                        disabled={pending}
                        placeholder={if draft.choice() == DecisionChoice::Returned {
                            "What another proposal must change"
                        } else {
                            "Why the Design premise did not hold"
                        }}
                    />
                </label>
            }
            <div class="room-inline-actions">
                <button
                    type="button"
                    onclick={Callback::from(move |_: MouseEvent| preview())}
                    disabled={pending}
                >
                    {"Preview decision"}
                </button>
            </div>
            {render_phase(context, draft, edit)}
        </div>
    }
}

fn render_phase(
    context: &Context<'_>,
    draft: &DecisionDraft,
    edit: &impl Fn(DraftChange) -> DraftCommand,
) -> Html {
    match draft.phase() {
        DraftPhase::Editing => Html::default(),
        DraftPhase::Pending(_) => html! {
            <p class="room-notice" role="status" aria-busy="true" data-decision-pending="">
                {"Submitting the exact decision…"}
            </p>
        },
        DraftPhase::Previewing(submission) => {
            let effect = match &submission.request.decision.outcome {
                DecisionOutcome::Selected {
                    selected_outputs, ..
                } => format!(
                    "Release only the {} checked output(s): {}. Unchecked outputs stay gated, and the Design remains open.",
                    selected_outputs.len(),
                    selected_outputs
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
                DecisionOutcome::Returned { reason, .. } => format!(
                    "Return this exact proposal version for another preparation pass: {reason}. No output is released, and the Design remains open."
                ),
                DecisionOutcome::Collapse { reason, .. } => format!(
                    "Collapse the Design on this proposal's premise: {reason}. No output is released."
                ),
            };
            let submit = submit(
                context.controls,
                context.sender,
                draft.candidate(),
                submission,
            );
            let cancel = edit(Rc::new(DecisionDraft::revise));
            html! {
                <div class="room-decision-preview" data-decision-preview={outcome_key(&submission.request.decision.outcome)}>
                    <p>{effect}</p>
                    <div class="room-inline-actions">
                        <button type="button" onclick={submit}>{"Submit decision"}</button>
                        <button type="button" onclick={Callback::from(move |_: MouseEvent| cancel())}>{"Cancel"}</button>
                    </div>
                </div>
            }
        }
    }
}

fn submit(
    controls: &Controls,
    sender: &SenderHandle,
    candidate: &CandidateRef,
    submission: &Submission,
) -> Callback<MouseEvent> {
    let (composer, deciding, sender) = (
        controls.decision.clone(),
        Rc::clone(&controls.deciding),
        sender.clone(),
    );
    let (candidate, submission) = (candidate.clone(), submission.clone());
    Callback::from(move |_: MouseEvent| {
        if !deciding.borrow_mut().insert(candidate.clone()) {
            return;
        }
        composer.dispatch(ComposerEdit {
            candidate: candidate.clone(),
            settles: false,
            change: Box::new(|draft| {
                draft.begin_submit();
            }),
        });
        let settle = {
            let (composer, deciding, candidate) =
                (composer.clone(), Rc::clone(&deciding), candidate.clone());
            move |outcome: Result<TextAnswer, String>| {
                deciding.borrow_mut().remove(&candidate);
                composer.dispatch(ComposerEdit {
                    candidate: candidate.clone(),
                    settles: true,
                    change: Box::new(move |draft| draft.settle(outcome)),
                });
            }
        };
        let action = ClientAction::Preparation {
            request: PreparationRequest::Decide {
                request: submission.request.clone(),
                expected: submission.expected.clone(),
            },
            respond: Rc::new(settle.clone()),
        };
        if !admit_action(&sender, action) {
            settle(Err("The decision could not be queued.".to_owned()));
        }
    })
}

fn render_notice(notice: Option<&DraftNotice>) -> Html {
    match notice {
        None => Html::default(),
        Some(DraftNotice::Refused(reason)) => html! {
            <p class="room-notice room-decision-refusal" role="alert" data-decision-refusal="">
                {reason.clone()}
            </p>
        },
        Some(DraftNotice::Recorded(answer)) => html! {
            <p class="room-notice" role="status" data-decision-recorded="">
                {format!("{answer}. Standing updates only when committed authority arrives.")}
            </p>
        },
    }
}

fn standing<'a>(preparation: &'a PreparationProjection, candidate: &CandidateRef) -> Standing<'a> {
    if let Some(authorized) = preparation
        .authorizations
        .values()
        .filter(|authorized| authorized.value.candidate == *candidate)
        .max_by_key(|authorized| authorized.position)
    {
        return Standing::Authorized(&authorized.value);
    }
    let latest = preparation
        .decisions
        .values()
        .filter(|decision| decision.value.outcome.candidate() == candidate)
        .max_by_key(|decision| decision.position);
    match latest.map(|decision| &decision.value) {
        None => Standing::Undecided,
        Some(PreparationDecision {
            decision,
            outcome: DecisionOutcome::Selected { .. },
            ..
        }) => Standing::Selected(decision),
        Some(PreparationDecision {
            outcome: DecisionOutcome::Returned { .. },
            ..
        }) => Standing::Returned,
        Some(PreparationDecision {
            outcome: DecisionOutcome::Collapse { .. },
            ..
        }) => Standing::Collapse,
    }
}

fn outcome_label(outcome: &DecisionOutcome) -> String {
    match outcome {
        DecisionOutcome::Selected {
            selected_outputs, ..
        } => format!(
            "selected {} output(s): {}",
            selected_outputs.len(),
            selected_outputs
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        DecisionOutcome::Returned { reason, .. } => format!("returned: {reason}"),
        DecisionOutcome::Collapse { reason, .. } => format!("collapsed: {reason}"),
    }
}

fn source_label(source: &RoomSource) -> String {
    match source {
        RoomSource::Transcript(hunk) => format!(
            "transcript member {} offset {} event {} bytes {}..{}",
            hunk.locus.member, hunk.locus.offset, hunk.locus.event, hunk.text.start, hunk.text.end,
        ),
        RoomSource::Journal { fact } => format!("journal fact {fact}"),
        RoomSource::Artifact { digest } => format!("artifact {digest}"),
    }
}

const fn outcome_key(outcome: &DecisionOutcome) -> &'static str {
    match outcome {
        DecisionOutcome::Selected { .. } => "selected",
        DecisionOutcome::Returned { .. } => "returned",
        DecisionOutcome::Collapse { .. } => "collapse",
    }
}
