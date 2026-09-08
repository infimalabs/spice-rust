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
) -> Result<(Vec<StreamDirective>, bool, crate::Stale), String> {
    let admitted = session
        .admit_stream(bytes)
        .map_err(|fault| format!("{}: {}", fault.reason, fault.detail))?;
    if matches!(admitted.get(), wire::StreamFrame::Pong { .. }) {
        return Ok((Vec::new(), false, crate::Stale::default()));
    }
    let stale = stale_reads(&admitted);
    let (directives, changed) = {
        let mut data = data.write();
        let runtime = data
            .as_mut()
            .ok_or_else(|| "stream frame arrived before runtime acceptance".to_owned())?;
        let application = runtime.apply_stream_frame(&admitted);
        let changed = application.changed();
        (application.into_directives(), changed)
    };
    if changed {
        surface.set(data.publication());
    }
    Ok((directives, false, stale))
}

pub(super) fn handle_incoming(
    session: &Session,
    handles: &SurfaceHandles,
    data: &SurfaceData,
    pending: &mut BTreeMap<CanonicalUuid, PendingKind>,
    demands: &mut DemandQueue<ClientAction>,
    bytes: &[u8],
) -> Result<(Vec<StreamDirective>, bool, crate::Stale), String> {
    let crossing: serde_json::Value = wire::decode(bytes).map_err(|error| error.to_string())?;
    if crossing.get("stream").is_some() {
        return handle_stream_frame(session, &handles.surface, data, bytes);
    }
    let identity: ResponseIdentity = wire::decode(bytes).map_err(|error| error.to_string())?;
    let kind = pending
        .remove(&identity.id)
        .ok_or_else(|| format!("response named unknown command {}", identity.id))?;
    let demand = kind.is_demand();
    let demand_response = demand
        .then(|| classify_demand_response(session, bytes))
        .transpose()?;
    if let Some(DemandResponse::Committed(range)) = &demand_response {
        demands.admit(identity.id, *range)?;
    }
    let mut runtime_slot = data.write();
    let runtime = runtime_slot
        .as_mut()
        .ok_or_else(|| "response arrived before runtime acceptance".to_owned())?;
    let application = apply_command_response(session, handles, runtime, identity.id, kind, bytes)?;
    if !demand && application.lane_authority {
        demands.observe_authority();
    }
    if let Some(response) = demand_response {
        let committed = settle_demand(demands, identity.id, application.lane_authority, response)?;
        if application.refresh {
            demands.await_authority(committed);
        }
    }
    drop(runtime_slot);
    handles.surface.set(data.publication());
    Ok((Vec::new(), application.refresh, crate::Stale::default()))
}

struct CommandApplication {
    refresh: bool,
    lane_authority: bool,
}

impl CommandApplication {
    const fn ordinary(refresh: bool) -> Self {
        Self {
            refresh,
            lane_authority: false,
        }
    }
}

fn classify_demand_response(session: &Session, bytes: &[u8]) -> Result<DemandResponse, String> {
    let admitted: crate::Admitted<Response<serde_json::Value>> = admit_response(session, bytes)?;
    classify_demand_outcome(admitted.get().id, &admitted.get().outcome)
}

fn settle_demand(
    demands: &mut DemandQueue<ClientAction>,
    id: CanonicalUuid,
    rebase_next: bool,
    response: DemandResponse,
) -> Result<bool, String> {
    match response {
        DemandResponse::Committed(range) => {
            demands.commit(id, range, rebase_next)?;
            Ok(true)
        }
        DemandResponse::Refused(issue) => {
            demands.refuse(id, issue)?;
            Ok(false)
        }
    }
}

