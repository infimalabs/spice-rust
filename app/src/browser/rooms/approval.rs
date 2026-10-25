//! Every immutable Design pass proposal and the operator's exact decision
//! surface. Work verdicts, outputless acceptance, and material returns are
//! previewed before one atomic `preparation decide`.
//! A version its author withdrew stays in history, apart from the open
//! versions, and offers no decision.

use std::borrow::Cow;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;
use wire::intent::{BoardTask, TextAnswer};
use wire::rooms::{
    CandidateCoverage, CandidateCritique, CandidateRef, CoverageAuthority, DecisionOutcome,
    DesignPassProposal, DesignPassRecommendation, ExecutionAuthorization, OutputVerdict,
    PreparationDecision, RoomSource,
};
use wire::{CommandKind, SequenceCode, TaskStamp};
use yew::prelude::*;

use super::standing::focus_in_place;
use super::{Controls, OpenTask, card, conversation::lane_label, task_handle_link};
use crate::AppRuntime;
use crate::browser::clock::Age;
use crate::browser::sender::{SenderHandle, admit_action};
use crate::client_action::{ClientAction, PreparationRequest};
use crate::preparation_decision::{
    DecisionComposer, DecisionDraft, DraftNotice, DraftPhase, Submission,
};
use crate::runtime::{CandidateWithdrawalRecord, PreparationProjection, RoomPositioned};
use crate::task_sheet::ProposalLine;

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

/// Where one version stands under committed decision authority.
enum Standing<'a> {
    Undecided,
    Returned,
    Accepted(&'a PreparationDecision),
    Resolved(&'a PreparationDecision),
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
            Self::Withdrawn => "Withdrawn by its author".to_owned(),
        }
    }
}

