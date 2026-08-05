fn dispatch_release_publish(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    command: &wire::AdmittedCommand,
    publish: &ReleasePublish,
) -> Outcome<CommandAnswer> {
    let prepared = match prepare_release_publish(bus, paths, command, publish) {
        Ok(prepared) => prepared,
        Err(outcome) => return outcome,
    };
    if let crate::release_command::PublicationState::Published { fact } = prepared.state {
        return release_recognition(publish.key.clone(), fact, fact, false);
    }
    let crate::release_command::PublicationState::Fresh { proof } = prepared.state else {
        unreachable!("published state returned above")
    };
    let origin = prepared
        .snapshot
        .facts()
        .iter()
        .find(|fact| fact.seq == proof)
        .map(|fact| fact.origin.clone())
        .expect("observed proof position belongs to the same canonical snapshot");
    let proposal = match propose_release_publish(
        paths,
        command,
        publish,
        prepared.release,
        prepared.state,
        proof,
        origin,
        epoch,
    ) {
        Ok(proposal) => proposal,
        Err(outcome) => return outcome,
    };
    match commit(bus, proposal) {
        Ok(committed) => release_recognition(
            publish.key.clone(),
            seq(committed.first()),
            seq(committed.last()),
            true,
        ),
        Err(outcome) => typed_outcome(outcome),
    }
}

struct PreparedReleasePublish {
    release: crate::release_command::AdmittedRelease,
    snapshot: journal::FactSnapshot,
    state: crate::release_command::PublicationState,
}

fn prepare_release_publish(
    bus: &Bus,
    paths: &RuntimePaths,
    command: &wire::AdmittedCommand,
    publish: &ReleasePublish,
) -> Result<PreparedReleasePublish, Outcome<CommandAnswer>> {
    let release = crate::release_command::admit_stage(&paths.stage)
        .map_err(|error| release_refusal("release::manifest-refused", &error.to_string()))?;
    if publish.key != *release.key() || publish.manifest != *release.manifest() {
        return Err(Outcome::Refusal(Refusal {
            reason: "release::plan-moved".to_owned(),
            witness: Some("publication-plan".to_owned()),
            expected: Some(format!("{}:{}", release.key(), release.manifest())),
            observed: Some(format!("{}:{}", publish.key, publish.manifest)),
            remedy: "preview the current sealed manifest and apply its exact key".to_owned(),
        }));
    }
    let snapshot = bus
        .fact_snapshot()
        .map_err(|error| typed_store_fault("release::journal-unreadable", &error.to_string()))?;
    let state = crate::release_command::observe(&snapshot, &release)
        .map_err(|error| release_refusal("release::evidence-refused", &error.to_string()))?;
    if !release_witnesses_match(command.expected(), &release, state) {
        return Err(Outcome::Refusal(Refusal {
            reason: "release::witness-moved".to_owned(),
            witness: Some("observation-and-seals".to_owned()),
            expected: Some(format!(
                "one {} observation at {} and both detached receipts",
                release.key(),
                state.observation()
            )),
            observed: Some(format!("{:?}", command.expected())),
            remedy: "re-read the current release plan and retry its exact witnesses".to_owned(),
        }));
    }
    Ok(PreparedReleasePublish {
        release,
        snapshot,
        state,
    })
}

#[allow(clippy::too_many_arguments)]
fn propose_release_publish(
    paths: &RuntimePaths,
    command: &wire::AdmittedCommand,
    publish: &ReleasePublish,
    release: crate::release_command::AdmittedRelease,
    state: crate::release_command::PublicationState,
    proof: Seq,
    origin: wire::Origin,
    epoch: CanonicalUuid,
) -> Result<JournalWrite, Outcome<CommandAnswer>> {
    let mut proposal = write_at_origin(
        command.actor().clone(),
        origin,
        Producer::ResidentRecognition,
    )
    .map_err(typed_outcome)?;
    let guarded_release = release.clone();
    let guarded_stage = paths.stage.clone();
    proposal.guard(JournalGuard::new(move |read| {
        let current = crate::release_command::admit_stage(&guarded_stage)
            .map_err(|_| GuardError::refused("release::manifest-moved"))?;
        if current != guarded_release {
            return Err(GuardError::refused("release::manifest-moved"));
        }
        let snapshot = read
            .fact_snapshot()
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        let current = crate::release_command::observe(&snapshot, &current)
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        if current != state {
            return Err(GuardError::refused("release::observation-moved"));
        }
        Ok(())
    }));
    proposal
        .fact(release.fact(), FactCause::Existing(proof))
        .map_err(|error| typed_store_fault("release::fact-invalid", &error.to_string()))?;
    let answer = RecognitionAnswer {
        key: publish.key.clone(),
        fresh: true,
    };
    proposal
        .command_result(
            Command {
                v: wire::WireVersion::CURRENT,
                id: command.id(),
                epoch: command.epoch(),
                kind: CommandKind::ReleasePublish,
                expected: command.expected().to_vec(),
                body: publish.clone(),
            },
            epoch,
            answer,
        )
        .map_err(|error| typed_store_fault("release::receipt-invalid", &error.to_string()))?;
    Ok(proposal)
}

