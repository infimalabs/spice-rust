//! Read every retained candidate and its durable standing, and record review
//! coverage as independent evidence. Exact selection belongs to the preparation
//! decision surface.

use std::rc::Rc;

use wire::intent::{BoardTask, TextAnswer};
use wire::rooms::{
    CandidateCoverage, CandidateCoverageRecord, CandidateRef, CoverageAuthority, CoverageRef,
    DecisionOutcome, DecisionRef, ExecutionAuthorization, PreparationDecision, ProposalRef,
    RetainedCandidate, RoomSource,
};
use wire::{CommandKind, Name, SequenceCode, TaskStamp, Witness};
use yew::prelude::*;

use super::{Controls, conversation::lane_label, statement_witness};
use crate::AppRuntime;
use crate::browser::sender::{SenderHandle, admit_action};
use crate::client_action::{ClientAction, PreparationRequest};
use crate::runtime::{PreparationProjection, RoomPositioned};

/// Where one candidate stands under its controller's decisions.
enum Standing<'a> {
    /// No decision names it.
    Undecided,
    /// Its latest decision returned it for further preparation.
    Returned,
    /// Its latest decision requests collapse through the Design authority.
    Collapse,
    /// A selected decision whose authorization is not in this projection.
    Selected(&'a DecisionRef),
    /// An exact selected decision authorized this subset.
    Authorized(&'a ExecutionAuthorization),
}

impl Standing<'_> {
    fn label(&self) -> String {
        match self {
            Self::Undecided => "Undecided".to_owned(),
            Self::Returned => "Returned".to_owned(),
            Self::Collapse => "Collapse requested".to_owned(),
            Self::Selected(decision) => format!("Selected by decision {}", decision.decision),
            Self::Authorized(authorization) => format!(
                "Selected {} output(s) by decision {}",
                authorization.selected_outputs.len(),
                authorization.decision.decision
            ),
        }
    }
}

/// What one Design's approval renders against.
struct Context<'a> {
    runtime: &'a AppRuntime,
    controls: &'a Controls,
    sender: &'a SenderHandle,
    preparation: &'a PreparationProjection,
    /// The Design while it is an open board task; review evidence is offered
    /// only then.
    controller: Option<&'a BoardTask>,
}

impl<'a> Context<'a> {
    fn task(&self, stamp: &TaskStamp) -> Option<&'a BoardTask> {
        board_task(self.runtime, stamp)
    }
}

fn board_task<'a>(runtime: &'a AppRuntime, stamp: &TaskStamp) -> Option<&'a BoardTask> {
    runtime
        .board()?
        .tasks
        .iter()
        .find(|task| task.statement.body.born == *stamp)
}

/// Everything one candidate's review action submits against, drawn from the
/// board and preparation authority this render read.
#[derive(Clone)]
struct Approval {
    report: Callback<String>,
    sender: SenderHandle,
    /// The candidate's retaining fact, which the review cites.
    basis: SequenceCode,
    /// The Design's statement head, which the review witnesses.
    statement: Witness,
    /// The identity the operator's next coverage of this candidate takes.
    next_coverage: CoverageRef,
}

/// The approval control for every candidate retained under one Design.
pub(super) fn render_approval(
    runtime: &AppRuntime,
    controls: &Controls,
    sender: &SenderHandle,
    design: &TaskStamp,
) -> Html {
    let preparation = runtime.room_projection().preparation();
    let candidates = preparation
        .candidates
        .values()
        .filter(|positioned| positioned.value.candidate.controller == *design)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Html::default();
    }
    let context = Context {
        runtime,
        controls,
        sender,
        preparation,
        controller: board_task(runtime, design)
            .filter(|task| task.workflow.body.completion.is_none()),
    };
    html! {
        <section class="room-approval" aria-label="Approval" data-room-approval="">
            <header>
                <strong>{"Approval"}</strong>
                <span>{candidates.len()}</span>
            </header>
            <p class="room-notice">
                {"Retained candidates and review evidence stay readable here. Exact selection is an operator preparation decision and leaves the Design open."}
            </p>
            {for candidates.into_iter().map(|positioned| render_candidate(&context, positioned))}
        </section>
    }
}

