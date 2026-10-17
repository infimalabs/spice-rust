//! Every immutable Design pass proposal and the operator's exact decision
//! surface. Work verdicts, outputless acceptance, and material returns are
//! previewed before one atomic `preparation decide`.
//! A version its author withdrew stays in history, apart from the open
//! versions, and offers no decision.

use std::collections::BTreeSet;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;
use wire::intent::{BoardTask, TextAnswer};
use wire::rooms::{
    CandidateCoverage, CandidateCritique, CandidateRef, CandidateReference, CoverageAuthority,
    DecisionOutcome, DesignPassProposal, DesignPassRecommendation, ExecutionAuthorization,
    OutputVerdict, PreparationDecision, RoomAuthorityStamp, RoomSource,
};
use wire::{CommandKind, SequenceCode, TaskStamp};
use yew::prelude::*;

use super::{Controls, conversation::lane_label};
use crate::AppRuntime;
use crate::browser::clock::Age;
use crate::browser::sender::{SenderHandle, admit_action};
use crate::client_action::{ClientAction, PreparationRequest};
use crate::preparation_decision::{
    DecisionChoice, DecisionComposer, DecisionDraft, DraftNotice, DraftPhase, Submission,
};
use crate::runtime::{CandidateWithdrawalRecord, PreparationProjection, RoomPositioned};

type DraftChange = Rc<dyn Fn(&mut DecisionDraft)>;
type DraftCommand = Box<dyn Fn()>;

/// This page's exact-version decision drafts. One pending version globally
/// locks submission until its response settles.
#[derive(Clone, Default, PartialEq)]
pub(super) struct Composer(DecisionComposer);

/// One edit to one version. A late response cannot settle another version.
pub(super) struct ComposerEdit {
    candidate: CandidateRef,
    settles: bool,
    change: Box<dyn FnOnce(&mut DecisionDraft)>,
}

impl Reducible for Composer {
    type Action = ComposerEdit;

    fn reduce(self: Rc<Self>, action: ComposerEdit) -> Rc<Self> {
        let mut composer = Rc::unwrap_or_clone(self).0;
        composer.apply(action.candidate, action.settles, action.change);
        Rc::new(Self(composer))
    }
}

/// Where one version stands under committed authority.
enum Standing<'a> {
    Undecided,
    Returned,
    Accepted(&'a PreparationDecision),
    Resolved(&'a PreparationDecision),
    Authorized(&'a ExecutionAuthorization),
    Withdrawn,
}

impl Standing<'_> {
    fn label(&self) -> String {
        match self {
            Self::Undecided => "Undecided".to_owned(),
            Self::Returned => "Returned".to_owned(),
            Self::Accepted(decision) => {
                format!("Accepted by decision {}", decision.decision.decision)
            }
            Self::Resolved(decision) => {
                let accepted = decision.outcome.accepted_outputs().count();
                format!(
                    "Resolved {accepted} accepted output(s) by decision {}",
                    decision.decision.decision
                )
            }
            Self::Authorized(authorization) => format!(
                "Accepted {} output(s) by decision {}",
                authorization.selected_outputs.len(),
                authorization.decision.decision
            ),
            Self::Withdrawn => "Withdrawn by its author".to_owned(),
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
            && self.runtime.principal().is_human()
            && self.runtime.offers(CommandKind::PreparationDecide)
    }
}

/// Every Design pass proposal under one Design, with controls only for an
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
    let synthetic = opportunistic_candidate(&board.tasks, preparation, design);
    let mut candidates = preparation
        .candidates
        .values()
        .filter(|positioned| positioned.value.candidate.controller == *design)
        .chain(synthetic.iter())
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| candidate.position);
    if candidates.is_empty() {
        return Html::default();
    }
    let (withdrawn, open): (Vec<_>, Vec<_>) = candidates.into_iter().partition(|candidate| {
        preparation
            .withdrawals
            .contains_key(&candidate.value.candidate)
    });
    let opportunistic = board.row(design).is_some_and(|task| {
        task.statement.body.intent == Some(wire::TaskIntent::OpportunisticDesign)
    });
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
                <span>{open.len()}</span>
            </header>
            if opportunistic {
                <p class="room-notice">
                    {"Every candidate is immutable. Every displayed line starts rejected; accept the lines that should receive authority. The complete decision is previewed before it records authority."}
                </p>
            } else {
                <p class="room-notice">
                    {"Every proposal is immutable. Work outputs start rejected; select Accept only for outputs that should receive authority. Accept the exact recommendation or return it with material feedback; each decision is previewed before it records authority."}
                </p>
            }
            {for open.into_iter().rev().map(|candidate| render_candidate(&context, candidate))}
            if !withdrawn.is_empty() {
                <section class="room-approval-withdrawn" aria-label="Withdrawn versions" data-room-withdrawn="">
                    <header>
                        <strong>{"Withdrawn versions"}</strong>
                        <span>{withdrawn.len()}</span>
                    </header>
                    <p class="room-notice">
                        {"Their authors withdrew these versions before any decision. They stay in history and accept no decision."}
                    </p>
                    {for withdrawn.into_iter().rev().map(|candidate| render_candidate(&context, candidate))}
                </section>
            }
        </section>
    }
}