fn apply_command_response(
    session: &Session,
    handles: &SurfaceHandles,
    runtime: &mut CanvasRuntime,
    id: CanonicalUuid,
    kind: PendingKind,
    bytes: &[u8],
) -> Result<CommandApplication, String> {
    Ok(match kind {
        kind @ (PendingKind::Board | PendingKind::Settings) => {
            apply_scalar_response(session, runtime, kind, bytes)?
        }
        // A read of the lane table and an arrangement of it answer with the
        // same authoritative table, so successful replies apply identically.
        // Orgs move only when that answer lands. A refused mutation differs:
        // it means this client lost its optimistic revision, so it immediately
        // re-reads the winning table rather than retaining the stale render.
        PendingKind::Lanes { wakes } => {
            let (refresh, applied) =
                apply_lane_response(session, handles, runtime, &wakes, false, bytes)?;
            CommandApplication {
                refresh,
                lane_authority: applied,
            }
        }
        PendingKind::LaneArrange | PendingKind::LineageArrange => {
            let (refresh, applied) =
                apply_lane_response(session, handles, runtime, &[], true, bytes)?;
            CommandApplication {
                refresh,
                lane_authority: applied,
            }
        }
        PendingKind::LaneRenewal {
            lineage,
            lane,
            requested,
        } => apply_lane_renewal_response(session, lineage, lane, requested, bytes)?,
        PendingKind::Inbox { covered } => CommandApplication::ordinary(apply_inbox_response(
            session, handles, runtime, &covered, bytes,
        )?),
        PendingKind::LaneSeat => {
            let _admitted: crate::Admitted<Response<wire::intent::LaneChangeAnswer>> =
                admit_response(session, bytes)?;
            CommandApplication::ordinary(true)
        }
        PendingKind::WakeSubmission { lane, submission } => CommandApplication::ordinary(
            apply_wake_submission_response(session, handles, submission, lane, bytes)?,
        ),
        PendingKind::LineageConfigure { lineage, life } => {
            let (refresh, applied) = apply_lineage_configuration(
                session,
                &handles.lineage_controls,
                runtime,
                lineage,
                life,
                bytes,
            )?;
            CommandApplication {
                refresh,
                lane_authority: applied,
            }
        }
        PendingKind::Steer => {
            let _admitted: crate::Admitted<Response<wire::intent::DirectiveAnswer>> =
                admit_response(session, bytes)?;
            CommandApplication::ordinary(true)
        }
        PendingKind::SteerSubmission { lane } => CommandApplication::ordinary(
            apply_submission_response(session, handles, runtime, id, lane, bytes)?,
        ),
        PendingKind::Note => {
            let _admitted: crate::Admitted<Response<TaskAnswer>> = admit_response(session, bytes)?;
            CommandApplication::ordinary(false)
        }
    })
}

fn apply_scalar_response(
    session: &Session,
    runtime: &mut CanvasRuntime,
    kind: PendingKind,
    bytes: &[u8],
) -> Result<CommandApplication, String> {
    match kind {
        PendingKind::Board => runtime.apply_board(&admit_response(session, bytes)?),
        PendingKind::Settings => runtime.apply_settings(&admit_response(session, bytes)?),
        _ => unreachable!("scalar response partition is closed"),
    }
    Ok(CommandApplication::ordinary(false))
}

fn apply_lane_response(
    session: &Session,
    handles: &SurfaceHandles,
    runtime: &mut CanvasRuntime,
    wakes: &[CanonicalUuid],
    refresh_on_failure: bool,
    bytes: &[u8],
) -> Result<(bool, bool), String> {
    let admitted: crate::Admitted<Response<wire::intent::LaneAnswer>> =
        admit_response(session, bytes)?;
    let wake_lanes = handles.submissions.queue.borrow().awaiting_lanes(wakes);
    let applied = runtime.apply_lanes(&admitted);
    if let wire::Outcome::Reply(reply) = &admitted.get().outcome
        && applied
    {
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
    Ok((refresh, applied))
}

fn apply_lane_renewal_response(
    session: &Session,
    lineage: CanonicalUuid,
    lane: CanonicalUuid,
    requested: bool,
    bytes: &[u8],
) -> Result<CommandApplication, String> {
    let admitted: crate::Admitted<Response<wire::intent::LaneRenewalAnswer>> =
        admit_response(session, bytes)?;
    if let wire::Outcome::Reply(reply) = &admitted.get().outcome {
        if reply.body.lineage_id != lineage || reply.body.lane != lane {
            return Err(format!(
                "agent renewal response named {}/{}; expected {lineage}/{lane}",
                reply.body.lineage_id, reply.body.lane,
            ));
        }
        if requested != reply.body.renewal.is_some() {
            return Err(format!(
                "agent renewal response for {lineage}/{lane} did not settle requested={requested}",
            ));
        }
    }
    Ok(CommandApplication::ordinary(true))
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
    let accepted = match &admitted.get().outcome {
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
            None
        }
    };
    handles.submissions.publish();
    if let Some(submission) = accepted {
        runtime.apply_steer_submission(&admitted, lane, submission.message, submission.attachments);
    }
    Ok(true)
}

fn apply_wake_submission_response(
    session: &Session,
    handles: &SurfaceHandles,
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
        quote: None,
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
