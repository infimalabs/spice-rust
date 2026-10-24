use super::*;

/// One decided claim and the board snapshot it was decided against.
///
/// The allocator's re-read guard exists precisely because deciding a claim and
/// writing it down are separated in time. Naming that separation is what makes
/// the hazard expressible: everything `commit_claim` writes, the guard's
/// observation included, is derived from the one snapshot `claim_plan` read,
/// never from a second read taken later. This is the shape `DocPlan` already
/// runs, where the plan carries the snapshot its decision was made against and
/// the later steps consume it.
pub(in crate::board_command) struct ClaimPlan {
    snapshot: ValidatedSnapshot,
    pub(in crate::board_command) selected: Entry,
    grant: ClaimGrant,
    next: Option<Box<str>>,
    room_context: Option<Box<str>>,
    possession: Possession,
}

pub(in crate::board_command) fn claim_win(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    win: &ClaimWin,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let plan = match claim_plan(bus, command, win, repository, projects, allocation) {
        Ok(plan) => plan,
        Err(outcome) => return outcome,
    };
    commit_claim(bus, command, win, projects, plan)
}

/// Decide one claim against a single board read.
///
/// Every refusal a caller could have avoided is raised here, in order, and the
/// caller sees the first one only.
pub(in crate::board_command) fn claim_plan(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    win: &ClaimWin,
    repository: Option<&Repository>,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Result<ClaimPlan, Outcome<CommandAnswer>> {
    let possession = possession_rule(win)?;
    // A named claim decides against its exact witnessed task even when that
    // task is settled, so its refusal names ineligibility rather than absence.
    let roots = command
        .expected()
        .iter()
        .filter_map(|witness| match witness {
            Witness::Task { task, .. } => Some(task.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let snapshot = validated_working_snapshot_for_tasks(
        bus,
        command.actor().clone(),
        &roots,
        projects,
        allocation,
    )?;
    let selected = claim_selection(&snapshot, command, possession)?.clone();
    // Named capture may possess a planning deliverable, but a recovery-bound
    // capture adopts implementation, so it refuses planning before Git work.
    if matches!(possession, Possession::Recover | Possession::RecoverSteal) {
        planning::admit_capture(&selected)?;
    }
    refuse_if_actor_already_claims(&snapshot.entries, command.actor())?;
    let room_context = claim_room(bus, repository, &selected)?;
    let grant = claim_grant(bus, command, repository, &snapshot, &selected, possession)?;
    let next = Some(claim_activation_next_action(&selected));
    Ok(ClaimPlan {
        snapshot,
        selected,
        grant,
        next,
        room_context,
        possession,
    })
}

/// `selected`'s room conversation for its claim answer, read through the
/// repository's tape and artifacts; a repository-less dispatch owns neither.
pub(super) fn claim_room(
    bus: &Bus,
    repository: Option<&Repository>,
    selected: &Entry,
) -> Result<Option<Box<str>>, Outcome<CommandAnswer>> {
    repository
        .map(|repository| claim_room_context(bus, repository, selected.handle(), selected.born()))
        .transpose()
        .map(Option::flatten)
}

fn refuse_if_actor_already_claims(
    entries: &[Entry],
    actor: &Actor,
) -> Result<(), Outcome<CommandAnswer>> {
    let mut active = entries.iter().filter(|entry| {
        entry.claim().is_some_and(|claim| {
            claim.actor == *actor && claim.disposition == ClaimDisposition::Active
        })
    });
    let Some(first) = active.next() else {
        return Ok(());
    };
    if active.next().is_some() {
        return Err(protocol_fault(
            "claim::active-ambiguous",
            "one actor holds more than one active claim",
        ));
    }
    Err(refusal(
        "claim::active-exists",
        Some(first.handle().to_string()),
        &format!(
            "run spice task reclaim {} before claiming another task",
            first.handle()
        ),
    ))
}

/// What a claim settles before the journal opens.
///
/// These resolve together because they refuse together, and the order is the
/// contract rather than an accident of writing: each step here can refuse, and
/// the caller sees the first refusal only, so a seat that cannot be read is
/// reported before a boundary that cannot be captured.
pub(in crate::board_command) struct ClaimGrant {
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    at: Instant,
    expiry: Instant,
    maximum: Instant,
    git: Option<GitBoundary>,
    source: body::SourceBoundary,
    prepared_git: Option<spice::task_git::ClaimPreparation>,
}

/// Resolve the seat, the deadlines and the two boundaries one claim records.
pub(in crate::board_command) fn claim_grant(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    repository: Option<&Repository>,
    snapshot: &ValidatedSnapshot,
    selected: &Entry,
    possession: Possession,
) -> Result<ClaimGrant, Outcome<CommandAnswer>> {
    let (lane, occupant) = claim_residency(command)?;
    let (at, expiry, maximum) = claim_deadlines()?;
    let git = claim_git_boundary(repository, snapshot, command, selected, possession)?;
    let source = claim_source_boundary(bus, &lane)?;
    Ok(ClaimGrant {
        lane,
        occupant,
        at,
        expiry,
        maximum,
        git: git.as_ref().map(|git| git.boundary.clone()),
        prepared_git: git.and_then(|git| git.prepared),
        source,
    })
}
/// Claim state prepared for a task made ready in the same transaction.
pub(super) struct PreparedClaim {
    grant: ClaimGrant,
    prepared_git: Option<spice::task_git::ClaimPreparation>,
    recovery: Option<String>,
}

/// Validate and prepare an atomic mutation-and-claim before the journal opens.
pub(super) fn prepare_capture_claim(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    repository: Option<&Repository>,
    snapshot: &ValidatedSnapshot,
    selected: &Entry,
) -> Result<PreparedClaim, Outcome<CommandAnswer>> {
    let claim_rule = command.expected().iter().find_map(|witness| match witness {
        Witness::Eligibility { rule }
            if matches!(rule.as_str(), "capture" | "recover" | CAPTURE_WORK_RULE) =>
        {
            Some(rule.as_str())
        }
        _ => None,
    });
    let Some(claim_rule) = claim_rule else {
        return Err(protocol_fault(
            "claim::capture-eligibility-witness",
            "atomic capture has no capture or recovery eligibility witness",
        ));
    };
    let captures_work = claim_rule == CAPTURE_WORK_RULE;
    // Every caller (add, wake, filing) shares this door, so adopting loose
    // commits or retained recovery into a planning deliverable refuses here
    // before any claim, binding, or fact.
    if captures_work || claim_rule == "recover" {
        planning::admit_capture(selected)?;
    }
    refuse_if_actor_already_claims(&snapshot.entries, command.actor())?;
    refuse_passive_design(selected)?;
    let satisfied = board::resolved_tasks(&snapshot.entries);
    refuse_held_design(selected, &satisfied)?;
    if !board::eligible_entry(
        &satisfied,
        selected,
        command.actor(),
        &snapshot.answer.environment(),
    ) {
        return Err(refusal(
            "claim::capture-ineligible",
            Some(selected.handle().to_string()),
            "atomically claim only a public, ready task whose environment requirements and dependencies are satisfied",
        ));
    }
    // Ordinary add-and-claim records the clean allocator boundary. Compound
    // capture and dirty adoption both retain loose commits; dirty adoption also
    // binds the synchronized quarantine. Every mode commits with task creation
    // under one journal guard.
    let possession = if captures_work || claim_rule == "recover" {
        Possession::Capture
    } else {
        Possession::Next
    };
    let mut grant = claim_grant(bus, command, repository, snapshot, selected, possession)?;
    let recovery = if claim_rule == "recover" {
        let resident = repository.ok_or_else(|| {
            protocol_fault(
                "git::recovery-resident-missing",
                "atomic dirty-handoff recovery requires the resident repository",
            )
        })?;
        let lane = task_repository(snapshot, command, resident)?;
        let recovery = spice::task_git::pending_recovery(&lane)
            .map_err(task_git_outcome)?
            .ok_or_else(|| {
                refusal(
                    "git::recovery-transaction-missing",
                    None,
                    "start atomic adoption through `spice task capture --title TITLE --project PROJECT`",
                )
            })?;
        let boundary = grant.git.clone().ok_or_else(|| {
            protocol_fault(
                "git::recovery-boundary-missing",
                "atomic dirty-handoff recovery prepared no Git boundary",
            )
        })?;
        spice::task_git::bind_recovery_claim(
            &lane,
            &recovery,
            spice::task_git::RecoveryClaim {
                actor: command.actor().clone(),
                task: selected.born().clone(),
                handle: Some(selected.handle().clone()),
                fresh: true,
                statement: wire::Seq::new(1).expect("fresh recovery placeholder is non-zero"),
                workflow: wire::Seq::new(1).expect("fresh recovery placeholder is non-zero"),
                phase: selected.workflow().phase.clone(),
                claim: None,
                lane: grant.lane,
                occupant: grant.occupant,
                epoch: command.epoch(),
                boundary,
            },
        )
        .map_err(task_git_outcome)?;
        Some(recovery.transaction().to_owned())
    } else {
        None
    };
    let prepared_git = grant.prepared_git.take();
    Ok(PreparedClaim {
        grant,
        prepared_git,
        recovery,
    })
}

impl PreparedClaim {
    pub(super) fn at(&self) -> Instant {
        self.grant.at
    }

    pub(super) fn guard(
        &self,
        write: &mut JournalWrite,
        snapshot: &ValidatedSnapshot,
        command: &wire::AdmittedCommand,
    ) {
        guard_capture(
            write,
            command.expected().to_vec(),
            snapshot.cursor,
            command.actor().clone(),
        );
    }

    pub(super) fn answer(
        &self,
        command: &wire::AdmittedCommand,
        snapshot: &ValidatedSnapshot,
        selected: &Entry,
        origin_context: Option<Box<str>>,
        room_context: Option<Box<str>>,
    ) -> wire::intent::ClaimAnswer {
        claim_answer(
            command,
            selected,
            &snapshot.entries,
            ClaimReading {
                next: Some(claim_activation_next_action(selected)),
                origin_context,
                room_context,
            },
            self.prepared_git
                .as_ref()
                .and_then(spice::task_git::ClaimPreparation::transplant),
            false,
        )
    }

    pub(super) fn append(
        self,
        write: &mut JournalWrite,
        command: &wire::AdmittedCommand,
        selected: &Entry,
    ) -> Result<Option<spice::task_git::ClaimPreparation>, Outcome<CommandAnswer>> {
        let Self {
            grant,
            prepared_git,
            recovery,
        } = self;
        if let Err(outcome) = mint_claim(
            write,
            board::allocate::Mint {
                task: selected.born(),
                actor: command.actor(),
                workflow: selected.workflow(),
                project: selected.project().as_str(),
                lane: grant.lane,
                occupant: grant.occupant,
                epoch: command.epoch(),
                at: grant.at,
                expiry: grant.expiry,
                maximum: grant.maximum,
                git: grant.git,
                recovery,
                source: grant.source,
                cause: None,
            },
        ) {
            if let Some(prepared) = prepared_git
                && let Err(error) = prepared.rollback()
            {
                return Err(task_git_outcome(error));
            }
            return Err(outcome);
        }
        Ok(prepared_git)
    }
}

/// The texts a claimant reads with its answer: the command it runs next and
/// the steering and room context each derived on its own.
struct ClaimReading {
    next: Option<Box<str>>,
    origin_context: Option<Box<str>>,
    room_context: Option<Box<str>>,
}

fn claim_answer(
    command: &wire::AdmittedCommand,
    selected: &Entry,
    entries: &[Entry],
    reading: ClaimReading,
    transplant: Option<&wire::intent::ClaimTransplant>,
    nothing_stolen: bool,
) -> wire::intent::ClaimAnswer {
    let ClaimReading {
        next,
        origin_context,
        room_context,
    } = reading;
    wire::intent::ClaimAnswer {
        task: selected.handle().clone(),
        statement: Some(Box::new(wire::intent::ClaimStatement {
            title: selected.statement().title.clone(),
            pitch: selected.statement().pitch.clone(),
            description: selected.statement().description.clone(),
            acceptance: selected.statement().acceptance.clone(),
        })),
        holder: Some(command.actor().clone()),
        expiry: None,
        warnings: board::claim_overlaps(selected, entries)
            .into_iter()
            .map(|overlap| wire::intent::ClaimWarning {
                task: overlap.task,
                lane: overlap.lane,
                surface: overlap.surface,
            })
            .collect::<Vec<_>>()
            .into_boxed_slice(),
        nothing_stolen,
        creation_context: Some(task_creation_context_output(&selected.statement().creation)),
        origin_context,
        room_context,
        next,
        phase: Some(claim_phase(selected, command.actor())),
        transplant: transplant.cloned(),
    }
}

/// Write the decided claim: one origin, the guards it owes, its two heads, its
/// receipt.
///
/// Nothing here refuses on what the caller asked for; by this point the command
/// is already admitted and the plan already decided, so every failure below is
/// a fault rather than a refusal the caller could have avoided: a validated
/// birth stamp that will not form an origin is a protocol fault, and the
/// mint and the receipt are store faults. The public handle is answer text only.
///
/// The plan arrives whole rather than as loose arguments so the guard's
/// observation can only come from the snapshot the decision was made against.
pub(in crate::board_command) fn commit_claim(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    win: &ClaimWin,
    projects: &ProjectPolicy,
    plan: ClaimPlan,
) -> Outcome<CommandAnswer> {
    let ClaimPlan {
        snapshot,
        selected,
        grant,
        next,
        room_context,
        possession,
    } = plan;
    let ClaimGrant {
        lane,
        occupant,
        at,
        expiry,
        maximum,
        git,
        source,
        prepared_git,
    } = grant;
    let task = selected.handle().clone();
    let born = selected.born().clone();
    let origin = match task_origin(&born) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    guard_claim(&mut write, &snapshot, command, possession, &task, projects);
    if let Err(outcome) = mint_claim(
        &mut write,
        board::allocate::Mint {
            task: &born,
            actor: command.actor(),
            workflow: selected.workflow(),
            project: selected.project().as_str(),
            lane,
            occupant,
            epoch: command.epoch(),
            at,
            expiry,
            maximum,
            git,
            recovery: win.recovery.clone(),
            source,
            cause: None,
        },
    ) {
        if let Some(prepared) = prepared_git
            && let Err(error) = prepared.rollback()
        {
            return task_git_outcome(error);
        }
        return outcome;
    }
    let origin_context = match task_steering_origin_output(bus, &snapshot.facts, &selected) {
        Ok(context) => context,
        Err(outcome) => {
            if let Some(prepared) = prepared_git
                && let Err(error) = prepared.rollback()
            {
                return task_git_outcome(error);
            }
            return outcome;
        }
    };
    let answer = claim_answer(
        command,
        &selected,
        &snapshot.entries,
        ClaimReading {
            next,
            origin_context,
            room_context,
        },
        prepared_git
            .as_ref()
            .and_then(spice::task_git::ClaimPreparation::transplant),
        matches!(possession, Possession::Steal | Possession::RecoverSteal)
            && selected
                .claim()
                .is_none_or(|claim| claim.disposition != ClaimDisposition::Active),
    );
    if let Err(error) = write.command_result(
        request(command, win.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        if let Some(prepared) = prepared_git
            && let Err(rollback) = prepared.rollback()
        {
            return task_git_outcome(rollback);
        }
        return store_fault("claim::win-receipt", &error.to_string());
    }
    let body = CommandAnswer::Claim(Box::new(answer));
    if let Some(prepared) = prepared_git {
        committed_with_task_git(bus, write, body, PreparedTaskGit::Claim(prepared))
    } else {
        committed(bus, write, body)
    }
}

/// The exact command a lane runs next on the task it just won.
///
/// A task held for taste review is held on its statement, so the admitted
/// next action is the rewording rather than another read.
pub(in crate::board_command) fn claim_activation_next_action(selected: &Entry) -> Box<str> {
    if selected.statement().taste.is_some() {
        return format!("spice task modify {} --title \"...\"", selected.handle()).into_boxed_str();
    }
    format!("spice task show {} --full", selected.handle()).into_boxed_str()
}

/// The phase a lane just won, said where the lane reads the assignment.
///
/// The handle and the next command say nothing about what kind of work this
/// is, and the phase decides whether the deliverable is board state or
/// repository files. A lane that has to read the task again to learn that has
/// already started the wrong work. `selected` is the row as it stood before
/// this win, so the lane's own review and a row it freed still read as such,
/// and a freed row carries the reminder of how to say what it waits on.
fn claim_phase(selected: &Entry, actor: &wire::Actor) -> wire::intent::ClaimPhase {
    let workflow = selected.workflow();
    let deliverable = if board::is_independent_handoff(selected, actor) {
        Some(board::phase::SELF_HANDOFF_DELIVERABLE)
    } else {
        board::phase::deliverable(&workflow.phase)
    };
    let deliverable = match (deliverable, board::is_released_by(selected, actor)) {
        (deliverable, false) => deliverable.map(Box::from),
        (Some(deliverable), true) => Some(
            format!(
                "{deliverable}; {}",
                board::phase::released_reminder(selected)
            )
            .into_boxed_str(),
        ),
        (None, true) => Some(board::phase::released_reminder(selected).into_boxed_str()),
    };
    wire::intent::ClaimPhase {
        name: workflow.phase.clone(),
        deliverable,
    }
}

/// Which possession law one claim win asks the board to apply.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::board_command) enum Possession {
    /// The allocator chooses among tasks nobody holds.
    Next,
    /// The caller names a task nobody holds.
    Capture,
    /// The caller names a task and takes it from its current holder.
    Steal,
    /// The caller names one unclaimed task for a sealed dirty adoption.
    Recover,
    /// The caller replaces a current holder with a sealed dirty adoption.
    RecoverSteal,
}
impl Possession {
    /// Whether the caller names the exact task rather than running allocation.
    const fn named(self) -> bool {
        !matches!(self, Self::Next)
    }
}

fn possession_rule(win: &ClaimWin) -> Result<Possession, Outcome<CommandAnswer>> {
    match (win.rule.as_str(), win.recovery.as_deref()) {
        ("next", None) => Ok(Possession::Next),
        ("capture", None) => Ok(Possession::Capture),
        ("steal", None) => Ok(Possession::Steal),
        (rule @ ("recover" | RECOVER_STEAL_RULE), Some(transaction))
            if spice::task_git::Recovery::is_transaction(transaction) =>
        {
            Ok(if rule == RECOVER_STEAL_RULE {
                Possession::RecoverSteal
            } else {
                Possession::Recover
            })
        }
        ("recover" | RECOVER_STEAL_RULE, None) => Err(refusal(
            "git::recovery-transaction-missing",
            None,
            "start sealed adoption through `spice task capture HANDLE`",
        )),
        ("recover" | RECOVER_STEAL_RULE, Some(transaction)) => Err(refusal(
            "git::recovery-transaction-invalid",
            Some(transaction.to_owned()),
            "use the content-addressed transaction sealed by task capture",
        )),
        ("next" | "capture" | "steal", Some(transaction)) => Err(refusal(
            "git::recovery-rule-mismatch",
            Some(transaction.to_owned()),
            "use recovery identity only with a sealed capture possession rule",
        )),
        _ => Err(refusal(
            "board::allocator-rule",
            Some(win.rule.to_string()),
            "use the next, capture, steal, or sealed-capture possession rule",
        )),
    }
}

fn claim_selection<'a>(
    snapshot: &'a ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    possession: Possession,
) -> Result<&'a Entry, Outcome<CommandAnswer>> {
    if possession.named() {
        named_claim_selection(snapshot, command, possession)
    } else {
        allocator_claim_selection(snapshot, command)
    }
}

fn allocator_claim_selection<'a>(
    snapshot: &'a ValidatedSnapshot,
    command: &wire::AdmittedCommand,
) -> Result<&'a Entry, Outcome<CommandAnswer>> {
    let subscription = allocation_subscription(&snapshot.answer.residency.body, command.actor())?;
    let environment = snapshot.answer.environment();
    let now = current_instant()?;
    let selected = board::select_at(
        &snapshot.entries,
        command.actor(),
        &subscription,
        snapshot.answer.allocation_band,
        snapshot.answer.recent_cell.as_ref(),
        &environment,
        now,
    )
    .map(|selected| selected.entry())
    .ok_or_else(|| {
        refusal(
            "board::allocator-empty",
            None,
            concat!("add or wake eligible work before ", "requesting task next",),
        )
    })?;
    let witnessed = command
        .expected()
        .iter()
        .find_map(|witness| match witness {
            Witness::Task { task, .. } => Some(task),
            _ => None,
        })
        .ok_or_else(|| {
            protocol_fault(
                "board::allocator-task-witness",
                "task next has no selected task witness",
            )
        })?;
    if witnessed != selected.born() {
        return Err(refusal(
            "board::witness-moved",
            Some(format!(
                "witnessed {witnessed}; selected {}",
                selected.handle()
            )),
            "read the current board subscription and retry task next",
        ));
    }
    Ok(selected)
}

