use super::*;

pub(in crate::board_command) struct RecoveryPreflight {
    actor: Actor,
    task: TaskStamp,
    statement: Seq,
    workflow: Seq,
    phase: wire::Phase,
    claim: Option<Seq>,
    lane: wire::CanonicalUuid,
    occupant: wire::CanonicalUuid,
    epoch: wire::CanonicalUuid,
    expected: Vec<Witness>,
}

impl RecoveryPreflight {
    pub(in crate::board_command) fn seal(
        self,
        boundary: GitBoundary,
    ) -> spice::task_git::RecoveryClaim {
        spice::task_git::RecoveryClaim {
            actor: self.actor,
            task: self.task,
            statement: self.statement,
            handle: None,
            fresh: false,
            workflow: self.workflow,
            phase: self.phase,
            claim: self.claim,
            lane: self.lane,
            occupant: self.occupant,
            epoch: self.epoch,
            boundary,
        }
    }
}
pub(super) fn client_add_recovery(
    repository: &Repository,
    contract: &Contract,
    body: TaskAdd,
) -> Outcome<CommandAnswer> {
    let pending = match spice::task_git::pending_recovery(repository) {
        Ok(pending) => pending,
        Err(error) => return task_git_outcome(error),
    };
    let recovery = match pending {
        Some(recovery) => {
            return resume_pending_client_capture(repository, None, recovery);
        }
        None => match spice::task_git::quarantine_for_recovery(repository) {
            Ok(recovery) => recovery,
            Err(error) => return task_git_outcome(error),
        },
    };
    if let Err(error) = spice::task_git::synchronize_recovery(repository, &recovery) {
        return rollback_recovery_error(repository, &recovery, error);
    }
    let outcome = submit_recovery_task_add(repository, contract, body);
    let claim = match spice::task_git::recovery_claim(repository, &recovery) {
        Ok(Some(claim)) if claim.fresh => claim,
        Ok(Some(_)) => {
            return refusal(
                "git::recovery-target-mismatch",
                Some(recovery.transaction().to_owned()),
                "retain the quarantine and resume only its sealed target",
            );
        }
        Ok(None) => {
            if let Err(error) = spice::task_git::restore_recovery(repository, &recovery) {
                return task_git_outcome(error);
            }
            return outcome;
        }
        Err(error) => return task_git_outcome(error),
    };
    let Some(task) = claim.handle.clone() else {
        return protocol_fault(
            "git::recovery-fresh-handle-missing",
            "atomic recovery claim has no sealed public handle",
        );
    };
    reconcile_client_recovery(repository, &task, recovery, claim, Some(outcome), true)
}

pub(super) fn client_capture_recovery(
    repository: &Repository,
    contract: &Contract,
    task: &TaskHandle,
    stealing: bool,
) -> Outcome<CommandAnswer> {
    let pending = match spice::task_git::pending_recovery(repository) {
        Ok(pending) => pending,
        Err(error) => return task_git_outcome(error),
    };
    if let Some(recovery) = pending {
        return resume_pending_client_capture(repository, Some(task), recovery);
    }
    let board = match current_board(
        repository,
        BoardRead::Tasks {
            tasks: vec![
                task.born()
                    .expect("a parsed handle carries a canonical task stamp"),
            ],
            dependencies: true,
            claims: true,
            history: false,
        },
    ) {
        Ok(board) => board,
        Err(outcome) => return outcome,
    };
    let preflight = match capture_recovery_preflight(&board, task, stealing) {
        Ok(preflight) => preflight,
        Err(outcome) => return outcome,
    };
    let recovery = match spice::task_git::quarantine_for_recovery(repository) {
        Ok(recovery) => recovery,
        Err(error) => return task_git_outcome(error),
    };
    let boundary = match spice::task_git::synchronize_recovery(repository, &recovery) {
        Ok(boundary) => boundary,
        Err(error) => return rollback_recovery_error(repository, &recovery, error),
    };
    let expected = preflight.expected.clone();
    let claim = preflight.seal(boundary);
    if let Err(error) = spice::task_git::bind_recovery_claim(repository, &recovery, claim.clone()) {
        // A failed durable bind can mean another writer sealed a different
        // intent. Preserve that evidence rather than destroying its authority.
        return task_git_outcome(error);
    }
    let transaction = recovery.transaction().to_owned();
    let rule = if stealing { "recover-steal" } else { "recover" }
        .parse::<Name>()
        .expect("recovery possession rules are wire names");
    let outcome = crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::ClaimWin,
        expected,
        &ClaimWin {
            rule,
            recovery: Some(transaction),
        },
    );
    reconcile_client_recovery(repository, task, recovery, claim, Some(outcome), false)
}

