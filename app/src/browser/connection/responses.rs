#[derive(Default)]
pub(super) struct FrameApplication {
    pub(super) directives: Vec<StreamDirective>,
    pub(super) refresh: bool,
    pub(super) stale: crate::Stale,
    /// An applied lane answer arms or cancels the one-shot quiet deadline.
    pub(super) lane_refresh: Option<Option<u32>>,
}

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
    publish: bool,
) -> Result<FrameApplication, String> {
    let admitted = session
        .admit_stream(bytes)
        .map_err(|fault| format!("{}: {}", fault.reason, fault.detail))?;
    if matches!(admitted.get(), wire::StreamFrame::Pong { .. }) {
        return Ok(FrameApplication::default());
    }
    let mut stale = stale_reads(&admitted);
    let (directives, changed) = {
        let mut data = data.write();
        let runtime = data
            .as_mut()
            .ok_or_else(|| "stream frame arrived before runtime acceptance".to_owned())?;
        let application = runtime.apply_stream_frame(&admitted);
        let changed = application.changed();
        if changed && let wire::StreamFrame::MessageGeneration { generation, .. } = admitted.get() {
            for message in &generation.messages {
                stale.invalidate_quiet_lane(runtime.lanes(), message.lane);
            }
        }
        stale.retain_task_details(|task| changed && runtime.invalidate_task_detail(task));
        stale.resolve_task_relations(runtime, changed);
        stale.resolve_work_history(runtime, changed);
        (application.into_directives(), changed)
    };
    if changed && publish {
        surface.set(data.publication());
    }
    Ok(FrameApplication {
        directives,
        stale,
        ..FrameApplication::default()
    })
}

pub(super) fn handle_incoming(
    session: &Session,
    handles: &SurfaceHandles,
    data: &SurfaceData,
    actions: &ActionQueue,
    bytes: &[u8],
    publish: bool,
) -> Result<FrameApplication, String> {
    let crossing: serde_json::Value = wire::decode(bytes).map_err(|error| error.to_string())?;
    if crossing.get("stream").is_some() {
        return handle_stream_frame(session, &handles.surface, data, bytes, publish);
    }
    let identity: ResponseIdentity = wire::decode(bytes).map_err(|error| error.to_string())?;
    let ordinary_kind = actions.settle_pending(identity.id);
    let topology_kind = actions
        .map_in_flight_topology(identity.id, topology_pending_kind)
        .ok();
    let (kind, topology) = match (ordinary_kind, topology_kind) {
        (Some(_), Some(_)) => {
            return Err(format!(
                "response identity {} names both ordinary and topology commands",
                identity.id
            ));
        }
        (Some(kind), None) => (kind, false),
        (None, Some(kind)) => (kind, true),
        (None, None) => {
            return Err(format!("response named unknown command {}", identity.id));
        }
    };
    let topology_response = topology
        .then(|| classify_topology_response(session, bytes))
        .transpose()?;
    if let Some(TopologyResponse::Committed(range)) = &topology_response {
        actions.admit_topology(identity.id, *range)?;
    }
    let mut runtime_slot = data.write();
    let runtime = runtime_slot
        .as_mut()
        .ok_or_else(|| "response arrived before runtime acceptance".to_owned())?;
    let application = apply_command_response(session, handles, runtime, identity.id, kind, bytes)?;
    if !topology && application.lane_authority {
        actions.observe_topology_authority();
    }
    if let Some(response) = topology_response {
        let committed =
            settle_topology(actions, identity.id, application.lane_authority, response)?;
        if application.refresh {
            actions.await_topology_authority(committed);
        }
    }
    let lane_refresh = application.lane_authority.then(|| {
        runtime
            .lanes()
            .expect("an applied lane answer installs its snapshot")
            .refresh_after_millis
    });
    drop(runtime_slot);
    if publish {
        handles.surface.set(data.publication());
    }
    Ok(FrameApplication {
        refresh: application.refresh,
        stale: application.stale,
        lane_refresh,
        ..FrameApplication::default()
    })
}

struct CommandApplication {
    refresh: bool,
    lane_authority: bool,
    stale: crate::Stale,
}

impl CommandApplication {
    fn ordinary(refresh: bool) -> Self {
        Self {
            refresh,
            lane_authority: false,
            stale: crate::Stale::default(),
        }
    }

