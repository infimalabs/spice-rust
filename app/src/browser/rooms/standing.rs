//! A room's standing inside its design menu: why each lane is involved and
//! whether it is available, and the resident's shadow relevance observations.
//! Every row names the admitted source it traces to. Nothing here is a roster
//! of authority, and rendering it mounts no view and wakes no lane.

use wasm_bindgen::JsCast as _;
use wire::intent::BoardTask;
use wire::rooms::{
    DesignCyclePresentation, DesignNoticeEvidence, DesignPassEnd, RoomProgressStanding,
    RoomRelevanceClass, RoomRelevanceVerdict, RoomSourceView,
};
use wire::{SequenceCode, TaskStamp};
use yew::prelude::*;

use crate::AppRuntime;
use crate::runtime::{
    ParticipantReason, Relevance, RoomAnchorAuthority, RoomParticipant, design_next_action,
};
use crate::watchband;

/// The standing section of one design's menu and of its read view; `row` is
/// the Design's current task row, when one is known.
pub(super) fn render_standing(
    runtime: &AppRuntime,
    room: &TaskStamp,
    row: Option<&BoardTask>,
) -> Html {
    let projection = runtime.room_projection();
    let participants = projection.participants(room, runtime.board());
    let relevance = projection.relevance(room).collect::<Vec<_>>();
    let lifecycle = row.and_then(BoardTask::design_standing);
    let cycle = row.and_then(|row| {
        let cycle = *runtime.board()?.design_cycles.get(room)?;
        Some(DesignCyclePresentation::new(
            cycle,
            row.design_cycle,
            &row.statement.body.handle,
            row.design_availability(),
            row.design_coverage.as_ref()?,
            &row.statement.body.acceptance,
        ))
    });
    let settled = lifecycle.as_ref().map_or_else(
        || {
            matches!(
                projection.anchor_authority(room, runtime.board()),
                RoomAnchorAuthority::Settled { .. }
            )
        },
        |standing| !standing.is_open(),
    );
    html! {
        <section class="room-standing" data-room-standing="">
            <header><h4>{"Standing"}</h4></header>
            if let Some(standing) = &lifecycle {
                <p class="room-standing-lifecycle" data-design-lifecycle={standing.key()}>
                    {standing.to_string()}
                </p>
            }
            {cycle.as_ref().map_or_else(Html::default, |cycle| render_cycle(room, cycle))}
            if let Some(research) = cycle.as_ref().and_then(|cycle| cycle.research.as_deref()) {
                <p class="room-standing-research" data-design-research="">{research}</p>
            }
            <p class="room-standing-next" data-room-next="">
                {design_next_action(
                    settled,
                    cycle.as_ref(),
                )}
            </p>
            {render_lanes(runtime, &participants)}
            if !relevance.is_empty() {
                <ul class="room-standing-relevance" aria-label="Shadow relevance observations">
                    {for relevance.iter().rev().map(|relevance| render_observation(&relevance.value))}
                </ul>
            }
        </section>
    }
}

/// The resident's pass cycle for one open Design: what it waits on, who
/// acts, how the last pass ended, and the exact proposal or decision it
/// cites.
fn render_cycle(room: &TaskStamp, cycle: &DesignCyclePresentation) -> Html {
    let last_pass = cycle.last_pass.map(|end| match end {
        DesignPassEnd::Proposed => "The last pass ended with a proposal, which released its claim.",
        DesignPassEnd::Abandoned => {
            "The last pass was abandoned: its claim ended without a proposal."
        }
    });
    html! {
        <div
            class="room-standing-cycle"
            data-design-cycle={cycle.cycle.key()}
            data-cycle-actor={cycle.actor.label()}
        >
            <p>
                {"Pass cycle: "}<strong>{cycle.cycle.label()}</strong>
                {" · next: "}<strong>{cycle.actor.label()}</strong>
            </p>
            if let (Some(end), Some(text)) = (cycle.last_pass, last_pass) {
                <p data-last-pass={end.label()}>{text}</p>
            }
            if let Some(proposal) = cycle.proposal {
                <p>{"Proposal awaiting decision: "}{fact_link(room, proposal)}</p>
            }
            if let Some(decision) = cycle.decision {
                <p>{"Latest decision: "}{fact_link(room, decision)}</p>
            }
        </div>
    }
}

/// A link to the exact proposal or decision fact. On the Design's read view
/// it focuses that record in place; elsewhere it opens the read view.
fn fact_link(room: &TaskStamp, fact: SequenceCode) -> Html {
    let code = fact.to_string();
    let onclick = focus_in_place(format!("[data-fact=\"{code}\"]"));
    html! {
        <a href={crate::view::design_href(room)} data-cycle-fact={code.clone()} {onclick}>
            <code>{code}</code>
        </a>
    }
}

