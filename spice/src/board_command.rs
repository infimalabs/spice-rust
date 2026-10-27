//! Board, task, and authored-document commands over the sole resident door.

use std::io::Read as _;

use board::{Entry, Project, View};
use bus::{Bus, CommitOutcome};
use clap::ArgMatches;
use journal::{FactCause, GuardError, JournalGuard, JournalRead, JournalWrite, PendingFact};
use spice::ambient::Ambient;
use spice::repository::Repository;
use wire::body::GitBoundary;
use wire::intent::{
    BoardAnswer, BoardGraph, BoardHandout, BoardRead, BoardTask, ClaimRenew, ClaimWin,
    CommandAnswer, DocApply, DocLedger, NextWorkCounts, NextWorkState, NextWorkTask, Positioned,
    ReviewFollowup, TASK_HISTORY_PAGE_LIMIT_MAX, TASK_HISTORY_QUERY_BYTES_MAX, TaskAdd, TaskClose,
    TaskConclude, TaskDefer, TaskDelete, TaskDepends, TaskDone, TaskEdit, TaskFile,
    TaskHistoryCursor, TaskHistoryPage, TaskNote, TaskOopsAdd, TaskRef, TaskReview, TaskWake,
    TextAnswer,
};
use wire::rooms::{
    RoomListenerAvailability, RoomListenerDelivery, RoomPost, RoomPostAnswer, RoomPostListener,
    RoomPostNext, RoomPostOutcome, RoomPublicationContent, RoomPublished,
};
use wire::{
    Actor, ClaimDisposition, ClaimHead, Command, CommandKind, Digest, Domain, Fault, FaultClass,
    Instant, LeaseHead, Name, Outcome, Producer, Refusal, Reply, ReplyPosition, Seq, SeqRange,
    TaskHandle, TaskHead, TaskStamp, WireVersion, Witness, WorkflowHead, body,
};

use crate::cli::Contract;
use crate::command_client::Host;

fn claim_capture_from_board(board: &BoardAnswer) -> gate::ClaimCapture {
    let mut active = board.tasks.iter().filter(|task| {
        task.claim.as_ref().is_some_and(|claim| {
            claim.body.actor == board.principal
                && claim.body.disposition == ClaimDisposition::Active
        })
    });
    let Some(task) = active.next() else {
        return gate::ClaimCapture::Idle;
    };
    if active.next().is_some() {
        return gate::ClaimCapture::Absent {
            reason: "board claim read found more than one active claim for this principal"
                .to_owned(),
        };
    }
    gate::ClaimCapture::Active {
        task: task.statement.body.handle.clone(),
        phase: task.workflow.body.phase.clone(),
    }
}

fn require_no_planning_implementation(
    board: &BoardAnswer,
    action: &str,
) -> Result<(), Outcome<CommandAnswer>> {
    match claim_capture_from_board(board) {
        gate::ClaimCapture::Active { task, phase }
            if gate::lacks_commit_authority(phase.as_str()) =>
        {
            Err(refusal(
                "task::planning-implementation",
                Some(format!(
                    "active task={task}; phase={phase}; refused action={action}"
                )),
                concat!(
                    "this phase holds no commit authority: record planning findings with task ",
                    "add, task modify, task depends, task note, or task artifact add; ",
                    "only a claimed implementation phase changes repository history",
                ),
            ))
        }
        gate::ClaimCapture::Absent { reason } => {
            Err(protocol_fault("claim::active-ambiguous", &reason))
        }
        _ => Ok(()),
    }
}

pub(crate) use spice::claim_lease::{
    AUTOMATIC_MAXIMUM_SECONDS, AUTOMATIC_PROVISIONAL_SECONDS, RECLAIM_HORIZON_SECONDS,
};
pub(crate) const AUTOMATIC_HEARTBEAT_SECONDS: i64 = 20;
pub(crate) const AUTOMATIC_ROLLING_SECONDS: i64 = 100;

#[cfg(test)]
pub(crate) use read_guard::next_work::read_next_work;
pub(crate) use read_guard::next_work::{OccupantMatch, OccupantProfiles, floored_standing};
const CAPTURE_WORK_RULE: &str = "capturework";
const RECOVER_STEAL_RULE: &str = "recoversteal";
fn reclaim_authority_refusal(reason: &str, observed: Option<String>) -> Outcome<CommandAnswer> {
    let command = wire::SpiceCommand::new("task", ["status"])
        .expect("task status is a structural Spice recovery command");
    Outcome::Refusal(Refusal::repairable(
        command,
        wire::RefusalDiagnostic::new(reason, None, None, observed).with_detail(
            "run spice task status to read current authority; when it reports no claim for this lane, run spice task next",
        ),
    ))
}

// Ordinary `mod` children, never `include!`: a child module already reads its
// ancestors' private items, so the guard tests below call `claim_plan` and
// `commit_claim` unchanged, and the formatters gate covers each file by
// construction rather than by an enumeration kept by hand.
mod client;
pub(super) use client::*;
mod client_document;
use client_document::*;
mod dependency_witness;
use dependency_witness::*;
mod design_return;
pub(crate) use design_return::{ReturnOwed, owed_returns, record_return};
mod followup;
use followup::*;
mod handoff;
pub(crate) use handoff::{phase_handoff, queued_at};
mod next_action;
use next_action::*;
mod automatic_lifecycle;
pub(crate) use automatic_lifecycle::{
    AutomaticClaim, AutomaticSeat, claim_automatic, is_automatic_claim, release_automatic,
};
mod mutation;
pub(crate) use mutation::task_defer;
use mutation::*;
mod planning;
use planning::*;
mod preparation;
pub(crate) use preparation::nudge::design_nudge;
use preparation::*;
mod progress;
use progress::*;
mod group;
pub(crate) use group::group_fast_lanes;
use group::*;
mod read_guard;
use read_guard::*;
mod report;
use report::read_report;
mod room;
use room::room_post;
pub(crate) use room::{
    derive_attention, project_attention, read_room_duty_rows, room_duties, room_publications_after,
};
mod room_control;
pub(crate) use room_control::room_control;
mod room_leave;
pub(crate) use room_leave::room_leave;
mod room_contribute;
pub(crate) use room_contribute::room_contribute;
mod deactivation;
use deactivation::Deactivation;
mod steering_origin;
use steering_origin::*;
mod task_step;
use task_step::*;
mod task_text;
pub(crate) use task_text::{
    TaskText, collapsed_acceptance, collapsed_description, current_task, replace_text,
};
#[cfg(test)]
use task_text::{acceptance_items, text_edit};
mod taste_review;
use taste_review::*;

pub(crate) fn parse_origin(
    matches: &ArgMatches,
) -> Result<Option<wire::Origin>, Outcome<CommandAnswer>> {
    matches
        .get_one::<String>("origin")
        .map(|written| {
            written.parse::<wire::Origin>().map_err(|error| {
                refusal(
                    "task::origin-invalid",
                    Some(written.clone()),
                    &error.to_string(),
                )
            })
        })
        .transpose()
}

#[cfg(test)]
mod tests;