fn opportunistic_candidate(
    tasks: &[BoardTask],
    preparation: &PreparationProjection,
    design: &TaskStamp,
) -> Option<RoomPositioned<DesignPassProposal>> {
    let controller = tasks.iter().find(|task| {
        task.statement.body.born == *design
            && task.statement.body.intent == Some(wire::TaskIntent::OpportunisticDesign)
            && task
                .design_standing()
                .is_some_and(|standing| standing.is_open())
    })?;
    let governed = preparation
        .decisions
        .values()
        .filter(|decision| decision.value.decision.controller == *design)
        .flat_map(|decision| match &decision.value.outcome {
            DecisionOutcome::Resolved { outputs, .. } => {
                outputs.iter().map(|output| output.output.clone()).collect()
            }
            DecisionOutcome::Returned { .. } | DecisionOutcome::Accepted { .. } => Vec::new(),
        })
        .collect::<BTreeSet<_>>();
    let mut lines = tasks
        .iter()
        .filter(|task| {
            task.workflow.body.status.is_pending()
                && task.statement.body.intent == Some(wire::TaskIntent::Plan)
                && task.statement.body.dependencies.contains(design)
                && task
                    .statement
                    .body
                    .preparation
                    .as_ref()
                    .is_some_and(|association| {
                        association.parent == *design
                            && association.controller == *design
                            && association.construction == task.statement.body.born
                    })
                && !governed.contains(&task.statement.body.born)
        })
        .collect::<Vec<_>>();
    lines.sort_by(|left, right| {
        left.statement
            .body
            .born
            .birth_cmp(&right.statement.body.born)
    });
    let newest = lines.last()?;
    let candidate = RoomAuthorityStamp::for_micros(newest.statement.body.born.micros())
        .expect("a task birth instant is a room authority instant");
    let position = lines
        .iter()
        .map(|line| line.statement.position)
        .max()
        .expect("a candidate has at least one line");
    Some(RoomPositioned {
        position,
        value: DesignPassProposal {
            candidate: CandidateRef {
                controller: design.clone(),
                candidate,
            },
            statement: controller.statement.position,
            claim: None,
            recommendation: DesignPassRecommendation::Work,
            governed_inputs: Vec::new(),
            outputs: lines
                .iter()
                .map(|line| line.statement.body.born.clone())
                .collect(),
            justification:
                "Resolve each pending unexpected-behavior line item through an explicit verdict."
                    .to_owned(),
            references: lines
                .iter()
                .map(|line| CandidateReference {
                    relation: "input".parse().expect("input is a wire name"),
                    source: RoomSource::Journal {
                        fact: SequenceCode::new(line.statement.position.get()),
                    },
                })
                .collect(),
            evidence: Vec::new(),
        },
    })
}

