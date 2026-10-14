//! A Design's approval control. For each retained candidate the operator
//! records an accepted or returned decision, records their own review
//! coverage of the candidate when no independent or operator review exists,
//! then authorizes an explicit selection of an accepted candidate's outputs
//! under the candidate's governed inputs and every coverage recorded over it.
//! No output is selected by default, a returned decision authorizes nothing,
//! an action the panel already knows the authority would refuse is disabled
//! with its reason, and the resident alone judges admissibility: its answer
//! or refusal renders under the candidate.

use std::rc::Rc;

use web_sys::HtmlInputElement;
use wire::intent::{BoardTask, TextAnswer};
use wire::rooms::{
    CandidateCoverage, CandidateCoverageRecord, CandidateRef, CoverageAuthority, CoverageRef,
    DecisionOutcome, DecisionRef, ExecutionAuthorization, ExecutionAuthorize, PreparationDecide,
    PreparationDecision, ProposalRef, RetainedCandidate, RoomSource,
};
use wire::{CommandKind, Name, SequenceCode, TaskStamp, Witness};
use yew::prelude::*;

use super::{Controls, conversation::lane_label, statement_witness, transition_witnesses};
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
    /// Its latest decision accepted it and no selection is authorized yet.
    Accepted(&'a DecisionRef),
    /// An accepted decision authorized this exact selection.
    Authorized(&'a ExecutionAuthorization),
}

impl Standing<'_> {
    fn label(&self) -> String {
        match self {
            Self::Undecided => "Undecided".to_owned(),
            Self::Returned => "Returned".to_owned(),
            Self::Accepted(decision) => format!("Accepted by decision {}", decision.decision),
            Self::Authorized(authorization) => format!(
                "Authorized {} output(s) by decision {}",
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
    /// The Design while it is an open board task; decisions and
    /// authorizations are offered only then.
    controller: Option<&'a BoardTask>,
    /// The decision identity the next decision under the Design takes.
    next: SequenceCode,
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

/// One output the operator may select, with the heads authorizing it witnesses.
#[derive(Clone)]
struct Selectable {
    task: TaskStamp,
    node: NodeRef,
    heads: Vec<Witness>,
}

/// Everything one candidate's buttons submit against, drawn from the board
/// and preparation authority this render read.
#[derive(Clone)]
struct Approval {
    report: Callback<String>,
    sender: SenderHandle,
    candidate: RetainedCandidate,
    /// The candidate's retaining fact, the basis a decision cites.
    basis: SequenceCode,
    next: SequenceCode,
    /// The Design's statement head, which a decision witnesses.
    statement: Witness,
    /// The Design's transition heads, which an authorization witnesses.
    controller: Vec<Witness>,
    /// Governed input statement heads, or why one cannot be witnessed.
    inputs: Result<Vec<Witness>, String>,
    coverage: Vec<CoverageRef>,
    /// The identity the operator's next coverage of this candidate takes.
    next_coverage: CoverageRef,
    /// Why an authorization would be refused, as far as this render can
    /// see: no certifying coverage, or a pending dependency of the Design.
    blocked: Option<String>,
    outputs: Vec<Selectable>,
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
        next: next_decision(preparation, design),
    };
    html! {
        <section class="room-approval" aria-label="Approval" data-room-approval="">
            <header>
                <strong>{"Approval"}</strong>
                <span>{candidates.len()}</span>
            </header>
            <p class="room-notice">
                {"Accept or return a retained candidate, record your review of it unless an operator or independent review already covers it, then authorize exactly the outputs you select. Authorizing concludes the Design; outputs you leave unselected stay pending and ineligible."}
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
    let selecting = matches!(standing, Standing::Accepted(_))
        && context.controller.is_some()
        && context.runtime.offers(CommandKind::ExecutionAuthorize);
    let outputs = candidate
        .outputs
        .iter()
        .filter_map(|output| {
            let task = context.task(output).filter(|_| selecting)?;
            let mut heads = transition_witnesses(task);
            heads.push(Witness::Authorization {
                output: output.clone(),
                head: None,
            });
            Some(Selectable {
                task: output.clone(),
                node: NodeRef::default(),
                heads,
            })
        })
        .collect::<Vec<_>>();
    let certified = coverage
        .iter()
        .any(|coverage| !matches!(coverage.authority, CoverageAuthority::SelfReview { .. }));
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
        candidate: candidate.clone(),
        basis: SequenceCode::new(positioned.position.get()),
        next: context.next,
        statement: statement_witness(controller),
        controller: transition_witnesses(controller),
        inputs: candidate
            .governed_inputs
            .iter()
            .map(|input| {
                context.task(input).map(statement_witness).ok_or_else(|| {
                    format!(
                        "preparation::task-unavailable: governed input {input} is not on the board; refresh and retry"
                    )
                })
            })
            .collect(),
        coverage: coverage.iter().map(|coverage| coverage.coverage.clone()).collect(),
        next_coverage,
        blocked: authorization_blocker(context, controller, certified),
        outputs,
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
                {for candidate.outputs.iter().map(|output| render_output(context, approval.as_ref(), output))}
            </ul>
            if let Some(approval) = &approval {
                {render_actions(context.runtime, &standing, approval)}
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
                    CoverageAuthority::SelfReview { lane, .. } => format!(
                        "self-review by {}, which certifies nothing alone",
                        lane_label(runtime, *lane)
                    ),
                };
                let scopes = coverage.scopes.iter().map(ToString::to_string).collect::<Vec<_>>();
                html! {
                    <li>{format!("Coverage {} · {by} · {}", coverage.coverage.coverage, scopes.join(", "))}</li>
                }
            })}
        </ul>
    }
}