    fn task_detail(task: wire::TaskStamp) -> Self {
        let mut stale = crate::Stale::default();
        stale.invalidate_task_detail_read(task);
        Self {
            refresh: false,
            lane_authority: false,
            stale,
        }
    }
}

enum TopologyResponse {
    Committed(wire::SeqRange),
    Refused,
}

fn classify_topology_response(session: &Session, bytes: &[u8]) -> Result<TopologyResponse, String> {
    let admitted: crate::Admitted<Response<serde_json::Value>> = admit_response(session, bytes)?;
    match &admitted.get().outcome {
        wire::Outcome::Reply(reply) => match reply.position {
            wire::ReplyPosition::Committed(range) => Ok(TopologyResponse::Committed(range)),
            wire::ReplyPosition::Snapshot(_) | wire::ReplyPosition::Direct {} => Err(format!(
                "topology command {} returned a non-committed position",
                admitted.get().id
            )),
        },
        wire::Outcome::Refusal(_) | wire::Outcome::Fault(_) => Ok(TopologyResponse::Refused),
    }
}

fn settle_topology(
    actions: &ActionQueue,
    id: CanonicalUuid,
    rebase_next: bool,
    response: TopologyResponse,
) -> Result<bool, String> {
    match response {
        TopologyResponse::Committed(range) => {
            actions.settle_topology(id, Some(range), rebase_next)?;
            Ok(true)
        }
        TopologyResponse::Refused => {
            actions.settle_topology(id, None, false)?;
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
        kind @ (PendingKind::Board | PendingKind::Settings | PendingKind::Vendors) => {
            apply_scalar_response(session, runtime, kind, bytes)?
        }
        PendingKind::TaskDetail { task } => {
            let admitted: crate::Admitted<Response<wire::intent::TaskDetailAnswer>> =
                admit_response(session, bytes)?;
            runtime.apply_task_detail(&task, &admitted);
            reconcile_note_detail(handles, runtime, &task)
        }
        PendingKind::TaskRelations { key } => {
            let admitted: crate::Admitted<Response<wire::intent::TaskRelationsAnswer>> =
                admit_response(session, bytes)?;
            runtime.apply_task_relations(&key, &admitted);
            CommandApplication::ordinary(false)
        }
        PendingKind::WorkHistory { request } => {
            let admitted: crate::Admitted<Response<wire::intent::BoardAnswer>> =
                admit_response(session, bytes)?;
            runtime.apply_work_history(&request, &admitted);
            CommandApplication::ordinary(false)
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
                stale: crate::Stale::default(),
            }
        }
        PendingKind::TeamArrange => {
            let (refresh, applied) =
                apply_lane_response(session, handles, runtime, &[], true, bytes)?;
            CommandApplication {
                refresh,
                lane_authority: applied,
                stale: crate::Stale::default(),
            }
        }
        PendingKind::LaneRenewal {
            team,
            lane,
            requested,
        } => apply_lane_renewal_response(session, handles, team, lane, requested, bytes)?,
        PendingKind::Inbox { covered } => {
            apply_inbox_response(session, handles, runtime, &covered, bytes)?
        }
        PendingKind::LaneSeat => {
            let _admitted: crate::Admitted<Response<wire::intent::LaneChangeAnswer>> =
                admit_response(session, bytes)?;
            CommandApplication::ordinary(true)
        }
        PendingKind::WakeSubmission { lane, submission } => CommandApplication::ordinary(
            apply_wake_submission_response(session, handles, submission, lane, bytes)?,
        ),
        PendingKind::TeamConfigure { team, life } => {
            let (refresh, applied) = apply_team_configuration(
                session,
                &handles.team_controls,
                runtime,
                team,
                life,
                bytes,
            )?;
            CommandApplication {
                refresh,
                lane_authority: applied,
                stale: crate::Stale::default(),
            }
        }
        PendingKind::Steer => {
            let _admitted: crate::Admitted<Response<wire::intent::DirectiveAnswer>> =
                admit_response(session, bytes)?;
            CommandApplication::ordinary(true)
        }
        PendingKind::SteerSubmission { lane } => CommandApplication::ordinary(
            apply_submission_response(session, handles, id, lane, bytes)?,
        ),
        PendingKind::Note { task, text, after } => {
            apply_note_response(session, handles, runtime, task, text, after, bytes)?
        }
    })
}
fn apply_note_response(
    session: &Session,
    handles: &SurfaceHandles,
    runtime: &mut CanvasRuntime,
    task: wire::TaskStamp,
    text: String,
    after: Option<wire::Seq>,
    bytes: &[u8],
) -> Result<CommandApplication, String> {
    let admitted: crate::Admitted<Response<TaskAnswer>> = admit_response(session, bytes)?;
    match &admitted.get().outcome {
        wire::Outcome::Reply(reply) => {
            let answered = reply
                .body
                .task
                .born()
                .map_err(|error| format!("note answer named a non-canonical task: {error}"))?;
            if answered != task {
                handles.notes.update(&task, |composer| {
                    composer.submission = NoteSubmission::Failed(
                        "The resident accepted an observation for a different task; the draft was retained."
                            .to_owned(),
                    );
                });
                return Err("note answer task identity did not match its command".to_owned());
            }
            handles.notes.update(&task, |composer| {
                *composer = NoteComposer {
                    submission: NoteSubmission::Reconciling { text, after },
                    ..NoteComposer::default()
                };
            });
            refresh_task_detail(runtime, &task);
            Ok(CommandApplication::task_detail(task))
        }
        outcome => {
            let issue = outcome_issue("Observation was not saved", outcome)
                .unwrap_or_else(|| "Observation was not saved.".to_owned());
            handles.notes.update(&task, |composer| {
                composer.submission = NoteSubmission::Failed(issue);
            });
            Ok(CommandApplication::ordinary(false))
        }
    }
}