fn named_claim_selection<'a>(
    snapshot: &'a ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    possession: Possession,
) -> Result<&'a Entry, Outcome<CommandAnswer>> {
    let task = command
        .expected()
        .iter()
        .find_map(|witness| match witness {
            Witness::Task { task, .. } => Some(task),
            _ => None,
        })
        .ok_or_else(|| {
            protocol_fault(
                "claim::capture-task-witness",
                "capture possession has no exact task witness",
            )
        })?;
    let entry = snapshot
        .entries
        .iter()
        .find(|entry| entry.born() == task)
        .ok_or_else(|| {
            refusal(
                "task::unknown",
                Some(task.to_string()),
                "capture into one current pending task",
            )
        })?;
    refuse_passive_design(entry)?;
    if entry.is_draft() {
        return Err(refusal(
            "task::draft-unfiled",
            Some(filing_state(entry)),
            &format!(
                "file and claim the Draft with spice task file {} --claim",
                entry.handle(),
            ),
        ));
    }
    let environment = snapshot.answer.environment();
    let satisfied = board::resolved_tasks(&snapshot.entries);
    let (admitted, reason, remedy) = match possession {
        Possession::Steal | Possession::RecoverSteal => (
            board::stealable_entry(&satisfied, entry, command.actor(), &environment),
            "claim::steal-ineligible",
            "capture --steal only a ready, pending task, held or not",
        ),
        Possession::Recover | Possession::Capture => {
            refuse_held_design(entry, &satisfied)?;
            (
                board::eligible_entry(&satisfied, entry, command.actor(), &environment),
                "claim::capture-ineligible",
                "capture only an unclaimed, ready, pending task",
            )
        }
        Possession::Next => unreachable!("allocator possession returned above"),
    };
    if admitted {
        Ok(entry)
    } else {
        Err(refusal(reason, Some(task.to_string()), remedy))
    }
}