fn render_candidate(
    context: &Context<'_>,
    positioned: &RoomPositioned<DesignPassProposal>,
) -> Html {
    let candidate = &positioned.value;
    let empty = DecisionDraft::new(candidate.candidate.clone());
    let composer = &context.controls.decision.0;
    let draft = composer.draft(&candidate.candidate).unwrap_or(&empty);
    let locked = composer.is_pending();
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
    let withdrawal = context.preparation.withdrawals.get(&candidate.candidate);
    let interactive =
        context.interactive() && withdrawal.is_none() && matches!(&standing, Standing::Undecided);
    let edit = editor(context.controls, &candidate.candidate);
    let claim = candidate.claim.map_or_else(
        || html! { <span>{" · historical claim not witnessed"}</span> },
        |claim| html! { <span>{" · claim "}<code>{claim.to_string()}</code></span> },
    );
    html! {
        <article
            class="room-card room-approval-card"
            key={code.clone()}
            data-candidate={code.clone()}
            data-fact={fact.to_string()}
            tabindex="0"
        >
            <header>
                <strong>{format!("Proposal {code}")}</strong>
                <span data-approval-standing="">{standing.label()}</span>
                <code title="Retaining journal fact">{fact.to_string()}</code>
            </header>
            {withdrawal.map_or_else(Html::default, |withdrawal| render_withdrawal(context.runtime, withdrawal))}
            <p class="room-approval-proposal">
                <strong>{recommendation_label(candidate.recommendation)}</strong>
                {" · "}{candidate.justification.clone()}
                {" · statement "}<code>{candidate.statement.to_string()}</code>
                {claim}
            </p>
            {render_inputs(context, candidate)}
            {render_references(candidate)}
            {render_evidence(context, candidate)}
            {render_critiques(&critiques)}
            {render_coverage(context.runtime, &coverage)}
            <ul class="room-approval-outputs" aria-label="Proposed outputs">
                {for candidate.outputs.iter().map(|output| {
                    render_output(context, output, draft, interactive, locked, &edit)
                })}
            </ul>
            {render_decision_history(&decisions, &authorizations)}
            if interactive {
                {render_controls(context, positioned, draft, locked, &edit)}
                if locked && !pending {
                    <p class="room-notice" role="status">
                        {"Another proposal decision is pending; wait for it to settle before editing this version."}
                    </p>
                }
            } else if withdrawal.is_none() && context.controller.is_none() {
                <p class="room-notice">{"This Design is concluded; its proposal history is read-only."}</p>
            }
            else if !matches!(&standing, Standing::Undecided | Standing::Withdrawn) {
                <p class="room-notice">{"This proposal version already has a recorded decision; its controls are read-only."}</p>
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

fn render_inputs(context: &Context<'_>, candidate: &DesignPassProposal) -> Html {
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

fn render_references(candidate: &DesignPassProposal) -> Html {
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

fn render_evidence(context: &Context<'_>, candidate: &DesignPassProposal) -> Html {
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
    locked: bool,
    edit: &impl Fn(DraftChange) -> DraftCommand,
) -> Html {
    let accepted = {
        let output = output.clone();
        edit(Rc::new(move |draft| {
            draft.set_output_verdict(output.clone(), OutputVerdict::Accepted);
        }))
    };
    let rejected = {
        let output = output.clone();
        edit(Rc::new(move |draft| {
            draft.set_output_verdict(output.clone(), OutputVerdict::Rejected);
        }))
    };
    let comment = {
        let (composer, candidate, output) = (
            context.controls.decision.clone(),
            draft.candidate().clone(),
            output.clone(),
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
                change: Box::new({
                    let output = output.clone();
                    move |draft| draft.set_output_comment(output, value)
                }),
            });
        })
    };
    let decisions = interactive && draft.choice() == DecisionChoice::Accept;
    let controls = decisions.then(|| html! {
        <div class="room-output-decision">
            <div
                class="room-output-verdict"
                role="group"
                aria-label={format!("Verdict for output {output}")}
            >
                <label>
                    <input
                        type="radio"
                        name={format!("output-verdict-{}-{output}", draft.candidate().candidate)}
                        checked={draft.verdict(output) == OutputVerdict::Accepted}
                        disabled={locked}
                        onchange={Callback::from(move |_: Event| accepted())}
                    />
                    {"Accept"}
                </label>
                <label>
                    <input
                        type="radio"
                        name={format!("output-verdict-{}-{output}", draft.candidate().candidate)}
                        checked={draft.verdict(output) == OutputVerdict::Rejected}
                        disabled={locked}
                        onchange={Callback::from(move |_: Event| rejected())}
                    />
                    {"Reject"}
                </label>
            </div>
            <label class="room-output-comment">
                <span>{"Comment (optional)"}</span>
                <input
                    type="text"
                    value={draft.output_comment(output).to_owned()}
                    oninput={comment}
                    disabled={locked}
                    placeholder="Why this output is accepted or rejected"
                />
            </label>
        </div>
    });
    let Some(task) = context.task(output) else {
        return html! {
            <li data-approval-output={output.to_string()} data-reference-state="unavailable">
                {controls}
                <code>{output.to_string()}</code>{" · unavailable on the current board"}
            </li>
        };
    };
    let statement = &task.statement.body;
    let standing = output_standing(task);
    html! {
        <li data-approval-output={output.to_string()}>
            <div class="room-approval-output-title">
                <strong>{task.presentation_title.clone()}</strong>
                <code>{statement.handle.to_string()}</code>
                <span>{standing}</span>
            </div>
            if let Some(description) = statement.description.as_deref() {
                <p class="room-approval-output-statement">{description}</p>
            }
            {controls}
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

fn output_standing(task: &BoardTask) -> &str {
    let statement = &task.statement.body;
    if statement.preparation.as_ref().is_some_and(|preparation| {
        preparation.construction == statement.born
            && !statement.dependencies.contains(&preparation.parent)
    }) {
        "authorized"
    } else {
        task.workflow.body.status.as_str()
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
                    <li key={position.to_string()} data-fact={SequenceCode::new(position.get()).to_string()} tabindex="-1"><code>{position.to_string()}</code>{" · "}{text}</li>
                })}
            </ol>
        </div>
    }
}

fn render_controls(
    context: &Context<'_>,
    positioned: &RoomPositioned<DesignPassProposal>,
    draft: &DecisionDraft,
    locked: bool,
    edit: &impl Fn(DraftChange) -> DraftCommand,
) -> Html {
    let candidate = &positioned.value;
    let opportunistic = candidate.claim.is_none()
        && context.controller.is_some_and(|controller| {
            controller.statement.body.intent == Some(wire::TaskIntent::OpportunisticDesign)
        });
    let choose = |choice| edit(Rc::new(move |draft| draft.set_choice(choice)));
    let accept = choose(DecisionChoice::Accept);
    let return_with_feedback = choose(DecisionChoice::Return);
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
    let show_reason = draft.choice() == DecisionChoice::Return
        || candidate.recommendation == DesignPassRecommendation::Work;
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
            <fieldset class="room-decision-choices" disabled={locked}>
                <legend>{"Decision"}</legend>
                <label>
                    <input
                        type="radio"
                        name={format!("proposal-decision-{}", candidate.candidate.candidate)}
                        checked={draft.choice() == DecisionChoice::Accept}
                        onchange={Callback::from(move |_: Event| accept())}
                    />
                    {if opportunistic {"Resolve line items"} else {"Accept proposal"}}
                </label>
                if !opportunistic {
                    <label>
                        <input
                            type="radio"
                            name={format!("proposal-decision-{}", candidate.candidate.candidate)}
                            checked={draft.choice() == DecisionChoice::Return}
                            onchange={Callback::from(move |_: Event| return_with_feedback())}
                        />
                        {"Return with feedback"}
                    </label>
                }
            </fieldset>
            if show_reason {
                <label class="room-decision-reason">
                    <span>{if draft.choice() == DecisionChoice::Accept {
                        "Overall comment (optional)"
                    } else {
                        "Material feedback"
                    }}</span>
                    <input
                        type="text"
                        value={draft.reason().to_owned()}
                        oninput={reason}
                        disabled={locked}
                        placeholder={if draft.choice() == DecisionChoice::Accept {
                            "Comment on the proposal as a whole"
                        } else {
                            "What must change in the next Design pass"
                        }}
                    />
                </label>
            }
            <div class="room-inline-actions">
                <button
                    type="button"
                    onclick={Callback::from(move |_: MouseEvent| preview())}
                    disabled={locked}
                >
                    {"Preview decision"}
                </button>
            </div>
            {render_phase(context, candidate, draft, edit)}
        </div>
    }
}

