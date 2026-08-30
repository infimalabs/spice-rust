/// Apply one admitted stream frame, publishing the surface it changed.
///
/// A liveness answer reports no state, and the caller has already taken the
/// only thing it carries by observing that a frame arrived. Answering early
/// skips the surface publication below, which would otherwise redraw an idle
/// board on every probe.
fn handle_stream_frame(
    session: &Session,
    surface: &UseStateHandle<SurfaceData>,
    data: &SurfaceData,
    bytes: &[u8],
) -> Result<
    (
        Vec<StreamDirective>,
        bool,
        crate::Stale,
        Option<wire::StreamCursor>,
    ),
    String,
> {
    let admitted = session
        .admit_stream(bytes)
        .map_err(|fault| format!("{}: {}", fault.reason, fault.detail))?;
    if matches!(admitted.get(), wire::StreamFrame::Pong { .. }) {
        return Ok((Vec::new(), false, crate::Stale::default(), None));
    }
    let stale = stale_reads(&admitted);
    let live_fact = matches!(admitted.get(), wire::StreamFrame::Fact { .. });
    let (directives, refresh_thread, changed) = {
        let mut data = data.write();
        let runtime = data
            .as_mut()
            .ok_or_else(|| "stream frame arrived before runtime acceptance".to_owned())?;
        let application = runtime.apply_stream_frame(&admitted);
        let changed = application.changed();
        let refresh_thread = (live_fact && changed)
            .then(|| runtime.pending_thread())
            .flatten();
        (application.into_directives(), refresh_thread, changed)
    };
    if changed {
        surface.set(data.publication());
    }
    Ok((directives, false, stale, refresh_thread))
}

pub(super) fn handle_incoming(
    session: &Session,
    handles: &SurfaceHandles,
    data: &SurfaceData,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    bytes: &[u8],
) -> Result<
    (
        Vec<StreamDirective>,
        bool,
        crate::Stale,
        Option<wire::StreamCursor>,
    ),
    String,
> {
    let crossing: serde_json::Value = wire::decode(bytes).map_err(|error| error.to_string())?;
    if crossing.get("stream").is_some() {
        return handle_stream_frame(session, &handles.surface, data, bytes);
    }
    let identity: ResponseIdentity = wire::decode(bytes).map_err(|error| error.to_string())?;
    let kind = pending
        .remove(&identity.id)
        .ok_or_else(|| format!("response named unknown command {}", identity.id))?;
    let mut runtime_slot = data.write();
    let runtime = runtime_slot
        .as_mut()
        .ok_or_else(|| "response arrived before runtime acceptance".to_owned())?;
    let refresh = apply_command_response(session, handles, runtime, identity.id, kind, bytes)?;
    drop(runtime_slot);
    handles.surface.set(data.publication());
    Ok((Vec::new(), refresh, crate::Stale::default(), None))
}