/// A Design outside every working group is passive: no claim path grants it.
pub(in crate::board_command) fn refuse_passive_design(
    entry: &Entry,
) -> Result<(), Outcome<CommandAnswer>> {
    if board::passive_design(entry) {
        return Err(refusal(
            "claim::design-passive",
            Some(entry.handle().to_string()),
            "a Design outside every working group is passive; move it into a working group to make it claimable",
        ));
    }
    Ok(())
}

/// An active Design whose pass cycle is not ready for a proposal is steeping:
/// capture and recovery name the cycle and its next action, and the operator's
/// nudge remains the escape hatch.
pub(in crate::board_command) fn refuse_held_design(
    entry: &Entry,
    satisfied: &board::ResolvedTasks,
) -> Result<(), Outcome<CommandAnswer>> {
    if let Some((cycle, next_action)) = board::design_hold(entry, satisfied) {
        return Err(refusal(
            "claim::design-held",
            Some(entry.handle().to_string()),
            &format!("{} is {}; {next_action}", entry.handle(), cycle.label()),
        ));
    }
    Ok(())
}

fn claim_residency(
    command: &wire::AdmittedCommand,
) -> Result<(wire::CanonicalUuid, wire::CanonicalUuid), Outcome<CommandAnswer>> {
    let seat = command
        .expected()
        .iter()
        .find_map(|witness| match witness {
            Witness::Residency {
                lane,
                occupant: Some(occupant),
                epoch,
            } if *epoch == command.epoch() => Some((*lane, *occupant)),
            _ => None,
        })
        .ok_or_else(|| {
            protocol_fault(
                "claim::residency-witness",
                "claim win has no current residency witness",
            )
        })?;
    if let Actor::Lane(actor) = command.actor()
        && *actor != seat.0
    {
        return Err(refusal(
            "claim::lane-mismatch",
            Some(seat.0.to_string()),
            concat!(
                "claim through the lane bound to the ",
                "authenticated capability",
            ),
        ));
    }
    Ok(seat)
}

