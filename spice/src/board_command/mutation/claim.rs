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
    let snapshot = validated_snapshot(bus, command.actor().clone(), projects, allocation)?;
    let selected = claim_selection(&snapshot, command, possession)?.clone();
    refuse_if_actor_already_claims(&snapshot.entries, command.actor())?;
    let grant = claim_grant(bus, command, repository, &snapshot, possession)?;
    let next = projected_claim_next_action(
        &snapshot,
        &selected,
        command,
        grant.lane,
        grant.occupant,
        grant.at,
        grant.expiry,
        grant.maximum,
        projects,
    );
    Ok(ClaimPlan {
        snapshot,
        selected,
        grant,
        next,
        possession,
    })
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
        "finish, review, unclaim, or reclaim the active task before claiming another",
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
}

/// Resolve the seat, the deadlines and the two boundaries one claim records.
pub(in crate::board_command) fn claim_grant(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    repository: Option<&Repository>,
    snapshot: &ValidatedSnapshot,
    possession: Possession,
) -> Result<ClaimGrant, Outcome<CommandAnswer>> {
    let (lane, occupant) = claim_residency(command)?;
    let (at, expiry, maximum) = claim_deadlines()?;
    let git = claim_git_boundary(repository, snapshot, command, possession.captures())?;
    let source = claim_source_boundary(bus, &lane)?;
    Ok(ClaimGrant {
        lane,
        occupant,
        at,
        expiry,
        maximum,
        git,
        source,
    })
}

/// Write the decided claim: one origin, the guards it owes, its two heads, its
/// receipt.
///
/// Nothing here refuses on what the caller asked for; by this point the command
/// is already admitted and the plan already decided, so every failure below is
/// a fault rather than a refusal the caller could have avoided: a validated
/// inception stamp that will not form an origin is a protocol fault, and the
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
        possession,
    } = plan;
    let task = selected.handle().clone();
    let born = selected.born().clone();
    let origin = match task_origin(&born) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(
        command.actor().clone(),
        origin,
        grant.at,
        Producer::CommandDoor,
    );
    guard_claim(
        &mut write,
        &snapshot,
        command,
        possession.captures(),
        &task,
        projects,
    );
    if let Err(outcome) = append_claim_heads(
        &mut write,
        command,
        &selected,
        &born,
        grant.lane,
        grant.occupant,
        grant.at,
        grant.expiry,
        grant.maximum,
        grant.git,
        grant.source,
    ) {
        return outcome;
    }
    let answer = wire::intent::ClaimAnswer {
        task,
        holder: command.actor().clone(),
        warnings: board::claim_overlaps(&selected, &snapshot.entries)
            .into_iter()
            .map(|overlap| wire::intent::ClaimWarning {
                task: overlap.task,
                lane: overlap.lane,
                surface: overlap.surface,
            })
            .collect(),
        next,
    };
    if let Err(error) = write.command_result(
        request(command, win.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("claim::win-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::Claim(answer))
}

#[allow(
    clippy::too_many_arguments,
    reason = "the projected claim is the exact pair of heads the transaction appends"
)]
fn projected_claim_next_action(
    snapshot: &ValidatedSnapshot,
    selected: &Entry,
    command: &wire::AdmittedCommand,
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    at: Instant,
    expiry: Instant,
    maximum: Instant,
    projects: &ProjectPolicy,
) -> Option<Box<str>> {
    let mut workflow = selected.workflow().clone();
    if workflow.phase.as_str() == "review" {
        workflow.assignment = Some(command.actor().clone());
        workflow.review = "assigned".to_owned();
    }
    let post = Entry::new(
        selected.statement().clone(),
        workflow,
        Some(ClaimHead {
            actor: command.actor().clone(),
            fact: snapshot.cursor,
            claimed: snapshot.cursor,
            cell: wire::ClaimCell {
                project: selected.project().as_str().to_owned(),
                phase: selected.workflow().phase.clone(),
            },
            lane,
            occupant,
            epoch: command.epoch(),
            disposition: ClaimDisposition::Active,
        }),
        Some(LeaseHead {
            start: at,
            renewal: at,
            expiry,
            maximum,
            epoch: command.epoch(),
        }),
        projects,
    )
    .expect("a validated selected entry accepts the transaction's claim heads");
    next_action(snapshot, &post, false)
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
}

