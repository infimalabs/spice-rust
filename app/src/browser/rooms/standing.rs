//! A room's standing inside its design menu: why each lane is involved and
//! whether it is available, what the latest coordination asks and whether
//! automation may continue it, and the resident's shadow relevance
//! observations. Every row names the admitted source it traces to. Nothing
//! here is a roster of authority, and rendering it mounts no view and wakes
//! no lane.

use wire::intent::{BoardTask, DesignStanding};
use wire::rooms::{RoomProgressStanding, RoomRelevanceClass, RoomRelevanceVerdict, RoomSourceView};
use wire::{Disposition, TaskStamp};
use yew::prelude::*;

use crate::AppRuntime;
use crate::browser::clock::now_millis;
use crate::runtime::{
    CoordinationStanding, ParticipantReason, Relevance, ResumeAutomation, RoomAnchorAuthority,
    RoomParticipant,
};
use crate::watchband;

const MICROS_PER_MILLI: i64 = 1_000;

const NOT_PROGRESS: &str = "Agreement, silence, message count, elapsed time, ACK and ANS never count as progress or approval.";

/// The standing section of one design's menu and of its read view; `row` is
/// the Design's current task row, when one is known.
pub(super) fn render_standing(
    runtime: &AppRuntime,
    room: &TaskStamp,
    row: Option<&BoardTask>,
) -> Html {
    let projection = runtime.room_projection();
    let participants =
        projection.participants(room, runtime.board(), now_millis() * MICROS_PER_MILLI);
    let coordination = projection.coordination_standing(room);
    let relevance = projection.relevance(room).collect::<Vec<_>>();
    let lifecycle = row.and_then(BoardTask::design_standing);
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
            <p class="room-standing-next" data-room-next="">
                {next_action(settled, lifecycle.as_ref(), coordination.as_ref(), &participants)}
            </p>
            {coordination.as_ref().map_or_else(Html::default, render_coordination)}
            {render_lanes(runtime, &participants)}
            if !relevance.is_empty() {
                <ul class="room-standing-relevance" aria-label="Shadow relevance observations">
                    {for relevance.iter().rev().map(|relevance| render_observation(&relevance.value))}
                </ul>
            }
            <p class="room-standing-note">{NOT_PROGRESS}</p>
        </section>
    }
}

/// The one next useful action the current authority supports.
fn next_action(
    settled: bool,
    lifecycle: Option<&DesignStanding>,
    coordination: Option<&CoordinationStanding<'_>>,
    participants: &[RoomParticipant],
) -> String {
    if settled {
        return "The anchor is settled; nothing remains to coordinate.".to_owned();
    }
    if lifecycle == Some(&DesignStanding::Passive) {
        return "Set the Design's working group to Explore or Converge before coordinating a lane onto it; if ungrouped, group it first."
            .to_owned();
    }
    let owed = participants.iter().any(|participant| {
        participant.reasons.iter().any(|reason| {
            matches!(
                reason,
                ParticipantReason::Receipt(_) | ParticipantReason::Result(_)
            )
        })
    });
    match coordination {
        Some(standing) if standing.request == Some(Disposition::Live) => format!(
            "Wait for {} to acknowledge request {}.",
            standing.record.value.target, standing.record.value.key
        ),
        _ if owed => "Wait for the receipts and results lanes owe below.".to_owned(),
        Some(standing) if matches!(standing.automation, ResumeAutomation::Armed { .. }) => {
            format!(
                "Wait for new input from another party; the rule then resumes {}.",
                standing.record.value.target
            )
        }
        Some(standing) if !standing.put_aside => format!(
            "Read {}'s contribution, then Resume or put the room aside.",
            standing.record.value.target
        ),
        _ => "Start coordination to ask one lane for the next contribution.".to_owned(),
    }
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
            <dt>{if record.automated.is_some() { "Automated resume" } else { "Request" }}</dt>
            <dd title={format!("Coordination record {}", record.publication)}>
                {format!("{} → {} · {request}{aside}", record.key, record.target)}
            </dd>
            if let Some(automated) = &record.automated {
                <dt>{"Input"}</dt>
                <dd>{format!("Step {} answered publication {}", automated.step, automated.evidence)}</dd>
            }
            if let Some(rule) = &record.rule {
                <dt>{"Subject"}</dt><dd>{rule.criterion.clone()}</dd>
                <dt>{"Scope"}</dt><dd>{rule.contribution.clone()}</dd>
                <dt>{"Stop"}</dt><dd>{rule.stop.clone()}</dd>
            }
            <dt>{"Automation"}</dt>
            <dd data-automation={automation_kind(standing.automation)}>{automation_text(standing)}</dd>
        </dl>
    }
}