fn claim_deadlines() -> Result<(Instant, Instant, Instant), Outcome<CommandAnswer>> {
    let at = current_instant()?;
    let (expiry, maximum) = claim_lease(at)?;
    Ok((at, expiry, maximum))
}

/// The ordinary claim lease granted at `at`: its first expiry and the maximum
/// instant renewal may reach.
pub(in crate::board_command) fn claim_lease(
    at: Instant,
) -> Result<(Instant, Instant), Outcome<CommandAnswer>> {
    Ok((instant_after(at, 60 * 60)?, instant_after(at, 2 * 60 * 60)?))
}

pub(in crate::board_command) struct ClaimGit {
    boundary: GitBoundary,
    prepared: Option<spice::task_git::ClaimPreparation>,
}

/// Commits and dirty work the task's latest release parked for its next claim.
pub(in crate::board_command) fn task_parked_commits<'a>(
    snapshot: &'a ValidatedSnapshot,
    task: &TaskStamp,
) -> Option<&'a wire::body::TaskParkedCommits> {
    snapshot
        .answer
        .tasks
        .iter()
        .find(|row| row.statement.body.born == *task)
        .and_then(|row| row.parked_commits.as_ref())
}

pub(in crate::board_command) fn claim_git_boundary(
    repository: Option<&Repository>,
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    selected: &Entry,
    possession: Possession,
) -> Result<Option<ClaimGit>, Outcome<CommandAnswer>> {
    let Some(resident) = repository else {
        return Ok(None);
    };
    let lane = task_repository(snapshot, command, resident)?;
    let parked = task_parked_commits(snapshot, selected.born());
    let result = match possession {
        // Capture adopts loose commits only into implementation; a planning
        // deliverable takes the allocator's clean boundary instead.
        Possession::Capture if parked.is_none() && !planning::planning_deliverable(selected) => {
            spice::task_git::prepare_explicit_claim(&lane).map(|prepared| ClaimGit {
                boundary: prepared.boundary.clone(),
                prepared: Some(prepared),
            })
        }
        Possession::Capture if parked.is_none() => {
            match spice::task_git::prepare_task_claim(&lane, None) {
                Err(error) if error.reason() == "git::claim-loose-commits" => {
                    planning::admit_capture(selected)?;
                    return Err(task_git_outcome(error));
                }
                result => result.map(|prepared| ClaimGit {
                    boundary: prepared.boundary.clone(),
                    prepared: Some(prepared),
                }),
            }
        }
        Possession::Steal => {
            spice::task_git::prepare_explicit_claim(&lane).map(|prepared| ClaimGit {
                boundary: prepared.boundary.clone(),
                prepared: Some(prepared),
            })
        }
        Possession::Next | Possession::Capture => {
            spice::task_git::prepare_task_claim(&lane, parked).map(|prepared| ClaimGit {
                boundary: prepared.boundary.clone(),
                prepared: Some(prepared),
            })
        }
        Possession::Recover | Possession::RecoverSteal => {
            return recovery_git_boundary(&lane, command, selected).map(|boundary| {
                Some(ClaimGit {
                    boundary,
                    prepared: None,
                })
            });
        }
    };
    result.map(Some).map_err(task_git_outcome)
}

