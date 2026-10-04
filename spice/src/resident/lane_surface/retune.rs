//! Durable seat retuning and confirmed live OMP application.
//!
//! The seat remains launch authority across recovery, relaunch, and
//! succession. A running OMP occupant may additionally change immediately,
//! but only through its negotiated command stream and only after the exact
//! correlated response succeeds. Unsupported, ambiguous, refused, or
//! interrupted live changes leave the running identity untouched while the
//! durable seat selection still advances.

use super::super::supervisor::command::{LiveRetune, LiveRetuneResult};
use super::*;

#[derive(Clone, Copy)]
enum Selection<'a> {
    Model(&'a LaneRetune),
    Effort(&'a LaneEffortRetune),
    Fast(&'a LaneFastRetune),
}

/// The seat fact one selection journals, carrying the value it replaced.
enum SeatFact {
    Model(wire::body::LaneRetuned),
    Effort(wire::body::LaneEffortRetuned),
    Fast(wire::body::LaneFastRetuned),
}

impl Selection<'_> {
    fn lane(self) -> CanonicalUuid {
        match self {
            Self::Model(request) => request.lane,
            Self::Effort(request) => request.lane,
            Self::Fast(request) => request.lane,
        }
    }

    fn topology_revision(self) -> u64 {
        match self {
            Self::Model(request) => request.topology_revision,
            Self::Effort(request) => request.topology_revision,
            Self::Fast(request) => request.topology_revision,
        }
    }

    fn validate(self) -> Option<Outcome<CommandAnswer>> {
        match self {
            Self::Model(request) if model_malformed(request.model.as_ref()) => Some(lane_refusal(
                "lane::model-invalid",
                Some("one nonblank model spelling without surrounding whitespace".to_owned()),
                request.model.clone(),
                "trim the model spelling or clear it to follow the vendor configuration",
            )),
            Self::Effort(request)
                if request
                    .effort
                    .as_deref()
                    .is_some_and(|effort| effort.parse::<vendor::Effort>().is_err()) =>
            {
                Some(lane_refusal(
                    "lane::effort-invalid",
                    Some(closed_efforts()),
                    request.effort.clone(),
                    "choose low, medium, high, xhigh, or max; omit it to follow vendor configuration",
                ))
            }
            _ => None,
        }
    }

    fn replace_seat(self, row: &mut wire::LaneResidency) -> SeatFact {
        match self {
            Self::Model(request) => SeatFact::Model(wire::body::LaneRetuned {
                lane: request.lane,
                from: std::mem::replace(&mut row.model, request.model.clone()),
                model: request.model.clone(),
            }),
            Self::Effort(request) => SeatFact::Effort(wire::body::LaneEffortRetuned {
                lane: request.lane,
                from: std::mem::replace(&mut row.effort, request.effort.clone()),
                effort: request.effort.clone(),
            }),
            Self::Fast(request) => SeatFact::Fast(wire::body::LaneFastRetuned {
                lane: request.lane,
                from: std::mem::replace(&mut row.fast_mode, request.fast_mode),
                fast_mode: request.fast_mode,
            }),
        }
    }

    /// The live change this selection asks of the running occupant, if any.
    ///
    /// A fast-mode seat that returns to the global setting still names a
    /// concrete disposition, so `global_fast` resolves it; model and effort
    /// clears keep the running value.
    fn live_request(self, row: &wire::LaneResidency, global_fast: bool) -> Option<LiveRetune> {
        if row.vendor.as_str() != "omp" || row.occupant.is_none() {
            return None;
        }
        let launch = row.session.as_ref()?.launch.as_ref()?;
        match self {
            Self::Effort(request) => {
                let effort = request.effort.as_ref()?;
                (launch.effort != *effort).then(|| LiveRetune::Effort {
                    effort: effort.clone(),
                })
            }
            Self::Model(request) => {
                let model = request.model.as_ref()?;
                if launch.model == *model {
                    return None;
                }
                let (provider, model) = model.split_once('/')?;
                if provider.is_empty() || model.is_empty() {
                    return None;
                }
                Some(LiveRetune::Model {
                    provider: provider.to_owned(),
                    model: model.to_owned(),
                })
            }
            Self::Fast(request) => {
                let enabled = request.fast_mode.unwrap_or(global_fast);
                (launch.fast_mode != enabled).then_some(LiveRetune::Fast { enabled })
            }
        }
    }
}

/// Apply one confirmed live change to the running launch identity.
fn apply_runtime(row: &mut wire::LaneResidency, live: &LiveRetune) {
    let launch = row
        .session
        .as_mut()
        .and_then(|session| session.launch.as_mut())
        .expect("a live request requires recorded launch identity");
    match live {
        LiveRetune::Model { provider, model } => launch.model = format!("{provider}/{model}"),
        LiveRetune::Effort { effort } => launch.effort.clone_from(effort),
        LiveRetune::Fast { enabled } => launch.fast_mode = *enabled,
    }
}

fn runtime_fact(
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    live: &LiveRetune,
) -> wire::body::LaneRuntimeRetuned {
    let mut fact = wire::body::LaneRuntimeRetuned {
        lane,
        occupant,
        model: None,
        effort: None,
        fast_mode: None,
    };
    match live {
        LiveRetune::Model { provider, model } => fact.model = Some(format!("{provider}/{model}")),
        LiveRetune::Effort { effort } => fact.effort = Some(effort.clone()),
        LiveRetune::Fast { enabled } => fact.fast_mode = Some(*enabled),
    }
    fact
}

/// Commit one seated lane's model selection and apply an exact live OMP model
/// when the spelling carries both provider and model identity.
pub(in crate::resident) fn retune(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    command: &wire::AdmittedCommand,
    request: &LaneRetune,
) -> Outcome<CommandAnswer> {
    change(bus, paths, epoch, seats, command, Selection::Model(request))
}

/// Commit one seated lane's effort selection and apply a named live OMP level.
pub(in crate::resident) fn retune_effort(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    command: &wire::AdmittedCommand,
    request: &LaneEffortRetune,
) -> Outcome<CommandAnswer> {
    change(
        bus,
        paths,
        epoch,
        seats,
        command,
        Selection::Effort(request),
    )
}

/// Commit one seated lane's fast-mode selection and apply the disposition it
/// resolves to on a live OMP occupant.
pub(in crate::resident) fn retune_fast(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    command: &wire::AdmittedCommand,
    request: &LaneFastRetune,
) -> Outcome<CommandAnswer> {
    change(bus, paths, epoch, seats, command, Selection::Fast(request))
}

fn change(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    command: &wire::AdmittedCommand,
    selection: Selection<'_>,
) -> Outcome<CommandAnswer> {
    if let Some(refusal) = selection.validate() {
        return refusal;
    }
    let observed = match current_residency(bus, epoch) {
        Ok(head) => head,
        Err(outcome) => return typed_outcome(outcome),
    };
    let topology_revision =
        match next_topology_revision(observed.body(), selection.topology_revision()) {
            Ok(revision) => revision,
            Err(outcome) => return outcome,
        };
    let global_fast = match selection {
        Selection::Fast(_) => match super::super::settings_surface::fast_mode(bus) {
            Ok(fast_mode) => fast_mode,
            Err(outcome) => return typed_outcome(outcome),
        },
        Selection::Model(_) | Selection::Effort(_) => false,
    };
    let mut next = observed.body().clone();
    next.topology_revision = topology_revision;
    let Some(row) = next
        .lanes
        .iter_mut()
        .find(|row| row.lane == selection.lane())
    else {
        return missing_lane(selection.lane());
    };
    let seat_fact = selection.replace_seat(row);
    let live = selection.live_request(row, global_fast).zip(row.occupant);
    let answer = match answer(&next, None, seats, paths) {
        Ok(answer) => answer,
        Err(outcome) => return typed_outcome(outcome),
    };

    let mut write = match write(
        command.actor().clone(),
        epoch,
        Producer::ResidentRecognition,
    ) {
        Ok(write) => write,
        Err(outcome) => return typed_outcome(outcome),
    };
    guard_residency(&mut write, &observed);
    let cause = FactCause::Existing(observed.position());
    let seat_fact = match seat_fact {
        SeatFact::Model(body) => write.fact(body, cause),
        SeatFact::Effort(body) => write.fact(body, cause),
        SeatFact::Fast(body) => write.fact(body, cause),
    };
    let seat_fact = match seat_fact {
        Ok(fact) => fact,
        Err(error) => return typed_store_fault("lane::fact-invalid", &error.to_string()),
    };
    write.residency_head(seat_fact, move |_| next);
    let result = match selection {
        Selection::Model(request) => {
            command_result(&mut write, command, request.clone(), epoch, answer.clone())
        }
        Selection::Effort(request) => {
            command_result(&mut write, command, request.clone(), epoch, answer.clone())
        }
        Selection::Fast(request) => {
            command_result(&mut write, command, request.clone(), epoch, answer.clone())
        }
    };
    if let Err(outcome) = result {
        return typed_outcome(outcome);
    }
    let seated = match commit(bus, write) {
        Ok(committed) => committed,
        Err(outcome) => return typed_outcome(outcome),
    };
    let first = seq(seated.first());
    let last = seq(seated.last());
    // The seat is durable before the occupant is touched, so no later refusal
    // can leave a running change that the journal never heard about. The reply
    // is exactly this transaction's receipt, so a retried command replays it
    // unchanged; a confirmed runtime change is a later residency fact that
    // lane reads observe.
    if let Some((live, occupant)) = live
        && matches!(
            super::super::supervisor::command::retune(
                paths,
                selection.lane(),
                occupant,
                live.clone()
            ),
            Ok(LiveRetuneResult::Applied)
        )
    {
        record_runtime(bus, epoch, command, selection.lane(), occupant, &live);
    }
    Outcome::Reply(Reply {
        position: ReplyPosition::Committed(SeqRange { first, last }),
        body: CommandAnswer::Lane(Box::new(answer)),
    })
}

/// Commits a runtime fact may race before it gives up; each attempt re-reads
/// the head, so only a lane whose residency changes this often goes unrecorded.
const RUNTIME_RECORD_ATTEMPTS: usize = 3;

/// Journal one confirmed live change against the current head, as long as
/// the same occupant still runs; a successor launches from the seat instead.
fn record_runtime(
    bus: &Bus,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    live: &LiveRetune,
) {
    for _ in 0..RUNTIME_RECORD_ATTEMPTS {
        let Ok(observed) = current_residency(bus, epoch) else {
            return;
        };
        let mut next = observed.body().clone();
        let Some(row) = next.lanes.iter_mut().find(|row| row.lane == lane) else {
            return;
        };
        let launched = row
            .session
            .as_ref()
            .is_some_and(|session| session.launch.is_some());
        if row.occupant != Some(occupant) || !launched {
            return;
        }
        apply_runtime(row, live);
        let Ok(mut write) = write(
            command.actor().clone(),
            epoch,
            Producer::ResidentRecognition,
        ) else {
            return;
        };
        guard_residency(&mut write, &observed);
        let Ok(fact) = write.fact(
            runtime_fact(lane, occupant, live),
            FactCause::Existing(observed.position()),
        ) else {
            return;
        };
        write.residency_head(fact, move |_| next);
        if commit(bus, write).is_ok() {
            return;
        }
    }
}
