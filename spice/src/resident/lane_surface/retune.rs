//! Durable seat retuning and confirmed live OMP application.
//!
//! The seat remains launch authority across recovery, relaunch, and
//! succession. A running OMP occupant may additionally change immediately,
//! but only through its negotiated command stream and only after the exact
//! correlated response succeeds. Unsupported, ambiguous, refused, or
//! interrupted live changes leave the running identity untouched while the
//! durable seat selection still advances.

use std::collections::BTreeSet;

use super::super::settings_surface::FastLevels;
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
                    "choose low, medium, high, xhigh, or max; clear it to follow vendor configuration",
                ))
            }
            _ => None,
        }
    }

    fn replace_seat(self, row: &mut wire::LaneResidency) -> SeatFact {
        match self {
            Self::Model(request) => {
                row.forget_fast_refusal();
                SeatFact::Model(wire::body::LaneRetuned {
                    lane: request.lane,
                    from: std::mem::replace(&mut row.model, request.model.clone()),
                    model: request.model.clone(),
                })
            }
            Self::Effort(request) => SeatFact::Effort(wire::body::LaneEffortRetuned {
                lane: request.lane,
                from: std::mem::replace(&mut row.effort, request.effort.clone()),
                effort: request.effort.clone(),
            }),
            Self::Fast(request) => {
                if request.fast_mode {
                    row.forget_fast_refusal();
                }
                SeatFact::Fast(wire::body::LaneFastRetuned {
                    lane: request.lane,
                    from: std::mem::replace(&mut row.fast_mode, request.fast_mode),
                    fast_mode: request.fast_mode,
                })
            }
        }
    }

    /// The live model or effort change this selection asks of the running
    /// occupant, if any. Clears keep the running value; fast mode is
    /// resolved across its levels by [`fast_flips`] instead. The recorded
    /// launch can lag a change still awaiting confirmation, so it is not
    /// compared; recording skips a confirmation that changes nothing.
    fn live_request(self, row: &wire::LaneResidency) -> Option<LiveRetune> {
        if row.vendor.as_str() != "omp" || row.occupant.is_none() {
            return None;
        }
        match self {
            Self::Effort(request) => request
                .effort
                .clone()
                .map(|effort| LiveRetune::Effort { effort }),
            Self::Model(request) => {
                let (provider, model) = request.model.as_ref()?.split_once('/')?;
                if provider.is_empty() || model.is_empty() {
                    return None;
                }
                Some(LiveRetune::Model {
                    provider: provider.to_owned(),
                    model: model.to_owned(),
                })
            }
            Self::Fast(_) => None,
        }
    }
}

/// One live fast-mode change a running OMP member owes after a write.
pub(super) struct FastFlip {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    live: LiveRetune,
}

/// The live fast-mode changes owed by one residency or global settings write.
///
/// `levels` resolve `before`; `after` resolves at the same levels with the
/// global setting `after_global`. A member owes one only when the write flips
/// its effective fast mode, resolved additively across every level, and it
/// runs an OMP launch with recorded identity. That identity can lag a change
/// still awaiting confirmation, so it is not compared.
pub(super) fn fast_flips(
    before: &wire::ResidencyHead,
    after: &wire::ResidencyHead,
    levels: &FastLevels,
    after_global: bool,
) -> Vec<FastFlip> {
    after
        .lanes
        .iter()
        .filter_map(|row| {
            let enabled = levels.lane_under(after_global, after, row);
            let prior = before.lanes.iter().find(|prior| prior.lane == row.lane)?;
            if levels.lane(before, prior) == enabled || row.vendor.as_str() != "omp" {
                return None;
            }
            let occupant = row.occupant?;
            Some(FastFlip {
                lane: row.lane,
                occupant,
                live: LiveRetune::Fast { enabled },
            })
        })
        .collect()
}