fn recovery_git_boundary(
    repository: &Repository,
    command: &wire::AdmittedCommand,
    selected: &Entry,
) -> Result<GitBoundary, Outcome<CommandAnswer>> {
    let recovery = spice::task_git::pending_recovery(repository)
        .map_err(task_git_outcome)?
        .ok_or_else(|| {
            refusal(
                "git::recovery-transaction-missing",
                None,
                "start sealed adoption through `spice task capture HANDLE`",
            )
        })?;
    let transaction = match command.body() {
        wire::intent::Intent::ClaimWin(win) => win.recovery.as_deref(),
        _ => None,
    }
    .expect("recover possession admission requires a transaction");
    if recovery.transaction() != transaction {
        return Err(refusal(
            "git::recovery-transaction-mismatch",
            Some(format!(
                "retained recovery is {}, but the claim names {transaction}",
                recovery.transaction()
            )),
            "resume only the retained recovery transaction",
        ));
    }
    let sealed = spice::task_git::recovery_claim(repository, &recovery)
        .map_err(task_git_outcome)?
        .ok_or_else(|| {
            refusal(
                "git::recovery-claim-unbound",
                Some(format!("recovery {transaction} has no sealed board claim")),
                "restore with `spice task capture --restore`, then restart capture",
            )
        })?;
    let workflow = command.expected().iter().find_map(|witness| match witness {
        Witness::Workflow { task, head, phase } if task == selected.born() => {
            Some((*head, phase.clone()))
        }
        _ => None,
    });
    let statement = command.expected().iter().find_map(|witness| match witness {
        Witness::Task { task, head } if task == selected.born() => Some(*head),
        _ => None,
    });
    let claim = command.expected().iter().find_map(|witness| match witness {
        Witness::Claim { task, head } if task == selected.born() => Some(*head),
        _ => None,
    });
    let residency = command.expected().iter().find_map(|witness| match witness {
        Witness::Residency {
            lane,
            occupant: Some(occupant),
            epoch,
        } => Some((*lane, *occupant, *epoch)),
        _ => None,
    });
    let Some((workflow, phase)) = workflow else {
        return Err(refusal(
            "git::recovery-workflow-witness",
            None,
            "read the current task and restart capture",
        ));
    };
    let Some(statement) = statement else {
        return Err(refusal(
            "git::recovery-task-witness",
            None,
            "read the current task and restart capture",
        ));
    };
    let Some((lane, occupant, epoch)) = residency else {
        return Err(refusal(
            "git::recovery-residency-witness",
            None,
            "retry through the currently seated lane",
        ));
    };
    if sealed.actor != *command.actor()
        || sealed.task != *selected.born()
        || sealed.statement != statement
        || sealed.workflow != workflow
        || sealed.phase != phase
        || sealed.claim != claim.flatten()
        || sealed.lane != lane
        || sealed.occupant != occupant
        || sealed.epoch != epoch
    {
        return Err(refusal(
            "git::recovery-claim-moved",
            Some(format!(
                "sealed claim intent no longer matches task {}",
                selected.handle()
            )),
            "retain the quarantine and inspect the moved actor/task/workflow/claim/residency state",
        ));
    }
    let boundary =
        spice::task_git::synchronize_recovery(repository, &recovery).map_err(task_git_outcome)?;
    if boundary != sealed.boundary {
        return Err(refusal(
            "git::recovery-claim-boundary-mismatch",
            Some(format!(
                "sealed boundary {:?}; synchronized boundary {boundary:?}",
                sealed.boundary
            )),
            "retain the quarantine and inspect the moved Git boundary",
        ));
    }
    Ok(boundary)
}