#[cfg(test)]
pub(in crate::board_command) fn recovery_preflight(
    board: &BoardAnswer,
    task: &TaskHandle,
) -> Result<RecoveryPreflight, Outcome<CommandAnswer>> {
    capture_recovery_preflight(board, task, false)
}

fn capture_recovery_preflight(
    board: &BoardAnswer,
    task: &TaskHandle,
    stealing: bool,
) -> Result<RecoveryPreflight, Outcome<CommandAnswer>> {
    let entries = actor_entries(board)?;
    if let Some(active) = entries.iter().find(|entry| {
        entry.claim().is_some_and(|claim| {
            claim.actor == board.principal && claim.disposition == ClaimDisposition::Active
        })
    }) {
        return Err(refusal(
            "claim::active-exists",
            Some(active.handle().to_string()),
            &format!(
                "run spice task reclaim {} before capturing another task",
                active.handle()
            ),
        ));
    }
    let heads = named_task_heads(board, task)?;
    let entry = entries
        .iter()
        .find(|entry| entry.born() == &heads.statement.body.born)
        .expect("actor entries retain every board task");
    planning::admit_capture(entry)?;
    let satisfied = board::resolved_tasks(&entries);
    let eligible = if stealing {
        board::stealable_entry(&satisfied, entry, &board.principal, &board.environment())
    } else {
        super::super::mutation::refuse_held_design(entry, &satisfied)?;
        board::eligible_entry(&satisfied, entry, &board.principal, &board.environment())
    };
    if !eligible {
        return Err(refusal(
            if stealing {
                "claim::steal-ineligible"
            } else {
                "claim::capture-ineligible"
            },
            Some(heads.statement.body.handle.to_string()),
            if stealing {
                "capture --steal only a ready, pending task held by another actor"
            } else {
                "capture only an unclaimed, ready, pending task"
            },
        ));
    }
    let (lane, occupant) = actor_seat(board)?;
    let rule = if stealing { "recover-steal" } else { "recover" }
        .parse::<Name>()
        .expect("recovery possession rules are wire names");
    let expected = vec![
        statement_witness(heads),
        workflow_witness(heads),
        claim_witness(heads),
        Witness::Eligibility { rule },
        residency_witness(board, lane, occupant),
    ];
    Ok(RecoveryPreflight {
        actor: board.principal.clone(),
        task: heads.statement.body.born.clone(),
        statement: heads.statement.position,
        workflow: heads.workflow.position,
        phase: heads.workflow.body.phase.clone(),
        claim: heads.claim.as_ref().map(|claim| claim.position),
        lane,
        occupant,
        epoch: board.residency.body.epoch,
        expected,
    })
}