/// Owe each running member whose effective fast mode a global setting change
/// to `after` flips its live change. `levels` hold the setting it replaced.
/// The setting is already durable.
pub(in crate::resident) fn owe_global_fast_flips(
    effects: &mut CommandEffects,
    command: &wire::AdmittedCommand,
    residency: &wire::ResidencyHead,
    levels: &FastLevels,
    after: bool,
) {
    owe_fast_flips(
        effects,
        command,
        fast_flips(residency, residency, levels, after),
    );
}

/// Owe each running member its fast-mode change. The write that owed them
/// is already durable.
pub(super) fn owe_fast_flips(
    effects: &mut CommandEffects,
    command: &wire::AdmittedCommand,
    flips: Vec<FastFlip>,
) {
    for flip in flips {
        effects.owe_live(LiveChange::new(
            command,
            flip.lane,
            flip.occupant,
            flip.live,
        ));
    }
}

/// The live fast-mode changes owed when the lanes fast-flagged working groups
/// run fast move from `before` to those `levels` resolves: one per running
/// OMP member that gained or lost group fast mode while no other level holds
/// it, so a change that leaves effective fast mode alone owes nothing.
pub(super) fn group_flips(
    head: &wire::ResidencyHead,
    before: &BTreeSet<CanonicalUuid>,
    levels: &FastLevels,
) -> Vec<FastFlip> {
    before
        .symmetric_difference(levels.groups())
        .filter_map(|lane| {
            let row = head.lanes.iter().find(|row| row.lane == *lane)?;
            if levels.held(head, row) || row.vendor.as_str() != "omp" {
                return None;
            }
            Some(FastFlip {
                lane: *lane,
                occupant: row.occupant?,
                live: LiveRetune::Fast {
                    enabled: levels.groups().contains(lane),
                },
            })
        })
        .collect()
}

/// The lanes fast-flagged working groups last ran fast. Claims, design moves,
/// and group flags change that set through many writers, so the resident
/// settles it after every loop turn rather than at each writer.
struct GroupFast {
    lanes: BTreeSet<CanonicalUuid>,
}

impl GroupFast {
    /// The live changes owed since the last settlement, as the resident's own
    /// derived act.
    fn settle(&mut self, bus: &Bus, epoch: CanonicalUuid) -> CommandResult<Vec<LiveChange>> {
        let levels = FastLevels::read(bus)?;
        if *levels.groups() == self.lanes {
            return Ok(Vec::new());
        }
        let head = current_residency(bus, epoch)?;
        let flips = group_flips(head.body(), &self.lanes, &levels);
        self.lanes = levels.into_groups();
        Ok(flips
            .into_iter()
            .map(|flip| LiveChange {
                actor: wire::Actor::Spice,
                lane: flip.lane,
                occupant: flip.occupant,
                live: flip.live,
            })
            .collect())
    }
}

/// One live change a running OMP occupant owes after its durable write.
///
/// Asking the occupant waits on the provider, so it runs outside the
/// resident loop; only a confirmed change returns to be journaled.
pub(in crate::resident) struct LiveChange {
    actor: wire::Actor,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    live: LiveRetune,
}

impl LiveChange {
    fn new(
        command: &wire::AdmittedCommand,
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        live: LiveRetune,
    ) -> Self {
        Self {
            actor: command.actor().clone(),
            lane,
            occupant,
            live,
        }
    }

    /// Ask the occupant's supervisor for this change and wait for the
    /// provider's correlated answer; true only when the provider applied it.
    pub(in crate::resident) fn confirm(&self, paths: &RuntimePaths) -> bool {
        matches!(
            super::super::supervisor::command::retune(
                paths,
                self.lane,
                self.occupant,
                self.live.clone(),
            ),
            Ok(LiveRetuneResult::Applied)
        )
    }

    /// Journal this confirmed change against the current head.
    pub(in crate::resident) fn record(&self, bus: &Bus, epoch: CanonicalUuid) {
        record_runtime(
            bus,
            epoch,
            &self.actor,
            self.lane,
            self.occupant,
            &self.live,
            None,
        );
    }
}