/// Focus the element `selector` names when this page shows it; otherwise
/// let the link open the Design's read view, which does.
pub(super) fn focus_in_place(selector: String) -> Callback<MouseEvent> {
    Callback::from(move |event: MouseEvent| {
        let Some(element) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.query_selector(&selector).ok().flatten())
        else {
            return;
        };
        event.prevent_default();
        element.scroll_into_view();
        if let Ok(element) = element.dyn_into::<web_sys::HtmlElement>() {
            let _ = element.focus();
        }
    })
}

fn render_lanes(runtime: &AppRuntime, participants: &[RoomParticipant]) -> Html {
    if participants.is_empty() {
        return html! { <p class="room-standing-empty">{"No lane is involved."}</p> };
    }
    html! {
        <ul class="room-standing-lanes" aria-label="Why each lane is involved">
            {for participants.iter().map(|participant| render_lane(runtime, participant))}
        </ul>
    }
}

fn render_lane(runtime: &AppRuntime, participant: &RoomParticipant) -> Html {
    let member = runtime.lanes().and_then(|lanes| {
        lanes
            .lanes
            .iter()
            .flat_map(|team| &team.members)
            .find(|member| member.lane == participant.lane)
    });
    let label = member.map_or_else(
        || participant.lane.to_string(),
        |member| {
            watchband::agent_message_label(&member.worktree, member.name.as_deref()).to_owned()
        },
    );
    let availability = match member {
        Some(member) if member.occupant.is_some() => "seated",
        Some(_) => "vacant",
        None => "unknown",
    };
    let href = super::super::watchband::lane_composer_href(runtime.lanes(), participant.lane);
    let publications = participant
        .reasons
        .iter()
        .filter_map(|reason| match reason {
            ParticipantReason::Publication(publication) => Some(publication),
            _ => None,
        })
        .collect::<Vec<_>>();
    html! {
        <li key={participant.lane.to_string()} data-lane={participant.lane.to_string()}>
            if let Some(href) = href {
                <a
                    class="room-standing-lane"
                    {href}
                    aria-label={format!("Open {label} in Chat")}
                >{label.clone()}</a>
            } else {
                <span class="room-standing-lane">{label}</span>
            }
            <span class="room-standing-availability" data-availability={availability}>{availability}</span>
            if let Some(latest) = publications.last() {
                {reason_chip(
                    "history",
                    &format!("Published {}", publications.len()),
                    &format!("Authored {} room publications; latest {latest}", publications.len()),
                )}
            }
            {for participant.reasons.iter().filter_map(|reason| render_reason(runtime, reason))}
        </li>
    }
}

/// One non-history reason as a chip naming its source; publication history
/// renders once per lane instead.
fn render_reason(runtime: &AppRuntime, reason: &ParticipantReason) -> Option<Html> {
    let leave;
    let invitation;
    let (kind, label, source) = match reason {
        ParticipantReason::Publication(_) => return None,
        ParticipantReason::Asked {
            evidence,
            questions,
            reach,
            matching,
        } => {
            let (label, scoring) = invitation_match(*reach, matching.as_ref());
            invitation = label;
            (
                "invitation",
                invitation.as_str(),
                format!(
                    "{}; {scoring}{}",
                    match evidence {
                        DesignNoticeEvidence::Task { task, .. }
                        | DesignNoticeEvidence::TaskClaim { task, .. } => format!(
                            "A Design request asked this lane from retained evidence about {}",
                            task_label(runtime, task)
                        ),
                        DesignNoticeEvidence::Conversation { source } => format!(
                            "A Design request asked this lane from retained conversation evidence at member {}, byte {}",
                            source.locus.member, source.locus.offset
                        ),
                    },
                    asked_questions(questions)
                ),
            )
        }
        ParticipantReason::Work(task) => (
            "work",
            "Working",
            format!("Holds {}", task_label(runtime, task)),
        ),
        ParticipantReason::Stop(key) => (
            "responsibility",
            "Asked to stop",
            format!("Stop request {key} on its current claim"),
        ),
        ParticipantReason::Left {
            publication,
            clause,
        } => {
            leave = format!("Left: {clause}");
            (
                "left",
                leave.as_str(),
                format!(
                    "Left at {publication}; only native work and duties keep this lane listening"
                ),
            )
        }
    };
    Some(reason_chip(kind, label, &source))
}