const fn automation_kind(automation: ResumeAutomation) -> &'static str {
    match automation {
        ResumeAutomation::Off => "off",
        ResumeAutomation::Armed { .. } => "armed",
        ResumeAutomation::Spent { .. } => "spent",
        ResumeAutomation::PutAside { .. } => "put-aside",
        ResumeAutomation::Ended { .. } => "ended",
    }
}

fn automation_text(standing: &CoordinationStanding<'_>) -> String {
    let record = &standing.record.value;
    let rule = record
        .automated
        .as_ref()
        .map_or(&record.publication, |automated| &automated.rule);
    match standing.automation {
        ResumeAutomation::Off => {
            "Off: the latest request carries no rule, so only a manual Start or Resume continues the room."
                .to_owned()
        }
        ResumeAutomation::Armed { limit, spent } => format!(
            "{spent} of {limit} resumes used under rule {rule}; the next waits for {} to acknowledge and another party to publish new input.",
            record.target
        ),
        ResumeAutomation::Spent { limit } => {
            format!("Stopped: all {limit} resumes under rule {rule} were used.")
        }
        ResumeAutomation::PutAside { limit, spent } => {
            format!("Stopped: the room was put aside after {spent} of {limit} resumes.")
        }
        ResumeAutomation::Ended {
            limit,
            spent,
            disposition,
        } => format!(
            "Stopped: the request was {} after {spent} of {limit} resumes.",
            disposition_label(disposition)
        ),
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
    let contributions = participant
        .reasons
        .iter()
        .filter_map(|reason| match reason {
            ParticipantReason::Contribution(publication) => Some(publication),
            _ => None,
        })
        .collect::<Vec<_>>();
    html! {
        <li key={participant.lane.to_string()} data-lane={participant.lane.to_string()}>
            <span class="room-standing-lane">{label}</span>
            <span class="room-standing-availability" data-availability={availability}>{availability}</span>
            if let Some(latest) = contributions.last() {
                {reason_chip(
                    "contribution",
                    &format!("Contributed {}", contributions.len()),
                    &format!("Authored {} room publications; latest {latest}", contributions.len()),
                )}
            }
            {for participant.reasons.iter().filter_map(|reason| render_reason(runtime, reason))}
        </li>
    }
}

/// One non-history reason as a chip naming its source; contribution history
/// renders once per lane instead.
fn render_reason(runtime: &AppRuntime, reason: &ParticipantReason) -> Option<Html> {
    let leave;
    let (kind, label, source) = match reason {
        ParticipantReason::Contribution(_) => return None,
        ParticipantReason::Activity(publication) => (
            "activity",
            "Active",
            format!("Latest contribution {publication} is inside the activity window"),
        ),
        ParticipantReason::Invitation(key) => (
            "invitation",
            "Invited",
            format!("A room packet addressed this lane under key {key}"),
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
        ParticipantReason::Receipt(key) => (
            "receipt",
            "Owes receipt",
            format!("Delivery {key} awaits acknowledgment"),
        ),
        ParticipantReason::Result(result) => (
            "result",
            "Owes result",
            format!("ASK result {result} awaits an answer"),
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