/// Read the lane's accounted transcript boundary for the claim heads.
///
/// The read goes through the writer queue, so it can fail on a closed or
/// faulted bus rather than merely returning nothing; that failure is a store
/// fault instead of a refusal because no command the caller could have sent
/// would have avoided it.
fn claim_source_boundary(
    bus: &Bus,
    lane: &wire::CanonicalUuid,
) -> Result<body::SourceBoundary, Outcome<CommandAnswer>> {
    bus.source_boundary(lane)
        .map_err(|error| store_fault("claim::boundary-unreadable", &error.to_string()))
}

fn guard_claim(
    write: &mut JournalWrite,
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    possession: Possession,
    _task: &TaskHandle,
    _projects: &ProjectPolicy,
) {
    let expected = command.expected().to_vec();
    if possession.named() {
        guard_capture(write, expected, snapshot.cursor, command.actor().clone());
    } else {
        guard_allocation(
            write,
            expected,
            AllocationObservation {
                cursor: snapshot.cursor,
                actor: command.actor().clone(),
                residency_position: snapshot.answer.residency.position,
            },
        );
    }
}

/// Mint one claim and its two heads, naming the head whose build refused.
pub(in crate::board_command) fn mint_claim(
    write: &mut JournalWrite,
    mint: board::allocate::Mint<'_>,
) -> Result<PendingFact, Outcome<CommandAnswer>> {
    board::allocate::mint(write, mint).map_err(|error| match error {
        board::allocate::MintError::Assignment(error) => {
            store_fault("review::assignment-build", &error.to_string())
        }
        board::allocate::MintError::Claim(error) => {
            store_fault("claim::win-build", &error.to_string())
        }
    })
}
