fn phase(name: &str) -> wire::Phase {
    name.parse()
        .expect("dialect phase is a lower-case wire phase")
}

fn validated_snapshot(
    bus: &Bus,
    principal: wire::Actor,
    projects: &ProjectPolicy,
) -> Result<ValidatedSnapshot, Outcome<CommandAnswer>> {
    let observed = bus
        .task_snapshot()
        .map_err(|error| store_fault("board::heads-unreadable", &error.to_string()))?;
    let cursor = observed.cursor().ok_or_else(|| {
        store_fault(
            "board::cursor-missing",
            "a running resident has no durable snapshot cursor",
        )
    })?;
    let residency = observed.residency().ok_or_else(|| {
        store_fault(
            "board::residency-missing",
            "a running resident has no current residency head",
        )
    })?;
    let mut residency = positioned(residency);
    for lane in &mut residency.body.lanes {
        lane.capabilities.clear();
    }
    let mut tasks = Vec::with_capacity(observed.tasks().len());
    let mut entries = Vec::with_capacity(observed.tasks().len());
    for heads in observed.tasks() {
        let statement = positioned(heads.statement());
        let workflow = positioned(heads.workflow());
        let claim = heads.claim().map(positioned);
        let lease = heads.lease().map(positioned);
        let handle = statement.body.handle.clone();
        let entry = Entry::new(
            statement.body.clone(),
            workflow.body.clone(),
            claim.as_ref().map(|head| head.body.clone()),
            lease.as_ref().map(|head| head.body.clone()),
            projects,
        )
        .map_err(|error| {
            store_fault(
                "board::head-invalid",
                &format!("task {handle} current heads are invalid: {error}"),
            )
        })?;
        let visibility = entry.project().visibility().clone();
        entries.push(entry);
        tasks.push(BoardTask::from_authoritative(
            visibility,
            statement,
            workflow,
            claim,
            lease,
        ));
    }
    Ok(ValidatedSnapshot {
        cursor,
        answer: BoardAnswer {
            schema: journal::SCHEMA_VERSION,
            cursor,
            principal,
            tasks,
            residency,
        },
        entries,
    })
}

fn positioned<T: Clone>(head: &journal::JournalHead<T>) -> Positioned<T> {
    Positioned {
        position: head.position(),
        body: head.body().clone(),
    }
}

fn task_repository(
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    resident: &Repository,
) -> Result<Repository, Outcome<CommandAnswer>> {
    let Some((lane, occupant)) = command.expected().iter().find_map(|witness| match witness {
        Witness::Residency {
            lane,
            occupant,
            epoch,
        } if *epoch == command.epoch() => Some((*lane, *occupant)),
        _ => None,
    }) else {
        return Err(protocol_fault(
            "git::residency-witness",
            "task Git boundary has no current residency witness",
        ));
    };
    let Some(row) = snapshot
        .answer
        .residency
        .body
        .lanes
        .iter()
        .find(|row| row.lane == lane && row.occupant == Some(occupant))
    else {
        return Err(refusal(
            "git::lane-moved",
            Some(lane.to_string()),
            "retry through the lane and occupant that currently hold the claim",
        ));
    };
    let repository = Repository::discover(&row.worktree).map_err(|error| {
        environment_fault("git::worktree-unresolved", &error.to_string())
    })?;
    if repository.common_dir() != resident.common_dir()
        || repository.worktree_root() != std::path::Path::new(&row.worktree)
    {
        return Err(environment_fault(
            "git::worktree-moved",
            "the claimed lane no longer names one worktree in the resident repository",
        ));
    }
    Ok(repository)
}

fn publish_task_boundary(
    repository: Option<&Repository>,
    snapshot: &ValidatedSnapshot,
    command: &wire::AdmittedCommand,
    entry: &Entry,
) -> Result<Option<GitBoundary>, Outcome<CommandAnswer>> {
    let Some(resident) = repository else {
        return Ok(None);
    };
    let lane = task_repository(snapshot, command, resident)?;
    let actor = command.actor().to_string();
    spice::task_git::publish(
        &lane,
        spice::task_git::Metadata {
            label: entry.statement().handle.as_str(),
            title: &entry.statement().title,
            project: &entry.statement().project,
            phase: entry.workflow().phase.as_str(),
            actor: &actor,
        },
    )
    .map(Some)
    .map_err(task_git_outcome)
}

fn task_git_outcome(error: spice::task_git::Error) -> Outcome<CommandAnswer> {
    match error.class() {
        spice::task_git::ErrorClass::Refusal => refusal(
            error.reason(),
            Some(error.detail().to_owned()),
            error.remedy(),
        ),
        spice::task_git::ErrorClass::Fault => {
            environment_fault(error.reason(), error.detail())
        }
    }
}