/// Journal that a running occupant's vendor refused fast mode in these words
/// and the occupant turned it off to continue without it.
pub(in crate::resident) fn record_fast_refusal(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    refusal: &str,
) {
    record_runtime(
        bus,
        epoch,
        &wire::Actor::Spice,
        lane,
        occupant,
        &LiveRetune::Fast { enabled: false },
        Some(refusal),
    );
}

/// Asks running occupants for owed live changes on one worker outside the
/// resident loop, one at a time in the order their writes committed, so a
/// later change to the same occupant is never applied before an earlier one.
/// Confirmed changes return through [`LiveChanges::recv`] for the loop to
/// journal, which keeps the loop the only residency writer.
pub(in crate::resident) struct LiveChanges {
    owed: Option<mpsc::Sender<LiveChange>>,
    confirmed: tokio::sync::mpsc::UnboundedReceiver<LiveChange>,
    worker: Option<std::thread::JoinHandle<()>>,
    groups: GroupFast,
}

impl LiveChanges {
    /// Start the worker, settling working-group fast mode from the lanes the
    /// journal's groups run fast now.
    pub(in crate::resident) fn start(bus: &Bus, paths: &RuntimePaths) -> CommandResult<Self> {
        let groups = GroupFast {
            lanes: FastLevels::read(bus)?.into_groups(),
        };
        let (owed, asks) = mpsc::channel::<LiveChange>();
        let (confirm, confirmed) = tokio::sync::mpsc::unbounded_channel();
        let paths = paths.clone();
        let worker = std::thread::Builder::new()
            .name("spice-live-retune".to_owned())
            .spawn(move || {
                for change in asks {
                    if confirm.is_closed() {
                        return;
                    }
                    if change.confirm(&paths) && confirm.send(change).is_err() {
                        return;
                    }
                }
            })
            .map_err(|error| {
                super::super::release::environment_fault(
                    "lane::live-change-start-failed",
                    &error.to_string(),
                )
            })?;
        Ok(Self {
            owed: Some(owed),
            confirmed,
            worker: Some(worker),
            groups,
        })
    }

    /// Queue the changes working-group fast mode owes since the last turn.
    pub(in crate::resident) fn settle_groups(
        &mut self,
        bus: &Bus,
        epoch: CanonicalUuid,
    ) -> CommandResult<()> {
        let changes = self.groups.settle(bus, epoch)?;
        self.owe(changes);
        Ok(())
    }

    /// Queue the changes one dispatched command owes.
    pub(in crate::resident) fn owe(&self, changes: Vec<LiveChange>) {
        let Some(owed) = &self.owed else {
            return;
        };
        for change in changes {
            let _ = owed.send(change);
        }
    }

    /// The next change its provider confirmed.
    pub(in crate::resident) async fn recv(&mut self) -> Option<LiveChange> {
        self.confirmed.recv().await
    }
}