pub(super) fn restore_client_recovery(repository: &Repository) -> Outcome<CommandAnswer> {
    let recovery = match spice::task_git::pending_recovery(repository) {
        Ok(Some(recovery)) => recovery,
        Ok(None) => {
            return refusal(
                "git::recovery-transaction-missing",
                None,
                "there is no retained dirty-handoff transaction to restore",
            );
        }
        Err(error) => return task_git_outcome(error),
    };
    let sealed = match spice::task_git::recovery_claim(repository, &recovery) {
        Ok(sealed) => sealed,
        Err(error) => return task_git_outcome(error),
    };
    let board = match current_board(repository, BoardRead::All { family: None }) {
        Ok(board) => board,
        Err(outcome) => return outcome,
    };
    let entries = match actor_entries(&board) {
        Ok(entries) => entries,
        Err(outcome) => return outcome,
    };
    if let Some(active) = entries.iter().find(|entry| {
        entry.claim().is_some_and(|claim| {
            claim.actor == board.principal && claim.disposition == ClaimDisposition::Active
        })
    }) {
        let exact = sealed.as_ref().is_some_and(|claim| {
            active.born() == &claim.task
                && active.claim().and_then(|head| head.recovery.as_deref())
                    == Some(recovery.transaction())
        });
        let remedy = if exact {
            format!(
                "resume the sealed target with `spice task capture {}`",
                active.handle()
            )
        } else {
            "release or complete the active claim before restoring unrelated dirty work".to_owned()
        };
        return refusal(
            if exact {
                "git::recovery-postclaim-active"
            } else {
                "claim::active-exists"
            },
            Some(active.handle().to_string()),
            &remedy,
        );
    }
    if let Some(claim) = &sealed {
        if board.principal != claim.actor {
            return refusal(
                "git::recovery-actor-moved",
                Some(claim.actor.to_string()),
                "restore through the lane that sealed the recovery transaction",
            );
        }
        let seat = actor_seat(&board);
        if !matches!(seat, Ok((lane, occupant)) if lane == claim.lane && occupant == claim.occupant)
            || board.residency.body.epoch != claim.epoch
        {
            return refusal(
                "git::recovery-residency-moved",
                Some(claim.lane.to_string()),
                "restore only through the original seated occupant and resident epoch",
            );
        }
    }
    let text = match spice::task_git::restore_recovery(repository, &recovery) {
        Ok(spice::task_git::RestoreOutcome::Restored) => {
            format!(
                "restored dirty-handoff transaction {}",
                recovery.transaction()
            )
        }
        Ok(spice::task_git::RestoreOutcome::Landed { retained }) => format!(
            "dirty-handoff transaction {} already landed on HEAD; nothing was replayed and the sealed recovery objects are retained at {}",
            recovery.transaction(),
            retained.display()
        ),
        Err(error) => return task_git_outcome(error),
    };
    Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body: CommandAnswer::Text(wire::intent::TextAnswer { text }),
    })
}

pub(super) fn resume_pending_client_capture(
    repository: &Repository,
    requested: Option<&TaskHandle>,
    recovery: spice::task_git::Recovery,
) -> Outcome<CommandAnswer> {
    let claim = match spice::task_git::recovery_claim(repository, &recovery) {
        Ok(Some(claim)) => claim,
        Ok(None) => {
            return refusal(
                "git::recovery-target-unbound",
                Some(recovery.transaction().to_owned()),
                "restore the interrupted adoption with `spice task capture --restore`",
            );
        }
        Err(error) => return task_git_outcome(error),
    };
    let exact =
        requested.is_some_and(|task| task.born().is_ok_and(|requested| requested == claim.task));
    if !exact {
        let target = claim
            .handle
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| claim.task.to_string());
        return refusal(
            "git::recovery-target-mismatch",
            Some(format!(
                "retained adoption {} is sealed for {target}",
                recovery.transaction()
            )),
            &format!(
                "resume the sealed target with `spice task capture {target}` or restore it with `spice task capture --restore`"
            ),
        );
    }
    let task = requested.expect("exact pending capture has a requested task");
    reconcile_client_recovery(repository, task, recovery, claim, None, false)
}

pub(in crate::board_command) enum RecoveryBoardState {
    PreClaim,
    PostClaim { first: Seq, last: Seq },
    Restorable(String),
    Ambiguous(String),
}

fn recovery_residency_matches(
    board: &BoardAnswer,
    sealed: &spice::task_git::RecoveryClaim,
) -> bool {
    matches!(
        actor_seat(board),
        Ok((lane, occupant)) if lane == sealed.lane && occupant == sealed.occupant
    ) && board.residency.body.epoch == sealed.epoch
}