/// Compact visible score beside its reach, plus complete audit provenance for
/// the chip title. Retained notices predating match provenance never read as
/// zero.
fn invitation_match(
    reach: Option<wire::rooms::DesignReach>,
    matching: Option<&wire::rooms::DesignNoticeMatch>,
) -> (String, String) {
    let reach = reach.map_or("reach unavailable", |reach| match reach {
        wire::rooms::DesignReach::Wide => "wide",
        wire::rooms::DesignReach::Narrow => "narrow",
    });
    match matching {
        Some(matching) => (
            format!(
                "Asked · {reach} {score:.3}/{floor:.3}",
                score = matching.score(),
                floor = matching.floor(),
            ),
            format!(
                "{reach} match score {score}, floor {floor}, fit {generation}",
                score = matching.score(),
                floor = matching.floor(),
                generation = matching.generation().as_str(),
            ),
        ),
        None => (
            format!("Asked · {reach} score unavailable"),
            format!("{reach} match score unavailable"),
        ),
    }
}

/// The questions a Design request asked, in asked order; nothing for a
/// whole-Design ask.
fn asked_questions(questions: &[wire::rooms::RoomQuestion]) -> String {
    if questions.is_empty() {
        return String::new();
    }
    let listed = questions
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        " on question{} {listed}",
        if questions.len() == 1 { "" } else { "s" }
    )
}

fn reason_chip(kind: &'static str, label: &str, source: &str) -> Html {
    html! {
        <span
            class="room-reason"
            data-reason={kind}
            title={source.to_owned()}
            aria-label={format!("{label}: {source}")}
        >{label.to_owned()}</span>
    }
}

fn task_label(runtime: &AppRuntime, task: &TaskStamp) -> String {
    runtime
        .board()
        .and_then(|board| {
            board
                .tasks
                .iter()
                .find(|entry| entry.statement.body.born == *task)
        })
        .map_or_else(
            || task.to_string(),
            |entry| entry.statement.body.handle.to_string(),
        )
}

fn render_observation(relevance: &Relevance) -> Html {
    let observation = &relevance.observation;
    let unavailable = observation
        .sources
        .iter()
        .filter(|source| matches!(source, RoomSourceView::Unavailable { .. }))
        .count();
    html! {
        <li
            key={observation.observation.to_string()}
            class="room-standing-observation"
            data-relevance={observation.classification.name()}
            data-progress={observation.progress.name()}
        >
            <strong>{class_label(observation.classification)}</strong>
            {format!(
                " · {} · {}",
                progress_label(observation.progress),
                verdict_label(relevance.verdict)
            )}
            <p>{observation.explanation.clone()}</p>
            if !observation.exceptions.is_empty() {
                <p>{format!("Exceptions: {}", observation.exceptions)}</p>
            }
            if !observation.uncertainty.is_empty() {
                <p>{format!("Uncertainty: {}", observation.uncertainty)}</p>
            }
            <small>
                {format!(
                    "{} sources, {unavailable} unavailable, through {}",
                    observation.sources.len(),
                    observation.through
                )}
            </small>
        </li>
    }
}

const fn class_label(class: RoomRelevanceClass) -> &'static str {
    match class {
        RoomRelevanceClass::RelevantContribution => "Relevant contribution",
        RoomRelevanceClass::Declined => "Declined",
        RoomRelevanceClass::Duplicate => "Duplicate",
        RoomRelevanceClass::StaleActivity => "Stale activity",
        RoomRelevanceClass::MissedOpportunity => "Missed opportunity",
        RoomRelevanceClass::Agreement => "Agreement, not evidence",
        RoomRelevanceClass::MessageCount => "Message count, not evidence",
        RoomRelevanceClass::Silence => "Silence, not evidence",
        RoomRelevanceClass::ElapsedTime => "Elapsed time, not evidence",
        RoomRelevanceClass::Unknown => "Unclassified",
    }
}

const fn progress_label(progress: RoomProgressStanding) -> &'static str {
    match progress {
        RoomProgressStanding::Advanced => "advanced a criterion",
        RoomProgressStanding::NoProgress => "no progress",
        RoomProgressStanding::Unknown => "progress unknown",
    }
}

const fn verdict_label(verdict: Option<RoomRelevanceVerdict>) -> &'static str {
    match verdict {
        None => "unassessed",
        Some(RoomRelevanceVerdict::Confirmed) => "operator confirmed",
        Some(RoomRelevanceVerdict::FalsePositive) => "operator: false positive",
        Some(RoomRelevanceVerdict::Unknown) => "operator unsure",
    }
}