fn render_phase(
    context: &Context<'_>,
    candidate: &DesignPassProposal,
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
                DecisionOutcome::Accepted { .. } => {
                    let action = match candidate.recommendation {
                        DesignPassRecommendation::Conclude => "Complete",
                        DesignPassRecommendation::Collapse => "Collapse",
                        DesignPassRecommendation::Work => {
                            unreachable!("work proposals resolve their outputs")
                        }
                    };
                    let closed = context
                        .controller
                        .map(|controller| controller.controlled.as_slice())
                        .unwrap_or_default();
                    if closed.is_empty() {
                        format!(
                            "{action} the Design exactly as proposed. No pending controlled task needs closure."
                        )
                    } else {
                        format!(
                            "{action} the Design exactly as proposed. This also closes: {}.",
                            closed
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join(", "),
                        )
                    }
                }
                DecisionOutcome::Returned { reason, .. } => format!(
                    "Return this exact proposal version for another preparation pass: {reason}. No output is released, and the Design remains open."
                ),
                DecisionOutcome::Resolved {
                    outputs, comment, ..
                } => {
                    let accepted = outputs
                        .iter()
                        .filter(|output| output.verdict == OutputVerdict::Accepted)
                        .count();
                    let rejected = outputs.len() - accepted;
                    format!(
                        "Record {accepted} accepted and {rejected} rejected output(s). Accepted outputs are released; rejected outputs stay gated. Overall comment: {}",
                        comment.as_deref().unwrap_or("none"),
                    )
                }
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
        let mut in_flight = deciding.borrow_mut();
        if !in_flight.is_empty() {
            return;
        }
        in_flight.insert(candidate.clone());
        drop(in_flight);
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

/// One author's recorded withdrawal: who withdrew the version, when, and why.
fn render_withdrawal(
    runtime: &AppRuntime,
    withdrawal: &RoomPositioned<CandidateWithdrawalRecord>,
) -> Html {
    let record = &withdrawal.value;
    let author = runtime.room_projection().author(&record.author);
    let author = match author {
        wire::Actor::Lane(lane) => lane_label(runtime, lane),
        other => crate::work::owner_label(&other, None, runtime.principals()),
    };
    let fact = SequenceCode::new(withdrawal.position.get());
    html! {
        <p class="room-approval-withdrawal" data-approval-withdrawal="">
            {format!("Withdrawn by {author} ")}
            <Age
                authored={record.at.get().as_millisecond()}
                class="relative-age"
                title={fact.to_string()}
            />
            {": "}{record.withdrawal.reason.clone()}
            {" "}<code title="Withdrawing journal fact">{fact.to_string()}</code>
        </p>
    }
}

fn standing<'a>(preparation: &'a PreparationProjection, candidate: &CandidateRef) -> Standing<'a> {
    if preparation.withdrawals.contains_key(candidate) {
        return Standing::Withdrawn;
    }
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
        Some(
            decision @ PreparationDecision {
                outcome: DecisionOutcome::Accepted { .. },
                ..
            },
        ) => Standing::Accepted(decision),
        Some(PreparationDecision {
            outcome: DecisionOutcome::Returned { .. },
            ..
        }) => Standing::Returned,
        Some(
            decision @ PreparationDecision {
                outcome: DecisionOutcome::Resolved { .. },
                ..
            },
        ) => Standing::Resolved(decision),
    }
}