fn apply_command_response(
    session: &Session,
    handles: &SurfaceHandles,
    runtime: &mut CanvasRuntime,
    id: CanonicalUuid,
    kind: PendingKind,
    bytes: &[u8],
) -> Result<bool, String> {
    Ok(match kind {
        PendingKind::Board => {
            runtime.apply_board(&admit_response(session, bytes)?);
            false
        }
        // A read of the lane table and an arrangement of it answer with the
        // same authoritative table, so successful replies apply identically.
        // Teams move only when that answer lands. A refused mutation differs:
        // it means this client lost its optimistic revision, so it immediately
        // re-reads the winning table rather than retaining the stale render.
        PendingKind::Lanes { wakes } => {
            apply_lane_response(session, handles, runtime, &wakes, false, bytes)?
        }
        PendingKind::LaneArrange | PendingKind::LineageArrange => {
            apply_lane_response(session, handles, runtime, &[], true, bytes)?
        }
        PendingKind::Inbox { covered } => {
            apply_inbox_response(session, handles, runtime, &covered, bytes)?
        }
        PendingKind::Consent => {
            runtime.apply_consent(&admit_response(session, bytes)?);
            false
        }
        PendingKind::Policy => {
            runtime.apply_policy(&admit_response(session, bytes)?);
            false
        }
        PendingKind::Effort => {
            runtime.apply_effort(&admit_response(session, bytes)?);
            false
        }
        PendingKind::LaneSeat => {
            let admitted: crate::Admitted<Response<wire::intent::LaneChangeAnswer>> =
                admit_response(session, bytes)?;
            runtime.apply_lane_change(&admitted);
            true
        }
        PendingKind::WakeSubmission { lane, submission } => {
            apply_wake_submission_response(session, handles, runtime, submission, lane, bytes)?
        }
        PendingKind::LineageConfigure { lineage, life } => apply_lineage_configuration(
            session,
            &handles.lineage_controls,
            runtime,
            lineage,
            life,
            bytes,
        )?,
        PendingKind::Steer { lane } => {
            let admitted: crate::Admitted<Response<wire::intent::DirectiveAnswer>> =
                admit_response(session, bytes)?;
            runtime.apply_steer(&admitted, lane);
            true
        }
        PendingKind::SteerSubmission { lane } => {
            apply_submission_response(session, handles, runtime, id, lane, bytes)?
        }
        PendingKind::Note(task) => {
            let admitted: crate::Admitted<Response<TaskAnswer>> = admit_response(session, bytes)?;
            runtime.apply_note(&admitted, &task.to_string());
            false
        }
        PendingKind::ConsentDecision => {
            let admitted: crate::Admitted<Response<wire::intent::ConsentAnswer>> =
                admit_response(session, bytes)?;
            runtime.apply_decision(&admitted);
            false
        }
    })
}

fn apply_lane_response(
    session: &Session,
    handles: &SurfaceHandles,
    runtime: &mut CanvasRuntime,
    wakes: &[CanonicalUuid],
    refresh_on_failure: bool,
    bytes: &[u8],
) -> Result<bool, String> {
    let admitted: crate::Admitted<Response<wire::intent::LaneAnswer>> =
        admit_response(session, bytes)?;
    let wake_lanes = handles.submissions.queue.borrow().awaiting_lanes(wakes);
    if let wire::Outcome::Reply(reply) = &admitted.get().outcome {
        for lane in &wake_lanes {
            let row = reply.body.lanes.iter().find(|row| row.lane == *lane);
            if let Some(row) = row
                && row.bound
                && let Some(occupant) = row.occupant
            {
                handles.submissions.queue.borrow_mut().bind_woken_lane(
                    *lane,
                    occupant,
                    reply.body.epoch,
                );
            } else {
                let restored = handles
                    .submissions
                    .queue
                    .borrow_mut()
                    .refuse_woken_lane(*lane)?;
                restore_lane_submissions(
                    handles,
                    *lane,
                    restored,
                    "Agent wake succeeded, but a fresh lane snapshot did not establish an \
                     occupant. Review the lane and retry.",
                );
            }
        }
        handles
            .lineage_controls
            .dispatch(LineageControlAction::SyncLanes {
                answer: reply.body.clone(),
                settle: None,
            });
    } else if let Some(issue) = outcome_issue(
        "Agent woke, but Canvas could not confirm fresh residency",
        &admitted.get().outcome,
    ) {
        for lane in &wake_lanes {
            let restored = handles
                .submissions
                .queue
                .borrow_mut()
                .refuse_woken_lane(*lane)?;
            restore_lane_submissions(handles, *lane, restored, &issue);
        }
    }
    if !wake_lanes.is_empty() {
        handles.submissions.publish();
    }
    let refresh = refresh_on_failure && topology_mutation_requires_refresh(&admitted.get().outcome);
    runtime.apply_lanes(&admitted);
    Ok(refresh)
}