impl Drop for LiveChanges {
    fn drop(&mut self) {
        drop(self.owed.take());
        self.confirmed.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Apply one confirmed live change to the running launch identity, saying
/// whether it changed anything. A fast-mode change keeps the vendor's refusal
/// when that turned it off and clears any earlier one; a model change clears
/// it, since the refusal was the old model's.
fn apply_runtime(row: &mut wire::LaneResidency, live: &LiveRetune, refusal: Option<&str>) -> bool {
    let launch = &mut row
        .session
        .as_mut()
        .expect("a live request requires a recorded session")
        .launch;
    match live {
        LiveRetune::Model { provider, model } => {
            let model = format!("{provider}/{model}");
            if launch.model == model {
                return false;
            }
            launch.model = model;
            launch.fast_refusal = None;
        }
        LiveRetune::Effort { effort } => {
            if launch.effort == *effort {
                return false;
            }
            launch.effort.clone_from(effort);
        }
        LiveRetune::Fast { enabled } => {
            if launch.fast_mode == *enabled {
                return false;
            }
            launch.fast_mode = *enabled;
            launch.fast_refusal = refusal.map(str::to_owned);
        }
    }
    true
}

/// Install `session` as this seat's running session. A refusal outlives the
/// occupant that met it: a launch of the same model without fast mode
/// inherits it, so later turns neither meet nor announce it again.
pub(in crate::resident) fn seat_session(
    row: &mut wire::LaneResidency,
    mut session: wire::LaneSession,
) {
    if let Some(prior) = row.session.take()
        && !session.launch.fast_mode
        && prior.launch.model == session.launch.model
    {
        session.launch.fast_refusal = prior.launch.fast_refusal;
    }
    row.session = Some(session);
}

fn runtime_fact(
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    live: &LiveRetune,
    refusal: Option<&str>,
) -> wire::body::LaneRuntimeRetuned {
    let mut fact = wire::body::LaneRuntimeRetuned {
        lane,
        occupant,
        model: None,
        effort: None,
        fast_mode: None,
        fast_refusal: refusal.map(str::to_owned),
    };
    match live {
        LiveRetune::Model { provider, model } => fact.model = Some(format!("{provider}/{model}")),
        LiveRetune::Effort { effort } => fact.effort = Some(effort.clone()),
        LiveRetune::Fast { enabled } => fact.fast_mode = Some(*enabled),
    }
    fact
}

/// Commit one seated lane's model selection and owe an exact live OMP model
/// when the spelling carries both provider and model identity.
pub(in crate::resident) fn retune(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    command: &wire::AdmittedCommand,
    request: &LaneRetune,
    effects: &mut CommandEffects,
) -> Outcome<CommandAnswer> {
    change(
        bus,
        paths,
        epoch,
        seats,
        command,
        Selection::Model(request),
        effects,
    )
}

/// Commit one seated lane's effort selection and owe a named live OMP level.
pub(in crate::resident) fn retune_effort(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    command: &wire::AdmittedCommand,
    request: &LaneEffortRetune,
    effects: &mut CommandEffects,
) -> Outcome<CommandAnswer> {
    change(
        bus,
        paths,
        epoch,
        seats,
        command,
        Selection::Effort(request),
        effects,
    )
}

/// Commit one seated lane's fast-mode selection and owe the disposition it
/// resolves to on a live OMP occupant.
pub(in crate::resident) fn retune_fast(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    command: &wire::AdmittedCommand,
    request: &LaneFastRetune,
    effects: &mut CommandEffects,
) -> Outcome<CommandAnswer> {
    change(
        bus,
        paths,
        epoch,
        seats,
        command,
        Selection::Fast(request),
        effects,
    )
}

fn change(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    command: &wire::AdmittedCommand,
    selection: Selection<'_>,
    effects: &mut CommandEffects,
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
    let levels = if matches!(selection, Selection::Fast(_)) {
        match FastLevels::read(bus) {
            Ok(levels) => Some(levels),
            Err(outcome) => return typed_outcome(outcome),
        }
    } else {
        None
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
    let live = selection.live_request(row).zip(row.occupant);
    let flips = levels
        .as_ref()
        .map(|levels| fast_flips(observed.body(), &next, levels, levels.global()))
        .unwrap_or_default();
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
    if let Some((live, occupant)) = live {
        effects.owe_live(LiveChange::new(command, selection.lane(), occupant, live));
    }
    owe_fast_flips(effects, command, flips);
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
    actor: &wire::Actor,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    live: &LiveRetune,
    refusal: Option<&str>,
) {
    for _ in 0..RUNTIME_RECORD_ATTEMPTS {
        let Ok(observed) = current_residency(bus, epoch) else {
            return;
        };
        let mut next = observed.body().clone();
        let Some(row) = next.lanes.iter_mut().find(|row| row.lane == lane) else {
            return;
        };
        let launched = row.session.is_some();
        if row.occupant != Some(occupant) || !launched {
            return;
        }
        if !apply_runtime(row, live, refusal) {
            return;
        }
        let Ok(mut write) = write(actor.clone(), epoch, Producer::ResidentRecognition) else {
            return;
        };
        guard_residency(&mut write, &observed);
        let Ok(fact) = write.fact(
            runtime_fact(lane, occupant, live, refusal),
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

#[cfg(test)]
mod tests;
