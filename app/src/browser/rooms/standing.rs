//! A room's standing inside its design menu: why each lane is involved and
//! whether it is available, what the latest coordination asks and how its
//! request stands, and the resident's shadow relevance observations. Every
//! row names the admitted source it traces to. Nothing here is a roster of
//! authority, and rendering it mounts no view and wakes no lane.

use wasm_bindgen::JsCast as _;
use wire::intent::{BoardTask, DesignStanding};
use wire::rooms::{
    DesignAvailability, DesignCycle, DesignCyclePresentation, DesignNoticeEvidence, DesignPassEnd,
    RoomProgressStanding, RoomRelevanceClass, RoomRelevanceVerdict, RoomSourceView,
};
use wire::{Disposition, SequenceCode, TaskStamp};
use yew::prelude::*;

use crate::AppRuntime;
use crate::runtime::{
    CoordinationStanding, ParticipantReason, Relevance, RoomAnchorAuthority, RoomParticipant,
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
    let coordination = projection.coordination_standing(room);
    let relevance = projection.relevance(room).collect::<Vec<_>>();
    let lifecycle = row.and_then(BoardTask::design_standing);
    let cycle = row.and_then(|row| {
        let cycle = *runtime.board()?.design_cycles.get(room)?;
        Some(DesignCyclePresentation::new(
            cycle,
            row.design_cycle,
            &row.statement.body.handle,
            row.design_availability(),
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
            <header><strong>{"Standing"}</strong></header>
            if let Some(standing) = &lifecycle {
                <p class="room-standing-lifecycle" data-design-lifecycle={standing.key()}>
                    {standing.to_string()}
                </p>
            }
            {cycle.as_ref().map_or_else(Html::default, |cycle| render_cycle(room, cycle))}
            <p class="room-standing-next" data-room-next="">
                {next_action(
                    settled,
                    lifecycle.as_ref(),
                    coordination.as_ref(),
                    cycle.as_ref().zip(row).map(|(cycle, row)| (cycle.cycle, row.design_availability())),
                )}
            </p>
            {coordination.as_ref().map_or_else(Html::default, render_coordination)}
            {render_lanes(runtime, &participants)}
            if !relevance.is_empty() {
                <ul class="room-standing-relevance" aria-label="Shadow relevance observations">
                    {for relevance.iter().rev().map(|relevance| render_observation(&relevance.value))}
                </ul>
            }
        </section>
    }
}

/// The one next useful action the current authority supports, as one
/// sentence naming who acts and on what. An open Design's cycle names it
/// once no coordination obligation is outstanding.
fn next_action(
    settled: bool,
    lifecycle: Option<&DesignStanding>,
    coordination: Option<&CoordinationStanding<'_>>,
    cycle: Option<(DesignCycle, DesignAvailability)>,
) -> String {
    if settled {
        return "The anchor is settled; nothing remains to coordinate.".to_owned();
    }
    match (coordination, cycle) {
        (Some(standing), _) if standing.request == Some(Disposition::Live) => format!(
            "Wait for {} to acknowledge request {}.",
            standing.record.value.target, standing.record.value.key
        ),
        (_, Some((cycle, availability))) => cycle_next(cycle, availability).to_owned(),
        _ if lifecycle == Some(&DesignStanding::Passive) => "Set the Design's working group to \
            Explore or Converge before coordinating a lane onto it; if ungrouped, group it first."
            .to_owned(),
        (Some(standing), None) if !standing.put_aside => format!(
            "Read {}'s contribution, then Resume or put the room aside.",
            standing.record.value.target
        ),
        _ => "Start coordination to ask one lane for the next contribution.".to_owned(),
    }
}

/// Who moves an open Design's pass cycle forward next, and how.
const fn cycle_next(cycle: DesignCycle, availability: DesignAvailability) -> &'static str {
    match (cycle, availability) {
        (DesignCycle::GatheringInput, _) => {
            "Lanes or you add input next: a vote, a question, or a post in this room."
        }
        (DesignCycle::ReadyForProposal, DesignAvailability::Passive) => {
            "You activate this Design by moving its working group to Explore or Converge, so a \
             lane can claim it."
        }
        (DesignCycle::ReadyForProposal, DesignAvailability::Unclaimed) => {
            "A lane claims this Design next and ends its pass with one proposal."
        }
        (DesignCycle::ReadyForProposal, DesignAvailability::Claimed) => {
            "The holding lane ends this pass with one proposal."
        }
        (DesignCycle::AwaitingDecision, _) => "You decide the proposal awaiting decision next.",
        (DesignCycle::WaitingOnSelectedWork, _) => {
            "The work this Design selected finishes next, unless lanes or you add new input."
        }
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

fn render_coordination(standing: &CoordinationStanding<'_>) -> Html {
    let record = &standing.record.value;
    let request = standing.request.map_or("unread", disposition_label);
    let aside = if standing.put_aside {
        " · put aside"
    } else {
        ""
    };
    html! {
        <dl class="room-standing-scope" data-coordination={record.key.clone()}>
            <dt>{"Request"}</dt>
            <dd title={format!("Coordination record {}", record.publication)}>
                {format!("{} → {} · {request}{aside}", record.key, record.target)}
            </dd>
        </dl>
    }
}

const fn disposition_label(disposition: Disposition) -> &'static str {
    match disposition {
        Disposition::Live => "awaiting acknowledgment",
        Disposition::Acked => "acknowledged",
        Disposition::Declined => "declined",
        Disposition::Lapsed => "lapsed",
        Disposition::DeadLettered => "undeliverable",
        Disposition::Canceled => "canceled",
    }
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
    let (kind, label, source) = match reason {
        ParticipantReason::Publication(_) => return None,
        ParticipantReason::Asked(
            DesignNoticeEvidence::Task { task, .. } | DesignNoticeEvidence::TaskClaim { task, .. },
        ) => (
            "invitation",
            "Asked",
            format!(
                "A Design request asked this lane from retained evidence about {}",
                task_label(runtime, task)
            ),
        ),
        ParticipantReason::Asked(DesignNoticeEvidence::Conversation { source }) => (
            "invitation",
            "Asked",
            format!(
                "A Design request asked this lane from retained conversation evidence at member {}, byte {}",
                source.locus.member, source.locus.offset
            ),
        ),
        ParticipantReason::Work(task) => (
            "work",
            "Working",
            format!("Holds {}", task_label(runtime, task)),
        ),
        ParticipantReason::Coordination(key) => (
            "requested",
            "Requested",
            format!("Live coordination request {key}"),
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