fn apply_inbox_response(
    session: &Session,
    handles: &SurfaceHandles,
    runtime: &mut CanvasRuntime,
    covered: &BTreeMap<CanonicalUuid, String>,
    bytes: &[u8],
) -> Result<bool, String> {
    let admitted: crate::Admitted<Response<wire::intent::InboxAnswer>> =
        admit_response(session, bytes)?;
    let answered = if let wire::Outcome::Reply(reply) = &admitted.get().outcome {
        handles.submissions.queue.borrow_mut().observe_backend(
            reply.body.lanes.iter().flat_map(|lane| lane.pending.iter()),
            covered,
        )?;
        handles.submissions.publish();
        true
    } else {
        false
    };
    runtime.apply_inbox(&admitted);
    Ok(answered && !handles.submissions.queue.borrow().accepted().is_empty())
}

fn apply_submission_response(
    session: &Session,
    handles: &SurfaceHandles,
    runtime: &mut CanvasRuntime,
    id: CanonicalUuid,
    lane: CanonicalUuid,
    bytes: &[u8],
) -> Result<bool, String> {
    let admitted: crate::Admitted<Response<wire::intent::DirectiveAnswer>> =
        admit_response(session, bytes)?;
    match &admitted.get().outcome {
        wire::Outcome::Reply(reply) => handles.submissions.queue.borrow_mut().accept(
            id,
            reply.body.directive,
            reply.body.key.clone(),
            reply.body.disposition,
        )?,
        wire::Outcome::Refusal(_) | wire::Outcome::Fault(_) => {
            let restored = handles.submissions.queue.borrow_mut().refuse(id)?;
            let issue = outcome_issue("Message was not sent", &admitted.get().outcome)
                .expect("the refused or faulted steering outcome has an issue");
            restore_lane_submissions(handles, lane, restored, &issue);
        }
    }
    handles.submissions.publish();
    runtime.apply_steer(&admitted, lane);
    Ok(true)
}

fn apply_wake_submission_response(
    session: &Session,
    handles: &SurfaceHandles,
    runtime: &mut CanvasRuntime,
    submission: CanonicalUuid,
    lane: CanonicalUuid,
    bytes: &[u8],
) -> Result<bool, String> {
    let admitted: crate::Admitted<Response<wire::intent::LaneChangeAnswer>> =
        admit_response(session, bytes)?;
    match &admitted.get().outcome {
        wire::Outcome::Reply(_) => handles
            .submissions
            .queue
            .borrow_mut()
            .wake_accepted(submission)?,
        wire::Outcome::Refusal(_) | wire::Outcome::Fault(_) => {
            let restored = handles
                .submissions
                .queue
                .borrow_mut()
                .refuse_wake(submission)?;
            let issue = outcome_issue("Could not wake this agent", &admitted.get().outcome)
                .expect("the refused or faulted wake outcome has an issue");
            restore_lane_submissions(handles, lane, restored, &issue);
        }
    }
    handles.submissions.publish();
    runtime.apply_lane_change(&admitted);
    Ok(matches!(&admitted.get().outcome, wire::Outcome::Reply(_)))
}

fn restore_lane_submissions(
    handles: &SurfaceHandles,
    lane: CanonicalUuid,
    restored: Vec<crate::composer_drafts::ComposerDraft>,
    issue: &str,
) {
    handles
        .composer_drafts
        .dispatch(ComposerAction::RestoreAll {
            lane: lane.to_string(),
            drafts: restored,
        });
    handles.composer_drafts.dispatch(ComposerAction::SetIssue {
        lane: lane.to_string(),
        issue: issue.to_owned(),
    });
}

fn outcome_issue<T>(action: &str, outcome: &wire::Outcome<T>) -> Option<String> {
    match outcome {
        wire::Outcome::Reply(_) => None,
        wire::Outcome::Refusal(refusal) => Some(format!(
            "{action}: {}. {}",
            refusal.reason(),
            refusal.explanation()
        )),
        wire::Outcome::Fault(fault) => {
            Some(format!("{action}: {}. {}", fault.reason, fault.detail))
        }
    }
}
use super::*;