fn read_board(
    bus: &Bus,
    read: &BoardRead,
    principal: wire::Actor,
    projects: &ProjectPolicy,
) -> Outcome<CommandAnswer> {
    let mut snapshot = match validated_snapshot(bus, principal, projects) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    if let Some(family) = &read.family {
        if family.is_empty()
            || !family
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        {
            return refusal(
                "board::family-invalid",
                Some(family.clone()),
                "name one upper-case task family",
            );
        }
        snapshot.answer.tasks.retain(|task| {
            task.statement
                .body
                .handle
                .as_str()
                .split_once('-')
                .is_some_and(|(stem, _)| stem == family)
        });
    }
    snapshot_reply(
        snapshot.cursor,
        CommandAnswer::Board(Box::new(snapshot.answer)),
    )
}

fn read_graph(
    bus: &Bus,
    graph: &BoardGraph,
    principal: wire::Actor,
    projects: &ProjectPolicy,
) -> Outcome<CommandAnswer> {
    if let Some(at) = &graph.at {
        return refusal(
            "board::historical-barrier-refused",
            Some(at.clone()),
            "omit --at to read the current validated board barrier",
        );
    }
    let view = match graph.view.as_str() {
        "dependencies" => View::Dependencies,
        "workflow" => View::Workflow,
        "claims" => View::Claims,
        other => {
            return refusal(
                "board::graph-view-invalid",
                Some(other.to_owned()),
                "choose dependencies, workflow, or claims",
            );
        }
    };
    let snapshot = match validated_snapshot(bus, principal, projects) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    snapshot_reply(
        snapshot.cursor,
        CommandAnswer::Text(TextAnswer {
            text: board::graph::render(view, &snapshot.entries)
                .text()
                .to_owned(),
        }),
    )
}

fn read_handout(
    bus: &Bus,
    handout: &BoardHandout,
    principal: wire::Actor,
    projects: &ProjectPolicy,
) -> Outcome<CommandAnswer> {
    if let Some(at) = &handout.at {
        return refusal(
            "board::historical-barrier-refused",
            Some(at.clone()),
            "omit --at to read the current validated board barrier",
        );
    }
    if handout.output.trim().is_empty() {
        return refusal(
            "board::handout-output-invalid",
            Some(handout.output.clone()),
            "name an output path or -",
        );
    }
    let snapshot = match validated_snapshot(bus, principal, projects) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    snapshot_reply(
        snapshot.cursor,
        CommandAnswer::Text(TextAnswer {
            text: board::handout::compose(&snapshot.entries),
        }),
    )
}

fn read_ledger(
    bus: &Bus,
    ledger: &DocLedger,
    principal: wire::Actor,
    projects: &ProjectPolicy,
) -> Outcome<CommandAnswer> {
    let snapshot = match validated_snapshot(bus, principal, projects) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(task) = snapshot
        .answer
        .tasks
        .iter()
        .find(|task| task.statement.body.handle == ledger.task)
    else {
        return refusal(
            "doc::task-unknown",
            Some(ledger.task.to_string()),
            "read board and name one current task handle",
        );
    };
    let statement = &task.statement.body;
    let mut source = format!("# {}\n", task.presentation_title());
    source.push_str(&format!(
        "\nFlow: {}\n",
        join_phases(&task.workflow.body.flow)
    ));
    if !statement.tags.is_empty() {
        source.push_str(&format!("Tags: {}\n", statement.tags.join(", ")));
    }
    let Some(document) = dialect::classify(&source).document().cloned() else {
        return store_fault(
            "doc::ledger-unrepresentable",
            "current task heads did not classify as a canonical task document",
        );
    };
    snapshot_reply(
        snapshot.cursor,
        CommandAnswer::Text(TextAnswer {
            text: dialect::emit(&document),
        }),
    )
}