fn refresh_task_detail(runtime: &mut CanvasRuntime, task: &wire::TaskStamp) {
    if !runtime.invalidate_task_detail(task) {
        runtime.begin_task_detail(task.clone());
    }
}

fn reconcile_note_detail(
    handles: &SurfaceHandles,
    runtime: &mut CanvasRuntime,
    task: &wire::TaskStamp,
) -> CommandApplication {
    let NoteSubmission::Reconciling { text, after } = handles.notes.submission(task) else {
        return CommandApplication::ordinary(false);
    };
    match runtime.task_detail(task) {
        Some(TaskDetailState::Available(detail) | TaskDetailState::Refreshing(detail)) => {
            let actor = runtime.principal().actor();
            let visible = detail.notes.iter().any(|note| {
                note.author == *actor
                    && note.text == text
                    && after.is_none_or(|position| note.position > position)
            });
            if visible {
                handles.notes.update(task, |composer| {
                    composer.submission = NoteSubmission::Idle;
                });
                CommandApplication::ordinary(false)
            } else {
                refresh_task_detail(runtime, task);
                CommandApplication::task_detail(task.clone())
            }
        }
        Some(TaskDetailState::Unavailable { reason, detail }) => {
            let issue = detail.as_ref().map_or_else(
                || format!("Observation was saved, but its detail could not be refreshed: {reason}"),
                |detail| {
                    format!(
                        "Observation was saved, but its detail could not be refreshed: {reason}. {detail}"
                    )
                },
            );
            handles.notes.update(task, |composer| {
                composer.submission = NoteSubmission::Failed(issue);
            });
            CommandApplication::ordinary(false)
        }
        Some(TaskDetailState::Loading) | None => CommandApplication::task_detail(task.clone()),
    }
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
        PendingKind::Vendors => runtime.apply_vendor_catalog(&admit_response(session, bytes)?),
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
                restore_submissions(
                    handles,
                    restored,
                    "Agent wake succeeded, but a fresh lane snapshot did not establish an \
                     occupant. Review the lane and retry.",
                );
            }
        }
        handles
            .team_controls
            .dispatch(TeamControlAction::SyncLanes {
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
            restore_submissions(handles, restored, &issue);
        }
    }
    if !wake_lanes.is_empty() {
        handles.submissions.publish();
    }
    let refresh = refresh_on_failure && topology_mutation_requires_refresh(&admitted.get().outcome);
    Ok((refresh, applied))
}

/// Prefix of every renewal refusal shown in the lane composer, so a later
/// success clears only the issue this family raised.
const RENEWAL_ISSUE: &str = "Could not renew this agent";

