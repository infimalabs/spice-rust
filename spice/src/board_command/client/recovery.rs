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

pub(super) fn client_recover(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let written = required(matches, "HANDLE");
    let task = match written.parse::<TaskHandle>() {
        Ok(task) => task,
        Err(error) => return refusal("task::handle-invalid", Some(written), &error.to_string()),
    };
    let pending = match spice::task_git::pending_recovery(repository) {
        Ok(pending) => pending,
        Err(error) => return task_git_outcome(error),
    };
    if let Some(recovery) = pending {
        return resume_client_recovery(repository, &task, recovery);
    }
    let board = match current_board(repository) {
        Ok(board) => board,
        Err(outcome) => return outcome,
    };
    let preflight = match recovery_preflight(&board, &task) {
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
        // intent. Preserve that evidence; restoring here would destroy the
        // ambiguity the manifest exists to make inspectable.
        return task_git_outcome(error);
    }
    let transaction = recovery.transaction().to_owned();
    let rule = "recover".parse::<Name>().expect("recover is a wire name");
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
    reconcile_client_recovery(repository, &task, recovery, claim, Some(outcome))
}

pub(in crate::board_command) fn recovery_preflight(
    board: &BoardAnswer,
    task: &TaskHandle,
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
                "run spice task reclaim {} before recovering another task",
                active.handle()
            ),
        ));
    }
    let heads = task_heads(board, task)?;
    let entry = entries
        .iter()
        .find(|entry| entry.born() == &heads.statement.body.born)
        .expect("actor entries retain every board task");
    if !board::eligible_entry(&entries, entry, &board.principal) {
        return Err(refusal(
            "git::recovery-task-ineligible",
            Some(task.to_string()),
            "recover only an unclaimed, ready, pending task",
        ));
    }
    let (lane, occupant) = actor_seat(board)?;
    let expected = vec![
        statement_witness(heads),
        workflow_witness(heads),
        claim_witness(heads),
        Witness::Eligibility {
            rule: "recover".parse::<Name>().expect("recover is a wire name"),
        },
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

fn resume_client_recovery(
    repository: &Repository,
    task: &TaskHandle,
    recovery: spice::task_git::Recovery,
) -> Outcome<CommandAnswer> {
    let claim = match spice::task_git::recovery_claim(repository, &recovery) {
        Ok(Some(claim)) => claim,
        Ok(None) => {
            return match spice::task_git::restore_recovery(repository, &recovery) {
                Ok(()) => refusal(
                    "git::recovery-preclaim-restored",
                    Some(recovery.transaction().to_owned()),
                    "the interrupted pre-claim generation was restored; retry task recover",
                ),
                Err(error) => task_git_outcome(error),
            };
        }
        Err(error) => return task_git_outcome(error),
    };
    reconcile_client_recovery(repository, task, recovery, claim, None)
}

pub(in crate::board_command) enum RecoveryBoardState {
    PreClaim,
    PostClaim { first: Seq, last: Seq },
    Ambiguous(String),
}

pub(in crate::board_command) fn recovery_board_state(
    board: &BoardAnswer,
    task: &TaskHandle,
    transaction: &str,
    sealed: &spice::task_git::RecoveryClaim,
) -> RecoveryBoardState {
    let Some(heads) = board
        .tasks
        .iter()
        .find(|heads| heads.statement.body.handle == *task)
    else {
        return RecoveryBoardState::Ambiguous(format!("task {task} is no longer visible"));
    };
    if board.principal != sealed.actor
        || heads.statement.body.born != sealed.task
        || heads.statement.position != sealed.statement
        || heads.workflow.body.phase != sealed.phase
    {
        return RecoveryBoardState::Ambiguous(format!(
            "task {task} actor, statement, workflow, or phase moved"
        ));
    }
    let seat = actor_seat(board);
    if !matches!(seat, Ok((lane, occupant)) if lane == sealed.lane && occupant == sealed.occupant)
        || board.residency.body.epoch != sealed.epoch
    {
        return RecoveryBoardState::Ambiguous(format!(
            "task {task} residency or resident epoch moved"
        ));
    }
    let position = heads.claim.as_ref().map(|claim| claim.position);
    if position == sealed.claim {
        if heads.workflow.position != sealed.workflow {
            return RecoveryBoardState::Ambiguous(format!(
                "task {task} workflow moved before its recovery claim"
            ));
        }
        let actor_holds_claim = board.tasks.iter().any(|candidate| {
            candidate.claim.as_ref().is_some_and(|claim| {
                claim.body.actor == sealed.actor
                    && claim.body.disposition == ClaimDisposition::Active
            })
        });
        return if actor_holds_claim {
            RecoveryBoardState::Ambiguous(format!(
                "actor {} acquired another active claim",
                sealed.actor
            ))
        } else {
            RecoveryBoardState::PreClaim
        };
    }
    let Some(claim) = &heads.claim else {
        return RecoveryBoardState::Ambiguous(format!("task {task} claim head disappeared"));
    };
    let workflow_matches_claim = heads.workflow.position == sealed.workflow
        || (sealed.phase.as_str() == "review"
            && heads.workflow.body.assignment.as_ref() == Some(&sealed.actor)
            && heads.workflow.body.review == "assigned");
    let active = board
        .tasks
        .iter()
        .filter(|candidate| {
            candidate.claim.as_ref().is_some_and(|claim| {
                claim.body.actor == sealed.actor
                    && claim.body.disposition == ClaimDisposition::Active
            })
        })
        .collect::<Vec<_>>();
    if claim.body.actor == sealed.actor
        && claim.body.disposition == ClaimDisposition::Active
        && claim.body.cell.phase == sealed.phase
        && claim.body.lane == sealed.lane
        && claim.body.occupant == sealed.occupant
        && claim.body.epoch == sealed.epoch
        && claim.body.recovery.as_deref() == Some(transaction)
        && workflow_matches_claim
        && matches!(active.as_slice(), [only] if only.statement.body.born == sealed.task)
    {
        RecoveryBoardState::PostClaim {
            first: if heads.workflow.position == sealed.workflow {
                claim.position
            } else {
                heads.workflow.position
            },
            last: claim.position,
        }
    } else {
        RecoveryBoardState::Ambiguous(format!(
            "task {task} claim does not carry recovery {transaction}"
        ))
    }
}

fn reconcile_client_recovery(
    repository: &Repository,
    task: &TaskHandle,
    recovery: spice::task_git::Recovery,
    sealed: spice::task_git::RecoveryClaim,
    outcome: Option<Outcome<CommandAnswer>>,
) -> Outcome<CommandAnswer> {
    let board = match current_board(repository) {
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
                    "the interrupted pre-claim generation was restored; retry task recover",
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
                    Outcome::Reply(Reply {
                        position: ReplyPosition::Committed(SeqRange { first, last }),
                        body: CommandAnswer::Claim(wire::intent::ClaimAnswer {
                            task: task.clone(),
                            holder: sealed.actor,
                            expiry: None,
                            warnings: Box::default(),
                            next: None,
                        }),
                    })
                })
        }
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
        Ok(()) => outcome,
        Err(restore) => task_git_outcome(restore),
    }
}