impl Possession {
    /// Whether the lane's local commits become this claim's boundary.
    const fn captures(self) -> bool {
        !matches!(self, Self::Next)
    }
}

fn possession_rule(win: &ClaimWin) -> Result<Possession, Outcome<CommandAnswer>> {
    match win.rule.as_str() {
        "next" => Ok(Possession::Next),
        "capture" => Ok(Possession::Capture),
        "steal" => Ok(Possession::Steal),
        _ => Err(refusal(
            "board::allocator-rule",
            Some(win.rule.to_string()),
            "use the next, capture or steal possession rule",
        )),
    }
}

fn claim_selection<'a>(
    snapshot: &'a ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    possession: Possession,
) -> Result<&'a Entry, Outcome<CommandAnswer>> {
    if !possession.captures() {
        let subscription =
            allocation_subscription(&snapshot.answer.residency.body, command.actor())?;
        let selected = board::select(
            &snapshot.entries,
            command.actor(),
            subscription,
            snapshot.answer.allocation_band,
            snapshot.answer.recent_cell.as_ref(),
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
        return Ok(selected);
    }
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
    let (admitted, reason, remedy) = match possession {
        Possession::Steal => (
            board::stealable_entry(&snapshot.entries, entry, command.actor()),
            "claim::steal-ineligible",
            "steal only a ready, pending task",
        ),
        _ => (
            board::capturable_entry(&snapshot.entries, entry, command.actor()),
            "claim::capture-ineligible",
            "capture only an unclaimed, ready, pending task",
        ),
    };
    if admitted {
        Ok(entry)
    } else {
        Err(refusal(reason, Some(task.to_string()), remedy))
    }
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
                occupant,
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
    let expiry = instant_after(at, 15 * 60)?;
    let maximum = instant_after(at, 60 * 60)?;
    Ok((at, expiry, maximum))
}

pub(in crate::board_command) fn claim_git_boundary(
    repository: Option<&Repository>,
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    capture: bool,
) -> Result<Option<GitBoundary>, Outcome<CommandAnswer>> {
    let Some(resident) = repository else {
        return Ok(None);
    };
    let lane = task_repository(snapshot, command, resident)?;
    let result = if capture {
        spice::task_git::capture(&lane).map(|capture| capture.boundary)
    } else {
        spice::task_git::prepare_claim(&lane)
    };
    result.map(Some).map_err(task_git_outcome)
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
    capture: bool,
    task: &TaskHandle,
    projects: &ProjectPolicy,
) {
    let expected = command.expected().to_vec();
    let heads = allocation_heads(&snapshot.answer);
    if capture {
        guard_capture(write, expected, heads, command.actor().clone());
    } else {
        guard_allocation(
            write,
            expected,
            AllocationObservation {
                heads,
                winner: task.clone(),
                actor: command.actor().clone(),
                subscription: allocation_subscription(
                    &snapshot.answer.residency.body,
                    command.actor(),
                )
                .expect("claim selection admitted one durable lineage subscription")
                .clone(),
                residency_position: snapshot.answer.residency.position,
                projects: projects.clone(),
                band: snapshot.answer.allocation_band,
                recent_cell: snapshot.answer.recent_cell.clone(),
            },
        );
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one claim fact and its two heads share one boundary"
)]
fn append_claim_heads(
    write: &mut JournalWrite,
    command: &wire::AdmittedCommand,
    selected: &Entry,
    task: &wire::TaskStamp,
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    at: Instant,
    expiry: Instant,
    maximum: Instant,
    git: Option<GitBoundary>,
    source: body::SourceBoundary,
) -> Result<(), Outcome<CommandAnswer>> {
    board::allocate::mint(
        write,
        board::allocate::Mint {
            task,
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
            source,
        },
    )
    .map(|_| ())
    .map_err(|error| match error {
        board::allocate::MintError::Assignment(error) => {
            store_fault("review::assignment-build", &error.to_string())
        }
        board::allocate::MintError::Claim(error) => {
            store_fault("claim::win-build", &error.to_string())
        }
    })
}