fn apply_lane_renewal_response(
    session: &Session,
    handles: &SurfaceHandles,
    team: CanonicalUuid,
    lane: CanonicalUuid,
    requested: bool,
    bytes: &[u8],
) -> Result<CommandApplication, String> {
    let admitted: crate::Admitted<Response<wire::intent::LaneRenewalAnswer>> =
        admit_response(session, bytes)?;
    match &admitted.get().outcome {
        wire::Outcome::Reply(reply) => {
            if reply.body.team != team || reply.body.lane != lane {
                return Err(format!(
                    "agent renewal response named {}/{}; expected {team}/{lane}",
                    reply.body.team, reply.body.lane,
                ));
            }
            if requested != reply.body.renewal.is_some() {
                return Err(format!(
                    "agent renewal response for {team}/{lane} did not settle requested={requested}",
                ));
            }
            handles
                .composer_drafts
                .dispatch(ComposerAction::ClearIssue {
                    lane: lane.to_string(),
                    quote: None,
                    family: RENEWAL_ISSUE,
                });
        }
        outcome => {
            // The control itself only knows durable lane state, so the refusal
            // of this one attempt lands beside it as the lane composer issue,
            // exactly where a refused wake already reports.
            let issue = outcome_issue(RENEWAL_ISSUE, outcome)
                .expect("the refused or faulted renewal outcome has an issue");
            handles.composer_drafts.dispatch(ComposerAction::SetIssue {
                lane: lane.to_string(),
                quote: None,
                issue,
            });
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
) -> Result<CommandApplication, String> {
    let admitted: crate::Admitted<Response<wire::intent::InboxAnswer>> =
        admit_response(session, bytes)?;
    let mut stale = crate::Stale::default();
    let answered = if let wire::Outcome::Reply(reply) = &admitted.get().outcome {
        if handles.submissions.queue.borrow_mut().observe_backend(
            reply.body.lanes.iter().flat_map(|lane| lane.pending.iter()),
            covered,
        )? {
            stale.invalidate_lanes();
        }
        handles.submissions.publish();
        true
    } else {
        false
    };
    runtime.apply_inbox(&admitted);
    Ok(CommandApplication {
        refresh: answered && !handles.submissions.queue.borrow().accepted().is_empty(),
        lane_authority: false,
        stale,
    })
}

fn apply_submission_response(
    session: &Session,
    handles: &SurfaceHandles,
    id: CanonicalUuid,
    lane: CanonicalUuid,
    bytes: &[u8],
) -> Result<bool, String> {
    let admitted: crate::Admitted<Response<wire::intent::DirectiveAnswer>> =
        admit_response(session, bytes)?;
    match &admitted.get().outcome {
        wire::Outcome::Reply(reply) => {
            let _ = handles.submissions.queue.borrow_mut().accept(
                id,
                reply.body.directive,
                reply.body.key.clone(),
                reply.body.disposition,
            )?;
        }
        wire::Outcome::Refusal(_) | wire::Outcome::Fault(_) => {
            let restore_lane = handles.submissions.queue.borrow().restore_lane(id)?;
            let restored = handles.submissions.queue.borrow_mut().refuse(id)?;
            let action = if restore_lane == lane {
                "Message was not sent".to_owned()
            } else {
                format!("Broadcast was not sent to agent {lane}")
            };
            let issue = outcome_issue(&action, &admitted.get().outcome)
                .expect("the refused or faulted steering outcome has an issue");
            restore_submissions(handles, restored, &issue);
        }
    }
    handles.submissions.publish();
    Ok(true)
}

fn apply_wake_submission_response(
    session: &Session,
    handles: &SurfaceHandles,
    submission: CanonicalUuid,
    _lane: CanonicalUuid,
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
            restore_submissions(handles, restored, &issue);
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
    let lane_key = lane.to_string();
    let current =
        crate::browser::watchband::current_composer_draft(&lane_key, &handles.composer_drafts);
    handles
        .composer_drafts
        .dispatch(ComposerAction::RestoreAll {
            lane: lane_key,
            drafts: restored,
            current,
        });
    handles.composer_drafts.dispatch(ComposerAction::SetIssue {
        lane: lane.to_string(),
        quote: None,
        issue: issue.to_owned(),
    });
}

fn restore_submissions(
    handles: &SurfaceHandles,
    restored: Vec<crate::composer_submissions::SubmissionRestore>,
    issue: &str,
) {
    let mut by_lane = BTreeMap::<CanonicalUuid, Vec<crate::composer_drafts::ComposerDraft>>::new();
    for restoration in restored {
        by_lane
            .entry(restoration.lane)
            .or_default()
            .push(restoration.draft);
    }
    for (lane, drafts) in by_lane {
        restore_lane_submissions(handles, lane, drafts, issue);
    }
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