fn render_output(context: &Context<'_>, approval: Option<&Approval>, output: &TaskStamp) -> Html {
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
    let selectable = approval.and_then(|approval| {
        approval
            .outputs
            .iter()
            .find(|selectable| selectable.task == *output)
    });
    html! {
        <li data-approval-output={output.to_string()}>
            <label>
                if let Some(selectable) = selectable {
                    <input type="checkbox" ref={selectable.node.clone()} />
                }
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

fn render_actions(runtime: &AppRuntime, standing: &Standing<'_>, approval: &Approval) -> Html {
    let decides = runtime.offers(CommandKind::PreparationDecide);
    let covers = runtime.offers(CommandKind::CandidateCoverageRecord);
    let authorizes = runtime.offers(CommandKind::ExecutionAuthorize);
    let accept = html! {
        <button type="button" disabled={!decides} onclick={approval.decide(true)}>{"Accept"}</button>
    };
    let ret = html! {
        <button type="button" disabled={!decides} onclick={approval.decide(false)}>{"Return"}</button>
    };
    // Recording a review is always the act that satisfies a coverage refusal,
    // missing or stale alike, so it stays offered until authorization.
    let review = html! {
        <button
            type="button"
            disabled={!covers}
            onclick={approval.review()}
            data-approval-review=""
        >
            {"Record my review"}
        </button>
    };
    let actions = match standing {
        Standing::Authorized(_) => return Html::default(),
        Standing::Undecided => html! { <>{accept}{ret}{review}</> },
        Standing::Returned => html! { <>{accept}{review}</> },
        Standing::Accepted(decision) => html! {
            <>
                <button
                    type="button"
                    disabled={!authorizes || approval.blocked.is_some()}
                    title={approval.blocked.clone()}
                    onclick={approval.authorize((*decision).clone())}
                    data-approval-authorize=""
                >
                    {"Authorize selected"}
                </button>
                {review}
                {ret}
            </>
        },
    };
    html! {
        <>
            <div class="room-inline-actions">{actions}</div>
            if let Some(blocked) = &approval.blocked {
                <p class="room-notice" data-approval-blocked="">{blocked.clone()}</p>
            }
        </>
    }
}

/// Why the authority would refuse an authorization of this candidate, as far
/// as the board and preparation authority this render read can tell.
fn authorization_blocker(
    context: &Context<'_>,
    controller: &BoardTask,
    certified: bool,
) -> Option<String> {
    if !certified {
        return Some(
            "Authorizing needs a review of this candidate by you or an independent lane; its author's self-review certifies nothing. Record my review cites this candidate's retained record and covers its outputs and their acceptance."
                .to_owned(),
        );
    }
    let pending = controller
        .statement
        .body
        .dependencies
        .iter()
        .filter(|dependency| {
            context
                .task(dependency)
                .is_some_and(|task| task.workflow.body.completion.is_none())
        })
        .map(|dependency| handle(context, dependency))
        .collect::<Vec<_>>();
    (!pending.is_empty()).then(|| {
        format!(
            "Authorizing waits until this Design's dependencies complete: {}.",
            pending.join(", ")
        )
    })
}

/// The candidate's current standing: an authorization of it, else its latest
/// accepting or returning decision.
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
            DecisionOutcome::Accepted { candidate: decided }
            | DecisionOutcome::Returned { candidate: decided } => decided == candidate,
            DecisionOutcome::Negative { .. } => false,
        })
        .max_by_key(|decision| decision.position);
    match latest.map(|decision| &decision.value) {
        None => Standing::Undecided,
        Some(PreparationDecision {
            decision,
            outcome: DecisionOutcome::Accepted { .. },
            ..
        }) => Standing::Accepted(decision),
        Some(_) => Standing::Returned,
    }
}