fn outcome_label(outcome: &DecisionOutcome) -> String {
    match outcome {
        DecisionOutcome::Accepted { .. } => "accepted proposal".to_owned(),
        DecisionOutcome::Returned { reason, .. } => format!("returned: {reason}"),
        DecisionOutcome::Resolved {
            outputs, comment, ..
        } => {
            let outputs = outputs
                .iter()
                .map(|output| {
                    let verdict = match output.verdict {
                        OutputVerdict::Accepted => "accepted",
                        OutputVerdict::Rejected => "rejected",
                    };
                    match &output.comment {
                        Some(comment) => format!("{} {verdict}: {comment}", output.output),
                        None => format!("{} {verdict}", output.output),
                    }
                })
                .collect::<Vec<_>>()
                .join("; ");
            match comment {
                Some(comment) => format!("resolved {outputs}; overall: {comment}"),
                None => format!("resolved {outputs}"),
            }
        }
    }
}

const fn recommendation_label(recommendation: DesignPassRecommendation) -> &'static str {
    match recommendation {
        DesignPassRecommendation::Work => "Work",
        DesignPassRecommendation::Conclude => "Conclude",
        DesignPassRecommendation::Collapse => "Collapse",
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
        RoomSource::Contribution { room, publication } => {
            format!("contribution {publication} in room {room}")
        }
    }
}

const fn outcome_key(outcome: &DecisionOutcome) -> &'static str {
    match outcome {
        DecisionOutcome::Accepted { .. } => "accepted",
        DecisionOutcome::Returned { .. } => "returned",
        DecisionOutcome::Resolved { .. } => "resolved",
    }
}