fn release_recognition(
    key: wire::Digest,
    first: Seq,
    last: Seq,
    fresh: bool,
) -> Outcome<CommandAnswer> {
    Outcome::Reply(Reply {
        position: ReplyPosition::Committed(SeqRange { first, last }),
        body: CommandAnswer::Recognition(RecognitionAnswer { key, fresh }),
    })
}

fn release_witnesses_match(
    witnesses: &[Witness],
    release: &crate::release_command::AdmittedRelease,
    state: crate::release_command::PublicationState,
) -> bool {
    let observations = witnesses
        .iter()
        .filter_map(|witness| match witness {
            Witness::Observation { key, head } => Some((key, *head)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut sealed = witnesses
        .iter()
        .filter_map(|witness| match witness {
            Witness::Sealed { proof, manifest } if manifest == release.manifest() => Some(proof),
            Witness::Sealed { .. } => None,
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut receipts = release.receipts().collect::<Vec<_>>();
    sealed.sort_unstable();
    receipts.sort_unstable();
    witnesses.len() == 3
        && observations == [(release.key(), state.observation())]
        && sealed == receipts
}

fn release_refusal(reason: &str, observed: &str) -> Outcome<CommandAnswer> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: Some("sealed-release".to_owned()),
        expected: Some("one current complete two-target proof and manifest chain".to_owned()),
        observed: Some(observed.to_owned()),
        remedy: "restore and recognize the exact current sealed release before publishing"
            .to_owned(),
    })
}

fn dispatch_up_change(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    command: &wire::AdmittedCommand,
    change: &wire::intent::UpChange,
) -> Outcome<CommandAnswer> {
    if !matches!(change.state, ResidentState::Dark) {
        return Outcome::Refusal(Refusal {
            reason: "down::target-not-dark".to_owned(),
            witness: Some("transition".to_owned()),
            expected: Some("dark".to_owned()),
            observed: Some(format!("{:?}", change.state).to_lowercase()),
            remedy: "request Dark and let the resident drive every bounded edge".to_owned(),
        });
    }
    let head = match bus.residency_head() {
        Ok(Some(head)) => head,
        Ok(None) => {
            return typed_store_fault("resident::head-missing", "residency head is absent");
        }
        Err(error) => {
            return typed_store_fault("resident::head-unreadable", &error.to_string());
        }
    };
    if head.body().epoch != epoch || command.epoch() != epoch {
        return typed_protocol_fault(
            "resident::epoch-moved",
            &format!(
                "request {}, resident {}, durable head {}",
                command.epoch(),
                epoch,
                head.body().epoch
            ),
        );
    }
    let from = head.body().state;
    let transition_matches = command.expected().iter().any(|witness| {
        matches!(
            witness,
            Witness::Transition {
                from: witnessed_from,
                to: ResidentState::Dark,
            } if *witnessed_from == from
        )
    });
    if !transition_matches {
        return Outcome::Refusal(Refusal {
            reason: "resident::transition-moved".to_owned(),
            witness: Some("transition".to_owned()),
            expected: Some(format!("{:?}->dark", from).to_lowercase()),
            observed: command.expected().iter().find_map(|witness| match witness {
                Witness::Transition { from, to } => {
                    Some(format!("{from:?}->{to:?}").to_lowercase())
                }
                _ => None,
            }),
            remedy: "read the current resident state and retry the exact transition".to_owned(),
        });
    }
    match drive_shutdown(bus, paths, epoch, seats) {
        Outcome::Reply(reply) => Outcome::Reply(Reply {
            position: reply.position,
            body: CommandAnswer::State(StateAnswer {
                from,
                to: ResidentState::Dark,
            }),
        }),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}


fn write(actor: Actor, epoch: CanonicalUuid, producer: Producer) -> CommandResult<JournalWrite> {
    let origin = format!("resident:{epoch}")
        .parse::<wire::Origin>()
        .map_err(|error| store_fault("resident::origin-invalid", &error.to_string()))?;
    write_at_origin(actor, origin, producer)
}

fn write_at_origin(
    actor: Actor,
    origin: wire::Origin,
    producer: Producer,
) -> CommandResult<JournalWrite> {
    let at = Instant::new(jiff::Timestamp::now())
        .map_err(|error| environment_fault("resident::clock-invalid", &error.to_string()))?;
    Ok(JournalWrite::new(actor, origin, at, producer))
}

fn commit(bus: &Bus, write: JournalWrite) -> CommandResult<JournalCommit> {
    match bus
        .commit(write)
        .map_err(|error| store_fault("bus::commit-failed", &error.to_string()))?
    {
        CommitOutcome::Committed(committed) => Ok(committed),
        CommitOutcome::Refused(refusal) => Err(Outcome::Refusal(Refusal {
            reason: refusal.reason().to_owned(),
            witness: Some("journal-guard".to_owned()),
            expected: None,
            observed: None,
            remedy: "read current resident discovery and retry against its epoch".to_owned(),
        })),
        CommitOutcome::Fault(fault) => Err(store_fault("journal::commit-failed", fault.detail())),
    }
}

fn decode_dark_outcome(outcome: Outcome<CommandAnswer>) -> Outcome<()> {
    match outcome {
        Outcome::Reply(reply) => match reply.body {
            CommandAnswer::State(StateAnswer {
                to: ResidentState::Dark,
                ..
            }) => Outcome::Reply(Reply {
                position: reply.position,
                body: (),
            }),
            answer => protocol_fault(
                "down::answer-shape",
                &format!("expected a state answer ending in dark, received {answer:?}"),
            ),
        },
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

fn server_outcome(served: Result<Result<(), io::Error>, tokio::task::JoinError>) -> Outcome<()> {
    match served {
        Ok(Ok(())) => environment_fault("canvas::stopped", "canvas server stopped early"),
        Ok(Err(error)) => environment_fault("canvas::serve-failed", &error.to_string()),
        Err(error) => environment_fault("canvas::task-failed", &error.to_string()),
    }
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    ignore_absent(fs::remove_file(path))
}

fn remove_tree_if_present(path: &Path) -> io::Result<()> {
    ignore_absent(fs::remove_dir_all(path))
}

fn ignore_absent(result: io::Result<()>) -> io::Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn committed_reply(first: u64, last: u64) -> Outcome<()> {
    Outcome::Reply(Reply {
        position: ReplyPosition::Committed(SeqRange {
            first: seq(first),
            last: seq(last),
        }),
        body: (),
    })
}

fn seq(value: u64) -> Seq {
    Seq::new(value).expect("journal commits always have non-zero positions")
}

fn new_uuid() -> CanonicalUuid {
    CanonicalUuid::new(uuid::Uuid::new_v4())
}

fn protocol_fault(reason: &str, detail: &str) -> Outcome<()> {
    fault(FaultClass::Protocol, reason, detail)
}

fn typed_protocol_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    fault(FaultClass::Protocol, reason, detail)
}

fn store_fault(reason: &str, detail: &str) -> Outcome<()> {
    fault(FaultClass::Store, reason, detail)
}

fn typed_store_fault(reason: &str, detail: &str) -> Outcome<CommandAnswer> {
    fault(FaultClass::Store, reason, detail)
}

fn typed_outcome(outcome: Outcome<()>) -> Outcome<CommandAnswer> {
    match outcome {
        Outcome::Reply(_) => unreachable!("unit command failures never carry replies"),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

fn environment_fault(reason: &str, detail: &str) -> Outcome<()> {
    fault(FaultClass::Environment, reason, detail)
}

fn fault<B>(class: FaultClass, reason: &str, detail: &str) -> Outcome<B> {
    Outcome::Fault(Fault {
        class,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}