fn active_recovery_claims<'a>(
    board: &'a BoardAnswer,
    actor: &Actor,
) -> (usize, Option<&'a wire::TaskStamp>) {
    let mut count = 0;
    let mut task = None;
    for candidate in &board.tasks {
        if candidate.claim.as_ref().is_some_and(|claim| {
            claim.body.actor == *actor && claim.body.disposition == ClaimDisposition::Active
        }) {
            count += 1;
            task = Some(&candidate.statement.body.born);
        }
    }
    (count, task)
}

fn recovery_preclaim_unchanged(heads: &BoardTask, sealed: &spice::task_git::RecoveryClaim) -> bool {
    !sealed.fresh
        && heads.statement.position == sealed.statement
        && heads.workflow.position == sealed.workflow
        && heads.workflow.body.phase == sealed.phase
        && heads.claim.as_ref().map(|claim| claim.position) == sealed.claim
}

fn exact_recovery_claim(
    heads: &BoardTask,
    sealed: &spice::task_git::RecoveryClaim,
    transaction: &str,
    active_count: usize,
    active_task: Option<&wire::TaskStamp>,
) -> bool {
    let Some(claim) = &heads.claim else {
        return false;
    };
    let statement_matches = sealed.fresh || heads.statement.position == sealed.statement;
    let workflow_matches = sealed.fresh
        || heads.workflow.position == sealed.workflow
        || (sealed.phase.as_str() == "review"
            && heads.workflow.body.status.reviewer() == Some(&sealed.actor));
    claim.body.actor == sealed.actor
        && claim.body.disposition == ClaimDisposition::Active
        && claim.body.cell.phase == sealed.phase
        && claim.body.lane == sealed.lane
        && claim.body.occupant == sealed.occupant
        && claim.body.epoch == sealed.epoch
        && claim.body.recovery.as_deref() == Some(transaction)
        && heads.workflow.body.phase == sealed.phase
        && statement_matches
        && workflow_matches
        && active_count == 1
        && active_task == Some(&sealed.task)
}

pub(in crate::board_command) fn recovery_board_state(
    board: &BoardAnswer,
    task: &TaskHandle,
    transaction: &str,
    sealed: &spice::task_git::RecoveryClaim,
) -> RecoveryBoardState {
    if board.principal != sealed.actor {
        return RecoveryBoardState::Ambiguous(format!(
            "task {task} recovery belongs to actor {}",
            sealed.actor
        ));
    }
    if !recovery_residency_matches(board, sealed) {
        return RecoveryBoardState::Ambiguous(format!(
            "task {task} residency or resident epoch moved"
        ));
    }
    let (active_count, active_task) = active_recovery_claims(board, &sealed.actor);
    let heads = match task_heads(board, task) {
        Ok(heads) if heads.statement.body.born == sealed.task => heads,
        Ok(_) => {
            return RecoveryBoardState::Ambiguous(format!(
                "task {task} resolved to another durable identity"
            ));
        }
        Err(_) if sealed.fresh && active_count == 0 => return RecoveryBoardState::PreClaim,
        Err(_) if active_count == 0 => {
            return RecoveryBoardState::Restorable(format!(
                "task {task} moved out of the recovery projection"
            ));
        }
        Err(_) => {
            return RecoveryBoardState::Ambiguous(format!(
                "task {task} is unavailable while actor {} holds a claim",
                sealed.actor
            ));
        }
    };
    if recovery_preclaim_unchanged(heads, sealed) {
        return if active_count == 0 {
            RecoveryBoardState::PreClaim
        } else {
            RecoveryBoardState::Ambiguous(format!(
                "actor {} acquired another active claim",
                sealed.actor
            ))
        };
    }
    let Some(claim) = &heads.claim else {
        return if sealed.fresh {
            RecoveryBoardState::Ambiguous(format!(
                "atomically created task {task} has no claim head"
            ))
        } else if active_count == 0 {
            RecoveryBoardState::Restorable(format!(
                "task {task} claim head disappeared before recovery"
            ))
        } else {
            RecoveryBoardState::Ambiguous(format!(
                "task {task} claim head disappeared while actor {} holds a claim",
                sealed.actor
            ))
        };
    };
    if exact_recovery_claim(heads, sealed, transaction, active_count, active_task) {
        RecoveryBoardState::PostClaim {
            first: if sealed.fresh || heads.workflow.position == sealed.workflow {
                claim.position
            } else {
                heads.workflow.position
            },
            last: claim.position,
        }
    } else if active_count == 0 {
        RecoveryBoardState::Restorable(format!(
            "task {task} advanced without recovery {transaction}"
        ))
    } else {
        RecoveryBoardState::Ambiguous(format!(
            "task {task} does not carry the only active recovery {transaction}"
        ))
    }
}