struct Context<'a> {
    runtime: &'a AppRuntime,
    controls: &'a Controls,
    sender: &'a SenderHandle,
    open_task: &'a OpenTask,
    preparation: &'a PreparationProjection,
    design: &'a TaskStamp,
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
    open_task: &OpenTask,
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
    let (withdrawn, open): (Vec<_>, Vec<_>) = candidates.into_iter().partition(|candidate| {
        preparation
            .withdrawals
            .contains_key(&candidate.value.candidate)
    });
    let context = Context {
        runtime,
        controls,
        sender,
        open_task,
        design,
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
                <h4>{"Proposal versions"}</h4>
                <span>{open.len()}</span>
            </header>

            {for open.into_iter().rev().map(|candidate| render_candidate(&context, candidate))}
            if !withdrawn.is_empty() {
                <section class="room-approval-withdrawn" aria-label="Withdrawn versions" data-room-withdrawn="">
                    <header>
                        <h4>{"Withdrawn versions"}</h4>
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

/// One recorded proposal as its room conversation shows it: who proposed
/// what, its decision standing as tab-style counts, links that focus the
/// version's decision surface on the Design's read view, and each output's
/// handle opening that task's sheet.
pub(super) fn render_proposal_card(
    runtime: &AppRuntime,
    open_task: &OpenTask,
    room: &TaskStamp,
    positioned: &RoomPositioned<DesignPassProposal>,
) -> Html {
    let code = positioned.value.candidate.candidate.to_string();
    html! {
        <article class="room-card" key={format!("proposal-{code}")} tabindex="0">
            {render_proposal_face(runtime, open_task, room, positioned)}
        </article>
    }
}

/// The proposal card's face, for a surface that supplies its own card.
pub(super) fn render_proposal_face(
    runtime: &AppRuntime,
    open_task: &OpenTask,
    room: &TaskStamp,
    positioned: &RoomPositioned<DesignPassProposal>,
) -> Html {
    let preparation = runtime.room_projection().preparation();
    let candidate = &positioned.value;
    let code = candidate.candidate.candidate.to_string();
    let record = preparation.proposal_records.get(&candidate.candidate);
    let speaker = record.map(
        |record| match runtime.room_projection().author(&record.author) {
            wire::Actor::Lane(lane) => card::Speaker {
                name: lane_label(runtime, lane),
                lane: Some(lane),
            },
            other => card::Speaker {
                name: crate::work::owner_label(&other, None, runtime.principals()),
                lane: None,
            },
        },
    );
    let tasks = runtime
        .board()
        .map_or(&[][..], |board| board.tasks.as_slice());
    let link = |selector: String, label: Html| {
        html! {
            <a href={crate::view::design_href(room)} onclick={focus_in_place(selector)}>{label}</a>
        }
    };
    let version = format!("[data-candidate=\"{code}\"]");
    let fact = SequenceCode::new(positioned.position.get()).to_string();
    let design_pitch = tasks
        .iter()
        .find(|task| task.statement.body.born == *room)
        .map_or("", |task| task.statement.body.pitch.as_str());
    let output_line = |stamp: &TaskStamp, selector: String| {
        tasks
            .iter()
            .find(|task| task.statement.body.born == *stamp)
            .map_or_else(
                || html! { <li>{link(selector.clone(), html! { <code>{stamp.to_string()}</code> })}</li> },
                |task| html! {
                    <li>
                        {task_handle_link(open_task, &task.statement.body.born, &task.statement.body.handle)}
                        {" "}{link(selector.clone(), html! { {task.statement.body.pitch.clone()} })}
                    </li>
                },
            )
    };
    let lines = crate::task_sheet::proposal_lines(candidate);
    let outputs = lines.iter().filter_map(|line| match line {
        ProposalLine::Output(output) => Some(output_line(
            output,
            format!("{version} [data-approval-output=\"{output}\"]"),
        )),
        _ => None,
    });
    let inputs = lines.iter().filter_map(|line| match line {
        ProposalLine::Input(input) => Some(output_line(input, version.clone())),
        _ => None,
    });
    let tail = html! {
        <div class="room-proposal" data-proposal-card={code.clone()}>
            <p class="room-proposal-standing">
                <span class="room-proposal-badges" role="group" aria-label="Decision">
                    {render_standing_badges(&standing(preparation, &candidate.candidate))}
                </span>
                {link(version.clone(), html! { <>{"Proposal "}<code>{code.clone()}</code></> })}
                {" · fact "}{link(version.clone(), html! { <code>{fact}</code> })}
            </p>
            if !candidate.outputs.is_empty() {
                <ul class="room-proposal-outputs" aria-label="Proposed outputs">{for outputs}</ul>
            }
            if !candidate.governed_inputs.is_empty() {
                <ul class="room-proposal-outputs" aria-label="Governed inputs">{for inputs}</ul>
            }
            {for lines.iter().filter_map(|line| match line {
                ProposalLine::References(count) => Some(render_reference_count(*count)),
                ProposalLine::Justification(justification) => Some(render_justification(justification)),
                _ => None,
            })}
        </div>
    };
    card::render(
        runtime,
        proposal_display_millis(preparation, positioned),
        card::Face {
            speaker,
            receivers: String::new(),
            badge: Some(card::Badge::Kind {
                kind: "proposal",
                label: "Proposal",
            }),
            words: format!(
                "{} · {design_pitch}",
                recommendation_label(candidate.recommendation),
            ),
            tail,
        },
        None,
    )
}

/// Retained references as their count alone; the decision surface keeps the
/// exact links.
fn render_reference_count(count: usize) -> Html {
    html! {
        <p class="room-approval-references">
            {"Retained references"}
            <span class="app-count-badge">{count}</span>
        </p>
    }
}

/// The authored justification, collapsed beneath everything the operator
/// decides on.
fn render_justification(justification: &str) -> Html {
    html! {
        <details class="room-approval-justification">
            <summary>{"Justification"}</summary>
            <p>{justification.to_owned()}</p>
        </details>
    }
}

/// Journal time shown by a proposal card and used to interleave it with other
/// activity; a candidate without its proposal record shows its stamp's time.
pub(super) fn proposal_display_millis(
    preparation: &PreparationProjection,
    positioned: &RoomPositioned<DesignPassProposal>,
) -> i64 {
    preparation
        .proposal_records
        .get(&positioned.value.candidate)
        .map_or_else(
            || positioned.value.candidate.candidate.micros() / 1_000,
            |record| record.at.get().as_millisecond(),
        )
}

/// What a proposal's card shape must change with: the version and its standing.
pub(super) fn proposal_shape(
    preparation: &PreparationProjection,
    positioned: &RoomPositioned<DesignPassProposal>,
) -> String {
    let standing = standing(preparation, &positioned.value.candidate).label();
    format!("{:?}|{standing}", positioned.value)
}

/// A decision's standing as tab-style badges; a resolved work proposal counts
/// its accepted and rejected outputs.
fn render_standing_badges(standing: &Standing<'_>) -> Html {
    let badge = |verdict: &'static str, label: &'static str, count: Option<usize>| {
        html! {
            <span class="room-proposal-badge" data-verdict={verdict}>
                {label}
                if let Some(count) = count {
                    <span class="app-count-badge">{count}</span>
                }
            </span>
        }
    };
    match standing {
        Standing::Undecided => badge("undecided", "Undecided", None),
        Standing::Withdrawn => badge("withdrawn", "Withdrawn", None),
        Standing::Returned => badge("returned", "Returned", None),
        Standing::Accepted(_) => badge("accepted", "Accepted", None),
        Standing::Resolved(decision) => {
            let DecisionOutcome::Resolved { outputs, .. } = &decision.outcome else {
                return Html::default();
            };
            let count = |verdict| {
                outputs
                    .iter()
                    .filter(|output| output.verdict == verdict)
                    .count()
            };
            html! {
                <>
                    {badge("accepted", "Accepted", Some(count(OutputVerdict::Accepted)))}
                    {badge("rejected", "Rejected", Some(count(OutputVerdict::Rejected)))}
                </>
            }
        }
    }
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
        |claim| html! { <span>{" · claim "}<code>{SequenceCode::new(claim.get()).to_string()}</code></span> },
    );
    let lines = crate::task_sheet::proposal_lines(candidate);
    html! {
        <article
            class="room-card room-approval-card"
            key={code.clone()}
            data-candidate={code.clone()}
            data-fact={fact.to_string()}
            tabindex="0"
        >
            <header>
                <h4>{format!("Proposal {code}")}</h4>
                <span data-approval-standing="">{standing.label()}</span>
                <code title="Retaining journal fact">{fact.to_string()}</code>
            </header>
            {withdrawal.map_or_else(Html::default, |withdrawal| render_withdrawal(context.runtime, withdrawal))}
            <div class="room-approval-proposal">
                <p class="room-approval-head">
                    <strong>{recommendation_label(candidate.recommendation)}</strong>
                    {" · "}{context.task(context.design).map_or("", |task| task.statement.body.pitch.as_str())}
                </p>
                <p class="room-approval-facts">
                    {"statement "}<code>{SequenceCode::new(candidate.statement.get()).to_string()}</code>
                    {claim}
                </p>
            </div>
            if interactive && !candidate.outputs.is_empty() {
                {render_every_output_verdict(candidate, locked || draft.returning(), &edit)}
            }
            <ul class="room-approval-outputs" aria-label="Proposed outputs">
                {for lines.iter().filter_map(|line| match line {
                    ProposalLine::Output(output) => {
                        Some(render_output(context, output, draft, interactive, locked, &edit))
                    }
                    _ => None,
                })}
            </ul>
            {render_inputs(context, &lines)}
            {for lines.iter().filter_map(|line| match line {
                ProposalLine::References(_) => Some(render_references(candidate)),
                ProposalLine::Justification(justification) => Some(render_justification(justification)),
                _ => None,
            })}
            {render_critiques(&critiques)}
            {render_coverage(context.runtime, &coverage)}
            {render_decision_history(context.runtime, &decisions, &authorizations)}
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

/// Governed inputs as output lines without a verdict: handle and pitch.
fn render_inputs(context: &Context<'_>, lines: &[ProposalLine<'_>]) -> Html {
    let inputs = lines.iter().filter_map(|line| match line {
        ProposalLine::Input(input) => Some(*input),
        _ => None,
    });
    if !lines
        .iter()
        .any(|line| matches!(line, ProposalLine::Input(_)))
    {
        return Html::default();
    }
    html! {
        <div class="room-approval-inputs">
            <strong>{"Governed inputs"}</strong>
            <ul class="room-approval-outputs" aria-label="Governed inputs">
            {for inputs.map(|input| match context.task(input) {
                Some(task) => html! {
                    <li key={input.to_string()}>
                        <div class="room-approval-output-title">
                            {task_handle_link(context.open_task, &task.statement.body.born, &task.statement.body.handle)}
                            <span>{task.statement.body.pitch.clone()}</span>
                        </div>
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

/// Retained references behind their count: the links stay exact and
/// unresolved, and nothing decided depends on opening them.
fn render_references(candidate: &DesignPassProposal) -> Html {
    html! {
        <details class="room-approval-references">
            <summary>
                {"Retained references"}
                <span class="app-count-badge">{candidate.references.len()}</span>
            </summary>
            <ul>
                {for candidate.references.iter().map(|reference| html! {
                    <li>
                        <span>{reference.relation.to_string()}</span>
                        {" · "}<code>{source_label(&reference.source)}</code>
                        <span class="room-reference-state">{"exact link; availability is not resolved by this view"}</span>
                    </li>
                })}
            </ul>
        </details>
    }
}

fn render_critiques(critiques: &[&RoomPositioned<CandidateCritique>]) -> Html {
    if critiques.is_empty() {
        return Html::default();
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
        return Html::default();
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

/// One control per verdict that marks every proposed output at once, so the
/// operator who read the list need not visit each line.
fn render_every_output_verdict(
    candidate: &DesignPassProposal,
    locked: bool,
    edit: &impl Fn(DraftChange) -> DraftCommand,
) -> Html {
    let all = |verdict: OutputVerdict| {
        let outputs = candidate.outputs.clone();
        let apply = edit(Rc::new(move |draft| {
            draft.set_every_output_verdict(outputs.iter().cloned(), verdict);
        }));
        Callback::from(move |_: MouseEvent| apply())
    };
    html! {
        <div
            class="room-inline-actions room-output-verdict-all"
            role="group"
            aria-label="Verdict for every output"
        >
            <button type="button" class="app-compact-control" disabled={locked} onclick={all(OutputVerdict::Accepted)}>
                {"Accept all"}
            </button>
            <button type="button" class="app-compact-control" disabled={locked} onclick={all(OutputVerdict::Rejected)}>
                {"Reject all"}
            </button>
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
        let output = output.clone();
        draft_text(context.controls, draft.candidate(), move |draft, value| {
            draft.set_output_comment(output.clone(), value);
        })
    };
    // Entered return feedback sets the verdicts aside without clearing them.
    let set_aside = locked || draft.returning();
    let controls = interactive.then(|| html! {
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
                        checked={draft.verdict(output) == Some(OutputVerdict::Accepted)}
                        disabled={set_aside}
                        onchange={Callback::from(move |_: Event| accepted())}
                    />
                    {"Accept"}
                </label>
                <label>
                    <input
                        type="radio"
                        name={format!("output-verdict-{}-{output}", draft.candidate().candidate)}
                        checked={draft.verdict(output) == Some(OutputVerdict::Rejected)}
                        disabled={set_aside}
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
                    disabled={set_aside}
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
    let kind = output_kind(task);
    let standing = output_standing(task, context.design);
    // The line the operator decides on: handle, pitch, kind and standing, then
    // its verdict control; a Plan's acceptance stays visible beneath because the
    // decision weighs it. The description never joins the line.
    html! {
        <li data-approval-output={output.to_string()}>
            <div class="room-approval-output-title">
                {task_handle_link(context.open_task, &task.statement.body.born, &task.statement.body.handle)}
                <span class="room-approval-output-pitch" title={task.presentation_title.clone()}>{statement.pitch.clone()}</span>
                <span data-approval-output-kind="">{kind}</span>
                <span>{standing}</span>
            </div>
            {controls}
            if !statement.acceptance.is_empty() {
                <ul class="room-approval-acceptance" aria-label="Acceptance">
                    {for statement.acceptance.iter().map(|criterion| html! { <li>{criterion.clone()}</li> })}
                </ul>
            }
        </li>
    }
}

fn output_kind(task: &BoardTask) -> Cow<'static, str> {
    match task.statement.body.intent {
        Some(wire::TaskIntent::Plan) => Cow::Borrowed("Plan"),
        Some(wire::TaskIntent::Design | wire::TaskIntent::OpportunisticDesign) => {
            Cow::Borrowed("Design")
        }
        None => {
            let mut label = String::from("Task");
            for (index, phase) in task.workflow.body.flow.iter().enumerate() {
                label.push_str(if index == 0 { " · " } else { " → " });
                label.push_str(phase.as_str());
            }
            Cow::Owned(label)
        }
    }
}

fn output_standing<'a>(task: &'a BoardTask, design: &TaskStamp) -> &'a str {
    if task.statement.body.dependencies.contains(design) {
        task.workflow.body.status.as_str()
    } else {
        "selected"
    }
}

fn render_decision_history(
    runtime: &AppRuntime,
    decisions: &[&RoomPositioned<PreparationDecision>],
    authorizations: &[&RoomPositioned<ExecutionAuthorization>],
) -> Html {
    if decisions.is_empty() && authorizations.is_empty() {
        return Html::default();
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
                decision_notes(runtime, &decision.value.outcome),
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
                Html::default(),
            )
        }))
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| row.0);
    html! {
        <div class="room-approval-history">
            <strong>{"Decision history"}</strong>
            <ol>
                {for rows.into_iter().map(|(position, text, notes)| {
                    let fact = SequenceCode::new(position.get()).to_string();
                    html! {
                        <li key={position.to_string()} data-fact={fact.clone()} tabindex="-1"><code>{fact}</code>{" · "}{text}{notes}</li>
                    }
                })}
            </ol>
        </div>
    }
}

/// Each input event's value applied to the draft of `candidate` through `set`.
fn draft_text(
    controls: &Controls,
    candidate: &CandidateRef,
    set: impl Fn(&mut DecisionDraft, String) + Clone + 'static,
) -> Callback<InputEvent> {
    let (composer, candidate) = (controls.decision.clone(), candidate.clone());
    Callback::from(move |event: InputEvent| {
        let Some(value) = event
            .target()
            .and_then(|target| target.dyn_into::<HtmlInputElement>().ok())
            .map(|input| input.value())
        else {
            return;
        };
        let set = set.clone();
        composer.dispatch(ComposerEdit {
            candidate: candidate.clone(),
            settles: false,
            change: Box::new(move |draft| set(draft, value)),
        });
    })
}

fn render_controls(
    context: &Context<'_>,
    positioned: &RoomPositioned<DesignPassProposal>,
    draft: &DecisionDraft,
    locked: bool,
    edit: &impl Fn(DraftChange) -> DraftCommand,
) -> Html {
    let candidate = &positioned.value;
    let comment = draft_text(
        context.controls,
        &candidate.candidate,
        DecisionDraft::set_comment,
    );
    let feedback = draft_text(
        context.controls,
        &candidate.candidate,
        DecisionDraft::set_feedback,
    );
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
            if candidate.recommendation == DesignPassRecommendation::Work {
                <label class="room-decision-reason">
                    <span>{"Overall comment (optional)"}</span>
                    <input
                        type="text"
                        value={draft.comment().to_owned()}
                        oninput={comment}
                        disabled={locked || draft.returning()}
                        placeholder="Comment on the proposal as a whole"
                    />
                </label>
            }
            <label class="room-decision-reason">
                <span>{"Return with feedback"}</span>
                <input
                    type="text"
                    value={draft.feedback().to_owned()}
                    oninput={feedback}
                    disabled={locked}
                    placeholder="What must change in the next Design pass; entering it returns this proposal"
                />
            </label>
            <div class="room-inline-actions">
                <button
                    type="button"
                    class="app-compact-control"
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
                    "Return with feedback: {reason}. This exact proposal version goes back for another preparation pass; no output is released, and the Design remains open."
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
                        <button type="button" class="app-compact-control" onclick={submit}>{"Submit decision"}</button>
                        <button type="button" class="app-compact-control" onclick={Callback::from(move |_: MouseEvent| cancel())}>{"Cancel"}</button>
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
                request: Box::new(submission.request.clone()),
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
        <div class="room-approval-withdrawal" data-approval-withdrawal="">
            <p>
                {format!("Withdrawn by {author} ")}
                <Age
                    authored={record.at.get().as_millisecond()}
                    class="relative-age"
                    title={fact.to_string()}
                />
                {" "}<code title="Withdrawing journal fact">{fact.to_string()}</code>
            </p>
            {super::card::render_words(runtime, &record.withdrawal.reason, None, None)}
        </div>
    }
}

fn standing<'a>(preparation: &'a PreparationProjection, candidate: &CandidateRef) -> Standing<'a> {
    if preparation.withdrawals.contains_key(candidate) {
        return Standing::Withdrawn;
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
        DecisionOutcome::Returned { .. } => "returned".to_owned(),
        DecisionOutcome::Resolved { outputs, .. } => format!(
            "resolved {}",
            outputs
                .iter()
                .map(|output| {
                    let verdict = match output.verdict {
                        OutputVerdict::Accepted => "accepted",
                        OutputVerdict::Rejected => "rejected",
                    };
                    format!("{} {verdict}", output.output)
                })
                .collect::<Vec<_>>()
                .join("; ")
        ),
    }
}

/// What a decision's author wrote: the return reason, each output's comment
/// under that output's code, then the overall comment.
fn decision_notes(runtime: &AppRuntime, outcome: &DecisionOutcome) -> Html {
    let notes = match outcome {
        DecisionOutcome::Accepted { .. } => Vec::new(),
        DecisionOutcome::Returned { reason, .. } => vec![(None, reason)],
        DecisionOutcome::Resolved {
            outputs, comment, ..
        } => outputs
            .iter()
            .filter_map(|output| {
                output
                    .comment
                    .as_ref()
                    .map(|comment| (Some(output.output.to_string()), comment))
            })
            .chain(
                comment
                    .iter()
                    .map(|comment| (Some("Overall".to_owned()), comment)),
            )
            .collect(),
    };
    html! {
        {for notes.into_iter().map(|(label, words)| html! {
            <div class="room-approval-note">
                if let Some(label) = label {
                    <strong>{label}</strong>
                }
                {super::card::render_words(runtime, words, None, None)}
            </div>
        })}
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