fn render_candidate(context: &Context<'_>, positioned: &RoomPositioned<RetainedCandidate>) -> Html {
    let candidate = &positioned.value;
    let standing = standing(context.preparation, &candidate.candidate);
    let coverage = context
        .preparation
        .coverage
        .values()
        .filter(|coverage| coverage.value.coverage.candidate == candidate.candidate)
        .map(|coverage| &coverage.value)
        .collect::<Vec<_>>();
    let next_coverage = CoverageRef {
        candidate: candidate.candidate.clone(),
        coverage: SequenceCode::new(
            context
                .preparation
                .coverage
                .keys()
                .filter(|coverage| coverage.candidate == candidate.candidate)
                .map(|coverage| coverage.coverage.get())
                .max()
                .unwrap_or(0)
                .saturating_add(1),
        ),
    };
    let approval = context.controller.map(|controller| Approval {
        report: context
            .controls
            .approval_reporter(candidate.candidate.clone()),
        sender: context.sender.clone(),
        basis: SequenceCode::new(positioned.position.get()),
        statement: statement_witness(controller),
        next_coverage,
    });
    let notice = context
        .controls
        .approval_notice
        .as_ref()
        .filter(|(noticed, _)| *noticed == candidate.candidate)
        .map(|(_, notice)| notice.clone());
    let code = candidate.candidate.candidate.to_string();
    html! {
        <article class="room-card room-approval-card" key={code.clone()} data-candidate={code.clone()}>
            <header>
                <strong>{format!("Candidate {code}")}</strong>
                <span data-approval-standing="">{standing.label()}</span>
            </header>
            {render_coverage(context.runtime, &coverage)}
            if !candidate.governed_inputs.is_empty() {
                <p class="room-approval-inputs">
                    {"Governed inputs: "}
                    {candidate.governed_inputs.iter().map(|input| handle(context, input)).collect::<Vec<_>>().join(", ")}
                </p>
            }
            <ul class="room-approval-outputs">
                {for candidate.outputs.iter().map(|output| render_output(context, output))}
            </ul>
            if let Some(approval) = &approval {
                {render_review_action(context.runtime, approval)}
            }
            if let Some(notice) = notice {
                <p class="room-notice" role="status" data-approval-notice="">{notice}</p>
            }
        </article>
    }
}

fn render_coverage(runtime: &AppRuntime, coverage: &[&CandidateCoverage]) -> Html {
    if coverage.is_empty() {
        return html! { <p class="room-approval-coverage">{"No review coverage recorded."}</p> };
    }
    html! {
        <ul class="room-approval-coverage" aria-label="Review coverage">
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
                    <li>{format!("Coverage {} · {by} · {}", coverage.coverage.coverage, scopes.join(", "))}</li>
                }
            })}
        </ul>
    }
}

fn render_output(context: &Context<'_>, output: &TaskStamp) -> Html {
    let Some(task) = context.task(output) else {
        return html! {
            <li data-approval-output={output.to_string()}>
                <code>{output.to_string()}</code>{" · not on the board"}
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
                <strong>{task.presentation_title.clone()}</strong>
                <code>{statement.handle.to_string()}</code>
                <span>{standing}</span>
            </label>
            if !statement.acceptance.is_empty() {
                <ul class="room-approval-acceptance" aria-label="Acceptance">
                    {for statement.acceptance.iter().map(|criterion| html! { <li>{criterion.clone()}</li> })}
                </ul>
            }
        </li>
    }
}

fn render_review_action(runtime: &AppRuntime, approval: &Approval) -> Html {
    html! {
        <div class="room-inline-actions">
            <button
                type="button"
                disabled={!runtime.offers(CommandKind::CandidateCoverageRecord)}
                onclick={approval.review()}
                data-approval-review=""
            >
                {"Record my review"}
            </button>
        </div>
    }
}

/// The candidate's current standing: an authorization of it, else its latest
/// selected or returned decision.
fn standing<'a>(preparation: &'a PreparationProjection, candidate: &CandidateRef) -> Standing<'a> {
    if let Some(authorized) = preparation
        .authorizations
        .values()
        .find(|authorized| authorized.value.candidate == *candidate)
    {
        return Standing::Authorized(&authorized.value);
    }
    let latest = preparation
        .decisions
        .values()
        .filter(|decision| match &decision.value.outcome {
            DecisionOutcome::Selected {
                candidate: decided, ..
            }
            | DecisionOutcome::Returned {
                candidate: decided, ..
            } => decided == candidate,
            DecisionOutcome::Collapse {
                candidate: decided, ..
            } => decided == candidate,
        })
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
fn handle(context: &Context<'_>, task: &TaskStamp) -> String {
    context.task(task).map_or_else(
        || task.to_string(),
        |task| task.statement.body.handle.to_string(),
    )
}

impl Approval {
    /// Record the operator's own review of this exact candidate, citing the
    /// candidate's retaining fact and covering its outputs and acceptance.
    fn review(&self) -> Callback<MouseEvent> {
        let approval = self.clone();
        Callback::from(move |_: MouseEvent| {
            let coverage = approval.next_coverage.clone();
            let expected = vec![
                approval.statement.clone(),
                Witness::Proposal {
                    proposal: ProposalRef::Coverage {
                        coverage: coverage.clone(),
                    },
                    head: None,
                },
            ];
            let scopes = ["outputs", "acceptance"]
                .into_iter()
                .map(|scope| {
                    scope
                        .parse::<Name>()
                        .expect("fixed coverage scope is a name")
                })
                .collect();
            approval.send(PreparationRequest::Cover {
                request: CandidateCoverageRecord {
                    coverage: CandidateCoverage {
                        coverage,
                        authority: CoverageAuthority::Operator {},
                        scopes,
                        source: RoomSource::Journal {
                            fact: approval.basis,
                        },
                    },
                },
                expected,
            });
        })
    }

    fn send(&self, request: PreparationRequest) {
        let report = self.report.clone();
        let respond = Rc::new(move |answer: Result<TextAnswer, String>| {
            report.emit(answer.map_or_else(|error| error, |answer| answer.text));
        });
        let _ = admit_action(&self.sender, ClientAction::Preparation { request, respond });
    }
}