fn join_phases(phases: &[wire::Phase]) -> String {
    phases
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn request<B>(command: &wire::AdmittedCommand, body: B) -> Command<B> {
    Command {
        v: WireVersion::CURRENT,
        id: command.id(),
        epoch: command.epoch(),
        kind: command.kind(),
        expected: command.expected().to_vec(),
        body,
    }
}

fn guard(write: &mut JournalWrite, expected: Vec<Witness>) {
    write.guard(JournalGuard::new(move |read| {
        witnesses_hold(read, &expected)
    }));
}

type AllocationHeads = Vec<(TaskHandle, Seq, Seq, Option<Seq>, Option<Seq>)>;

fn allocation_heads(board: &BoardAnswer) -> AllocationHeads {
    board
        .tasks
        .iter()
        .map(|task| {
            (
                task.statement.body.handle.clone(),
                task.statement.position,
                task.workflow.position,
                task.claim.as_ref().map(|head| head.position),
                task.lease.as_ref().map(|head| head.position),
            )
        })
        .collect()
}

fn guard_allocation(
    write: &mut JournalWrite,
    expected: Vec<Witness>,
    observed: AllocationHeads,
    winner: TaskHandle,
    actor: Actor,
    projects: ProjectPolicy,
) {
    write.guard(JournalGuard::new(move |read| {
        witnesses_hold(read, &expected)?;
        let heads = read.task_heads()?;
        let current = heads
            .iter()
            .map(|task| {
                (
                    task.statement().body().handle.clone(),
                    task.statement().position(),
                    task.workflow().position(),
                    task.claim().map(journal::JournalHead::position),
                    task.lease().map(journal::JournalHead::position),
                )
            })
            .collect::<AllocationHeads>();
        if current != observed {
            return Err(GuardError::refused(
                "board::witness-moved: allocator head set changed",
            ));
        }
        let entries = heads
            .iter()
            .map(|task| {
                Entry::new(
                    task.statement().body().clone(),
                    task.workflow().body().clone(),
                    task.claim().map(|head| head.body().clone()),
                    task.lease().map(|head| head.body().clone()),
                    &projects,
                )
                .map_err(|error| {
                    GuardError::Invalid(format!("allocator current heads are invalid: {error}"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if board::select(&entries, &actor)
            .is_some_and(|selected| selected.entry().statement().handle == winner)
        {
            Ok(())
        } else {
            Err(GuardError::refused(
                "board::witness-moved: allocator winner changed",
            ))
        }
    }));
}

fn guard_capture(
    write: &mut JournalWrite,
    expected: Vec<Witness>,
    observed: AllocationHeads,
) {
    write.guard(JournalGuard::new(move |read| {
        witnesses_hold(read, &expected)?;
        let current = read
            .task_heads()?
            .iter()
            .map(|task| {
                (
                    task.statement().body().handle.clone(),
                    task.statement().position(),
                    task.workflow().position(),
                    task.claim().map(journal::JournalHead::position),
                    task.lease().map(journal::JournalHead::position),
                )
            })
            .collect::<AllocationHeads>();
        if current == observed {
            Ok(())
        } else {
            Err(GuardError::refused(
                "board::witness-moved: capture head set changed",
            ))
        }
    }));
}

fn witnesses_hold(read: &JournalRead<'_>, expected: &[Witness]) -> Result<(), GuardError> {
    for witness in expected {
        let holds = match witness {
            Witness::Task { task, head } => read
                .task_head(task)?
                .is_some_and(|observed| observed.position() == *head),
            Witness::Workflow { task, head, phase } => {
                read.workflow_head(task)?.is_some_and(|observed| {
                    observed.position() == *head && observed.body().phase == *phase
                })
            }
            Witness::Claim { task, head } => {
                read.claim_head(task)?.map(|observed| observed.position()) == *head
            }
            Witness::Lease { task, head } => read
                .lease_head(task)?
                .is_some_and(|observed| observed.position() == *head),
            Witness::Residency {
                lane,
                occupant,
                epoch,
            } => read.residency_head()?.is_some_and(|observed| {
                observed.body().epoch == *epoch
                    && observed.body().lanes.iter().any(|seat| {
                        seat.lane == *lane
                            && seat.occupant == Some(*occupant)
                            && seat.disposition == "occupied"
                    })
            }),
            Witness::Absence { task } => read.task_head(task)?.is_none(),
            Witness::Existence { task } => read.task_head(task)?.is_some(),
            Witness::Edge { task, after } => {
                let subject = read.task_head(task)?;
                let dependency = read.task_head(after)?;
                subject.is_some_and(|head| !head.body().dependencies.contains(after))
                    && dependency.is_some()
            }
            Witness::Deferral {
                task,
                head,
                deferred,
            } => read.workflow_head(task)?.is_some_and(|observed| {
                observed.position() == *head && observed.body().deferral.is_some() == *deferred
            }),
            Witness::Eligibility { rule } => {
                matches!(rule.as_str(), "next" | "capture")
            }
            Witness::Assignment {
                task,
                head,
                reviewer,
            } => read.workflow_head(task)?.is_some_and(|observed| {
                observed.position() == *head
                    && observed.body().assignment.as_ref() == Some(reviewer)
            }),
            Witness::Preview { .. } => true,
            Witness::Barrier { name, digest } => {
                name.as_str() == "board" && board_barrier_from_read(read)? == *digest
            }
            Witness::Directive { .. }
            | Witness::Lineage { .. }
            | Witness::Consent { .. }
            | Witness::Rule { .. }
            | Witness::Budget { .. }
            | Witness::Trust { .. }
            | Witness::Taste { .. }
            | Witness::Observation { .. }
            | Witness::Sealed { .. }
            | Witness::Transition { .. } => {
                return Err(GuardError::Invalid(
                    "board transaction carried a witness outside its authority plane".to_owned(),
                ));
            }
        };
        if !holds {
            return Err(GuardError::refused(format!(
                "board::witness-moved: {witness:?}"
            )));
        }
    }
    Ok(())
}

fn committed(bus: &Bus, write: JournalWrite, body: CommandAnswer) -> Outcome<CommandAnswer> {
    match bus.commit(write) {
        Ok(CommitOutcome::Committed(commit)) => Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(commit.first()),
                last: seq(commit.last()),
            }),
            body,
        }),
        Ok(CommitOutcome::Refused(refusal)) => Outcome::Refusal(Refusal {
            reason: "board::witness-moved".to_owned(),
            witness: Some("current durable heads".to_owned()),
            expected: None,
            observed: Some(refusal.reason().to_owned()),
            remedy: "read the current board and retry the exact task operation".to_owned(),
        }),
        Ok(CommitOutcome::Fault(fault)) => store_fault("board::commit-failed", fault.detail()),
        Err(error) => store_fault("board::bus-closed", &error.to_string()),
    }
}

fn phases(flow: Option<&str>) -> Result<Vec<wire::Phase>, Outcome<CommandAnswer>> {
    let names = flow.map_or_else(
        || vec!["todo", "review"],
        |flow| {
            flow.split(',')
                .map(str::trim)
                .filter(|phase| !phase.is_empty())
                .collect()
        },
    );
    if names.is_empty() {
        return Err(refusal(
            "task::flow-empty",
            flow.map(str::to_owned),
            "name at least one lower-case workflow phase",
        ));
    }
    names
        .into_iter()
        .map(|phase| {
            phase.parse::<wire::Phase>().map_err(|error| {
                refusal(
                    "task::flow-invalid",
                    Some(phase.to_owned()),
                    &error.to_string(),
                )
            })
        })
        .collect()
}

fn board_barrier_from_read(read: &JournalRead<'_>) -> Result<Digest, GuardError> {
    let heads = read
        .task_heads()?
        .into_iter()
        .map(|task| {
            (
                task.statement().body().handle.clone(),
                task.statement().position(),
                task.workflow().position(),
            )
        })
        .collect::<Vec<_>>();
    let bytes = wire::encode(&heads)
        .map_err(|error| GuardError::Invalid(format!("board barrier did not encode: {error}")))?;
    Ok(Digest::compute(Domain::BoardBarrier, &bytes))
}

fn task_origin(task: &TaskHandle) -> Result<wire::Origin, Outcome<CommandAnswer>> {
    format!("task:{task}").parse().map_err(|error| {
        protocol_fault(
            "task::origin-invalid",
            &format!("validated handle did not form a task origin: {error}"),
        )
    })
}

fn current_instant() -> Result<Instant, Outcome<CommandAnswer>> {
    Instant::new(jiff::Timestamp::now())
        .map_err(|error| environment_fault("board::clock-invalid", &error.to_string()))
}

fn instant_after(at: Instant, seconds: i64) -> Result<Instant, Outcome<CommandAnswer>> {
    let timestamp = at
        .get()
        .checked_add(jiff::SignedDuration::from_secs(seconds))
        .map_err(|error| environment_fault("board::clock-range", &error.to_string()))?;
    Instant::new(timestamp)
        .map_err(|error| environment_fault("board::clock-invalid", &error.to_string()))
}

fn seq(value: u64) -> Seq {
    Seq::new(value).expect("journal commits use non-zero positions")
}

fn snapshot_reply(cursor: wire::Seq, body: CommandAnswer) -> Outcome<CommandAnswer> {
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(cursor),
        body,
    })
}

fn required(matches: &ArgMatches, name: &str) -> String {
    matches
        .get_one::<String>(name)
        .expect("generated syntax requires this argument")
        .clone()
}

fn refusal(reason: &str, observed: Option<String>, remedy: &str) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: None,
        expected: None,
        observed,
        remedy: remedy.to_owned(),
    })
}

fn store_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Store,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

fn environment_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

fn protocol_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    Outcome::Fault(Fault {
        class: FaultClass::Protocol,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}
