//! Durable seat retuning and confirmed live OMP application.
//!
//! The seat remains launch authority across recovery, relaunch, and
//! succession. A running OMP occupant may additionally change immediately,
//! but only through its negotiated command stream and only after the exact
//! correlated response succeeds. Unsupported, ambiguous, refused, or
//! interrupted live changes leave the running identity untouched while the
//! durable seat selection still advances.

use super::*;

#[derive(Clone, Copy)]
enum Selection<'a> {
    Model(&'a LaneRetune),
    Effort(&'a LaneEffortRetune),
}

impl Selection<'_> {
    fn lane(self) -> CanonicalUuid {
        match self {
            Self::Model(request) => request.lane,
            Self::Effort(request) => request.lane,
        }
    }

    fn topology_revision(self) -> u64 {
        match self {
            Self::Model(request) => request.topology_revision,
            Self::Effort(request) => request.topology_revision,
        }
    }

    fn value(self) -> Option<String> {
        match self {
            Self::Model(request) => request.model.clone(),
            Self::Effort(request) => request.effort.clone(),
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

    fn replace_seat(self, row: &mut wire::LaneResidency) -> Option<String> {
        match self {
            Self::Model(_) => std::mem::replace(&mut row.model, self.value()),
            Self::Effort(_) => std::mem::replace(&mut row.effort, self.value()),
        }
    }

    fn live_request(
        self,
        row: &wire::LaneResidency,
    ) -> Option<super::super::supervisor::command::LiveRetune> {
        if row.vendor.as_str() != "omp" || row.occupant.is_none() {
            return None;
        }
        let launch = row.session.as_ref()?.launch.as_ref()?;
        match self {
            Self::Effort(request) => {
                let effort = request.effort.as_ref()?;
                (launch.effort != *effort).then(|| {
                    super::super::supervisor::command::LiveRetune::Effort {
                        effort: effort.clone(),
                    }
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
                Some(super::super::supervisor::command::LiveRetune::Model {
                    provider: provider.to_owned(),
                    model: model.to_owned(),
                })
            }
        }
    }

    fn apply_runtime(self, row: &mut wire::LaneResidency) {
        let launch = row
            .session
            .as_mut()
            .and_then(|session| session.launch.as_mut())
            .expect("a live request requires recorded launch identity");
        match self {
            Self::Model(request) => {
                launch.model = request.model.clone().expect("live model is named");
            }
            Self::Effort(request) => {
                launch.effort = request.effort.clone().expect("live effort is named");
            }
        }
    }

    fn runtime_fact(self, occupant: CanonicalUuid) -> wire::body::LaneRuntimeRetuned {
        match self {
            Self::Model(request) => wire::body::LaneRuntimeRetuned {
                lane: request.lane,
                occupant,
                model: request.model.clone(),
                effort: None,
            },
            Self::Effort(request) => wire::body::LaneRuntimeRetuned {
                lane: request.lane,
                occupant,
                model: None,
                effort: request.effort.clone(),
            },
        }
    }
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
    let mut next = observed.body().clone();
    next.topology_revision = topology_revision;
    let Some(row) = next
        .lanes
        .iter_mut()
        .find(|row| row.lane == selection.lane())
    else {
        return missing_lane(selection.lane());
    };
    let from = selection.replace_seat(row);
    let live = selection.live_request(row).zip(row.occupant);
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
    let seat_fact = match selection {
        Selection::Model(request) => write.fact(
            wire::body::LaneRetuned {
                lane: request.lane,
                from,
                model: request.model.clone(),
            },
            FactCause::Existing(observed.position()),
        ),
        Selection::Effort(request) => write.fact(
            wire::body::LaneEffortRetuned {
                lane: request.lane,
                from,
                effort: request.effort.clone(),
            },
            FactCause::Existing(observed.position()),
        ),
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
    let mut answer = answer;
    // The seat is durable before the occupant is touched, so no later refusal
    // can leave a running change that the journal never heard about.
    // The reply position remains this receipt's seat transaction; a confirmed
    // runtime fact can follow unrelated commits and is not part of that range.
    if let Some((request, occupant)) = live {
        let applied = matches!(
            super::super::supervisor::command::retune(paths, selection.lane(), occupant, request),
            Ok(super::super::supervisor::command::LiveRetuneResult::Applied)
        );
        if applied
            && let Some(recorded) =
                record_runtime(bus, paths, epoch, seats, command, selection, occupant)
        {
            answer = recorded;
        }
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
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    command: &wire::AdmittedCommand,
    selection: Selection<'_>,
    occupant: CanonicalUuid,
) -> Option<wire::intent::LaneAnswer> {
    for _ in 0..RUNTIME_RECORD_ATTEMPTS {
        let observed = current_residency(bus, epoch).ok()?;
        let mut next = observed.body().clone();
        let row = next
            .lanes
            .iter_mut()
            .find(|row| row.lane == selection.lane())?;
        let launched = row
            .session
            .as_ref()
            .is_some_and(|session| session.launch.is_some());
        if row.occupant != Some(occupant) || !launched {
            return None;
        }
        selection.apply_runtime(row);
        let answer = answer(&next, None, seats, paths).ok()?;
        let mut write = write(
            command.actor().clone(),
            epoch,
            Producer::ResidentRecognition,
        )
        .ok()?;
        guard_residency(&mut write, &observed);
        let fact = write
            .fact(
                selection.runtime_fact(occupant),
                FactCause::Existing(observed.position()),
            )
            .ok()?;
        write.residency_head(fact, move |_| next);
        if commit(bus, write).is_ok() {
            return Some(answer);
        }
    }
    None
}