fn reconcile_client_recovery(
    repository: &Repository,
    task: &TaskHandle,
    recovery: spice::task_git::Recovery,
    sealed: spice::task_git::RecoveryClaim,
    outcome: Option<Outcome<CommandAnswer>>,
    task_reply: bool,
) -> Outcome<CommandAnswer> {
    let board = match current_board(
        repository,
        BoardRead::Tasks {
            tasks: vec![sealed.task.clone()],
            dependencies: false,
            claims: true,
            history: false,
        },
    ) {
        Ok(board) => board,
        Err(read) => return read,
    };
    match recovery_board_state(&board, task, recovery.transaction(), &sealed) {
        RecoveryBoardState::PreClaim => {
            if outcome
                .as_ref()
                .is_some_and(|outcome| matches!(outcome, Outcome::Reply(_)))
            {
                return refusal(
                    "git::recovery-claim-receipt-ambiguous",
                    Some(recovery.transaction().to_owned()),
                    "retain the quarantine; the successful claim receipt is not reflected by current board heads",
                );
            }
            if let Err(error) = spice::task_git::restore_recovery(repository, &recovery) {
                return task_git_outcome(error);
            }
            outcome.unwrap_or_else(|| {
                refusal(
                    "git::recovery-preclaim-restored",
                    Some(recovery.transaction().to_owned()),
                    "the interrupted pre-claim generation was restored; retry task capture",
                )
            })
        }
        RecoveryBoardState::PostClaim { first, last } => {
            if let Err(error) = spice::task_git::replay_recovery(repository, &recovery) {
                return task_git_outcome(error);
            }
            outcome
                .filter(|outcome| matches!(outcome, Outcome::Reply(_)))
                .unwrap_or_else(|| {
                    let claim = wire::intent::ClaimAnswer {
                        task: task.clone(),
                        holder: Some(sealed.actor),
                        expiry: None,
                        warnings: Box::default(),
                        creation_context: None,
                        origin_context: None,
                        room_context: None,
                        next: None,
                        phase: None,
                        transplant: None,
                    };
                    let body = if task_reply {
                        CommandAnswer::Task(wire::intent::TaskAnswer {
                            task: task.clone(),
                            status: "pending".to_owned(),
                            completion: None,
                            claim: Some(Box::new(claim)),
                            closed: Vec::new(),
                            next: None,
                        })
                    } else {
                        CommandAnswer::Claim(Box::new(claim))
                    };
                    Outcome::Reply(Reply {
                        position: ReplyPosition::Committed(SeqRange { first, last }),
                        body,
                    })
                })
        }
        RecoveryBoardState::Restorable(detail) => refusal(
            "git::recovery-preclaim-moved",
            Some(detail),
            "restore the exact quarantined generation with `spice task capture --restore`",
        ),
        RecoveryBoardState::Ambiguous(detail) => refusal(
            "git::recovery-board-ambiguous",
            Some(detail),
            "retain the quarantine; inspect the task claim, lane residency, and Git boundary before retrying",
        ),
    }
}

fn rollback_recovery_error(
    repository: &Repository,
    recovery: &spice::task_git::Recovery,
    error: spice::task_git::Error,
) -> Outcome<CommandAnswer> {
    let outcome = task_git_outcome(error);
    match spice::task_git::restore_recovery(repository, recovery) {
        Ok(_) => outcome,
        Err(restore) => task_git_outcome(restore),
    }
}