/// One past the greatest decision recorded under the Design.
fn next_decision(preparation: &PreparationProjection, design: &TaskStamp) -> SequenceCode {
    let greatest = preparation
        .decisions
        .keys()
        .filter(|decision| decision.controller == *design)
        .map(|decision| decision.decision.get())
        .max()
        .unwrap_or(0);
    SequenceCode::new(greatest.saturating_add(1))
}

fn handle(context: &Context<'_>, task: &TaskStamp) -> String {
    context.task(task).map_or_else(
        || task.to_string(),
        |task| task.statement.body.handle.to_string(),
    )
}

impl Approval {
    /// Accept or return the candidate as the next decision under its Design.
    fn decide(&self, accepted: bool) -> Callback<MouseEvent> {
        let approval = self.clone();
        Callback::from(move |_: MouseEvent| {
            let candidate = approval.candidate.candidate.clone();
            let decision = DecisionRef {
                controller: candidate.controller.clone(),
                decision: approval.next,
            };
            let expected = vec![
                approval.statement.clone(),
                Witness::Proposal {
                    proposal: ProposalRef::Decision {
                        decision: decision.clone(),
                    },
                    head: None,
                },
            ];
            let outcome = if accepted {
                DecisionOutcome::Accepted { candidate }
            } else {
                DecisionOutcome::Returned { candidate }
            };
            approval.send(PreparationRequest::Decide {
                request: PreparationDecide {
                    decision: PreparationDecision {
                        decision,
                        source: RoomSource::Journal {
                            fact: approval.basis,
                        },
                        outcome,
                    },
                },
                expected,
            });
        })
    }

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

    /// Authorize exactly the checked outputs under `decision`.
    fn authorize(&self, decision: DecisionRef) -> Callback<MouseEvent> {
        let approval = self.clone();
        Callback::from(
            move |_: MouseEvent| match approval.authorization(&decision) {
                Ok(request) => approval.send(request),
                Err(refusal) => approval.report.emit(refusal),
            },
        )
    }

    /// The authorization the checked outputs describe, or why nothing is
    /// submitted: a refusal the panel can already see, no output checked, or
    /// a governed input the board does not carry and so cannot be witnessed.
    fn authorization(&self, decision: &DecisionRef) -> Result<PreparationRequest, String> {
        if let Some(blocked) = &self.blocked {
            return Err(blocked.clone());
        }
        let selected = self
            .outputs
            .iter()
            .filter(|output| {
                output
                    .node
                    .cast::<HtmlInputElement>()
                    .is_some_and(|input| input.checked())
            })
            .collect::<Vec<_>>();
        if selected.is_empty() {
            return Err("Select the outputs to authorize; none is selected by default.".to_owned());
        }
        let mut expected = self.controller.clone();
        expected.extend(self.inputs.clone()?);
        for output in &selected {
            expected.extend(output.heads.iter().cloned());
        }
        Ok(PreparationRequest::Authorize {
            request: ExecutionAuthorize {
                authorization: ExecutionAuthorization {
                    decision: decision.clone(),
                    candidate: self.candidate.candidate.clone(),
                    governed_inputs: self.candidate.governed_inputs.clone(),
                    selected_outputs: selected.iter().map(|output| output.task.clone()).collect(),
                    coverage: self.coverage.clone(),
                },
            },
            expected,
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
