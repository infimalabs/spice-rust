use super::*;
use wire::rooms::RoomAuthorityStamp;

pub(super) fn dispatch_room_client(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Option<Outcome<CommandAnswer>> {
    let outcome = match contract.path {
        ["room", "post"] => client_room_post(repository, contract, matches),
        ["room", "status"] => client_room_status(repository, contract, matches),
        ["room", "start" | "resume"] => client_room_coordinate(repository, contract, matches),
        ["room", "put-aside"] => client_room_put_aside(repository, contract, matches),
        ["room", "leave"] => client_room_leave(repository, contract, matches),
        ["room", "ack" | "nak" | "answer" | "recover" | "assess"] => {
            client_room_control(repository, contract, matches)
        }
        ["room", "decline" | "cancel"] => client_room_close(repository, contract, matches),
        _ => return None,
    };
    Some(outcome)
}

pub(super) fn client_room_post(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    mutate_task_from_board(repository, contract, matches, |board, room, heads| {
        let text = matches
            .get_one::<String>("TEXT")
            .cloned()
            .unwrap_or_default();
        let disposition = matches.get_one::<String>("disposition");
        let reports = matches
            .get_many::<String>("report")
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let grooming_requested = disposition.is_some()
            || !reports.is_empty()
            || matches.get_one::<String>("cause").is_some()
            || matches.get_one::<String>("exceptions").is_some()
            || matches.get_one::<String>("uncertainty").is_some();
        let mut expected = vec![statement_witness(heads)];
        let grooming = if grooming_requested {
            if !text.is_empty() {
                return Err(refusal(
                    "room::post-shape",
                    Some(room.to_string()),
                    "supply either authored TEXT or an OOPS disposition, never both",
                ));
            }
            let disposition = disposition
                .ok_or_else(|| {
                    refusal(
                        "room::oops-disposition-missing",
                        Some(room.to_string()),
                        "name --disposition for the selected OOPS reports",
                    )
                })?
                .parse()
                .map_err(|error: wire::rooms::OopsDispositionError| {
                    refusal(
                        "room::oops-disposition-invalid",
                        Some(room.to_string()),
                        &error.to_string(),
                    )
                })?;
            if reports.is_empty() {
                return Err(refusal(
                    "room::oops-reports-missing",
                    Some(room.to_string()),
                    "name at least one parked report with --report",
                ));
            }
            let mut stamps = Vec::with_capacity(reports.len());
            for named in reports {
                let handle = named.parse::<TaskHandle>().map_err(|error| {
                    refusal(
                        "room::oops-report-invalid",
                        Some(named.clone()),
                        &error.to_string(),
                    )
                })?;
                let report = named_task_heads(board, &handle)?;
                if stamps.contains(&report.statement.body.born) {
                    return Err(refusal(
                        "room::oops-report-duplicate",
                        Some(report.statement.body.handle.to_string()),
                        "name each source report once",
                    ));
                }
                stamps.push(report.statement.body.born.clone());
                expected.push(statement_witness(report));
            }
            let cause = matches
                .get_one::<String>("cause")
                .map(|cause| cause.trim())
                .filter(|cause| !cause.is_empty())
                .ok_or_else(|| {
                    refusal(
                        "room::oops-cause-missing",
                        Some(room.to_string()),
                        "state the evidenced common cause or negative basis with --cause",
                    )
                })?
                .to_owned();
            Some(OopsGrooming {
                reports: stamps,
                disposition,
                cause,
                exceptions: matches
                    .get_one::<String>("exceptions")
                    .map_or_else(String::new, |value| value.trim().to_owned()),
                uncertainty: matches
                    .get_one::<String>("uncertainty")
                    .map_or_else(String::new, |value| value.trim().to_owned()),
            })
        } else {
            None
        };
        Ok((
            CommandKind::RoomPost,
            expected,
            serde_json::to_value(RoomPost {
                room,
                text,
                grooming,
            })
            .expect("typed room post encodes"),
        ))
    })
}

pub(super) fn client_room_status(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let room = match required(matches, "HANDLE").parse::<TaskHandle>() {
        Ok(room) => room,
        Err(error) => return refusal("room::anchor-invalid", None, &error.to_string()),
    };
    crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::RoomCoordinationRead,
        Vec::new(),
        &wire::intent::RoomCoordinationRead { room },
    )
}

pub(in crate::board_command) fn coordinate_target(
    board: &BoardAnswer,
    target: wire::OccupantAddress,
) -> Result<(wire::CanonicalUuid, wire::CanonicalUuid), Outcome<CommandAnswer>> {
    let unavailable = || {
        refusal(
            "room::address-unavailable",
            Some(target.to_string()),
            "name the current base-52 address of a seated occupant",
        )
    };
    let lane = board
        .addresses
        .iter()
        .find_map(|(lane, address)| (address.address() == Some(target)).then_some(*lane))
        .ok_or_else(unavailable)?;
    let seat = board
        .residency
        .body
        .lanes
        .iter()
        .find(|row| row.lane == lane)
        .ok_or_else(unavailable)?;
    if seat.disposition != "occupied" {
        return Err(refusal(
            "room::address-unoccupied",
            Some(format!("{target}: {}", seat.disposition)),
            &format!(
                "address {target} names a seat whose disposition is {}; name the current address of an occupied seat",
                seat.disposition
            ),
        ));
    }
    let occupant = seat.occupant.ok_or_else(unavailable)?;
    Ok((lane, occupant))
}

pub(super) fn client_room_coordinate(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    mutate_task_from_board(repository, contract, matches, |board, room, heads| {
        let target = required(matches, "ADDRESS")
            .parse::<wire::OccupantAddress>()
            .map_err(|error| refusal("room::address-invalid", None, &error.to_string()))?;
        let (lane, occupant) = coordinate_target(board, target)?;
        let action = if contract.path == ["room", "resume"] {
            wire::intent::RoomCoordinateAction::Resume
        } else {
            wire::intent::RoomCoordinateAction::Start
        };
        let auto_resume = matches
            .get_one::<String>("auto-resume")
            .map(|written| {
                written.parse::<u8>().map_err(|_| {
                    refusal(
                        "room::auto-resume-out-of-range",
                        Some(written.clone()),
                        "authorize from 1 through 8 automated resumes",
                    )
                })
            })
            .transpose()?;
        let request = wire::intent::RoomCoordinate {
            room,
            target,
            action,
            criterion: matches
                .get_one::<String>("criterion")
                .cloned()
                .unwrap_or_default(),
            contribution: matches
                .get_one::<String>("contribution")
                .cloned()
                .unwrap_or_default(),
            stop: matches
                .get_one::<String>("stop")
                .cloned()
                .unwrap_or_default(),
            auto_resume,
        };
        Ok((
            CommandKind::RoomCoordinate,
            vec![
                statement_witness(heads),
                workflow_witness(heads),
                claim_witness(heads),
                residency_witness(board, lane, occupant),
            ],
            serde_json::to_value(request).expect("typed room coordination encodes"),
        ))
    })
}

pub(super) fn client_room_put_aside(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    mutate_task_from_board(repository, contract, matches, |_board, room, heads| {
        Ok((
            CommandKind::RoomPutAside,
            vec![
                statement_witness(heads),
                workflow_witness(heads),
                claim_witness(heads),
            ],
            serde_json::to_value(wire::intent::RoomPutAside { room })
                .expect("typed room put-aside encodes"),
        ))
    })
}

/// Leave one room as the connected lane and say what still keeps it there.
fn client_room_leave(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let written = required(matches, "HANDLE");
    let room = match written.parse::<TaskHandle>() {
        Ok(room) => room,
        Err(error) => {
            return refusal(
                "room::anchor-invalid",
                Some(written),
                &format!("name the room's anchor task handle: {error}"),
            );
        }
    };
    let outcome = crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::RoomLeave,
        Vec::new(),
        &wire::intent::RoomLeave {
            room,
            clause: matches
                .get_one::<String>("clause")
                .cloned()
                .unwrap_or_default(),
        },
    );
    let Outcome::Reply(Reply {
        position,
        body: CommandAnswer::RoomLeave(answer),
    }) = outcome
    else {
        return outcome;
    };
    Outcome::Reply(Reply {
        position,
        body: CommandAnswer::Text(TextAnswer {
            text: render_leave(&answer),
        }),
    })
}

fn render_leave(answer: &wire::intent::RoomLeaveAnswer) -> String {
    use wire::intent::RoomListeningDuty as Duty;
    let mut lines = vec![if answer.recorded {
        format!("left {} publication {}", answer.room, answer.publication)
    } else {
        format!(
            "already left {} publication {}",
            answer.room, answer.publication
        )
    }];
    if answer.kept.is_empty() {
        lines.push(
            "no longer listening; a contribution or a new invitation restores listening".to_owned(),
        );
        return lines.join("\n");
    }
    lines.push("still listening until these native duties end; leaving ends none:".to_owned());
    for duty in &answer.kept {
        lines.push(match duty {
            Duty::Work { task } => {
                format!("  work {task}: active claim on the anchor or related work")
            }
            Duty::Coordination { key } => {
                format!("  coordination {key}: live room coordination request")
            }
            Duty::Stop { key } => format!("  stop {key}: live stop request for this lane's claim"),
            Duty::Receipt { key } => format!("  receipt {key}: owed ACK or NAK of a room delivery"),
            Duty::Result { key } => format!("  result {key}: owed answer to a room ASK"),
        });
    }
    lines.join("\n")
}

/// Submit one operator ACK, NAK, answer, or recovery.
fn client_room_control(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let control = match room_control(contract.path, matches) {
        Ok(control) => control,
        Err(outcome) => return outcome,
    };
    render_control(crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::RoomControl,
        Vec::new(),
        &control,
    ))
}

fn room_control(
    path: &[&str],
    matches: &ArgMatches,
) -> Result<wire::intent::RoomControl, Outcome<CommandAnswer>> {
    use wire::intent::RoomControl;
    use wire::rooms::RoomReceiptDisposition;
    let disposition = match path {
        ["room", "ack"] => RoomReceiptDisposition::Ack,
        ["room", "nak"] => RoomReceiptDisposition::Nak,
        ["room", "answer"] => {
            let context = matches
                .get_one::<String>("context")
                .map(|written| {
                    written.parse::<TaskHandle>().map_err(|error| {
                        refusal(
                            "room::context-invalid",
                            Some(written.clone()),
                            &error.to_string(),
                        )
                    })
                })
                .transpose()?;
            return Ok(RoomControl::Answer {
                result: result_ref(&required(matches, "RESULT"))?,
                context,
                text: required(matches, "TEXT"),
            });
        }
        ["room", "assess"] => {
            let observation = required(matches, "OBSERVATION");
            let observation = observation.parse::<RoomAuthorityStamp>().map_err(|error| {
                refusal(
                    "room::observation-invalid",
                    Some(observation.clone()),
                    &format!(
                        "name an observation as `spice room relevance HANDLE` lists it: {error}"
                    ),
                )
            })?;
            let verdict = required(matches, "VERDICT");
            let verdict = verdict
                .parse::<wire::rooms::RoomRelevanceVerdict>()
                .map_err(|error| refusal("room::verdict-invalid", Some(verdict.clone()), error))?;
            return Ok(RoomControl::Assess {
                observation,
                verdict,
                explanation: required(matches, "TEXT"),
            });
        }
        _ => {
            return Ok(RoomControl::Recover {
                item: item_ref(&required(matches, "ITEM"))?,
            });
        }
    };
    let keys = matches
        .get_many::<String>("ITEM")
        .expect("generated syntax requires items")
        .map(|written| item_ref(written))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(RoomControl::Dispose {
        disposition,
        keys,
        clause: matches
            .get_one::<String>("clause")
            .cloned()
            .unwrap_or_default(),
    })
}

/// Submit one decline or cancellation as the connected party, or as the
/// operator overriding a lane party.
fn client_room_close(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let result = match result_ref(&required(matches, "RESULT")) {
        Ok(result) => result,
        Err(outcome) => return outcome,
    };
    let closure = match contract.path {
        ["room", "decline"] => wire::rooms::QuestionClosure::Declined,
        _ => wire::rooms::QuestionClosure::Cancelled,
    };
    render_control(crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::RoomClose,
        Vec::new(),
        &wire::intent::RoomClose {
            result,
            closure,
            clause: matches
                .get_one::<String>("clause")
                .cloned()
                .unwrap_or_default(),
            override_party: matches.get_flag("override"),
        },
    ))
}

fn item_ref(written: &str) -> Result<wire::rooms::RoomReceiptRef, Outcome<CommandAnswer>> {
    written
        .parse::<RoomAuthorityStamp>()
        .map(wire::rooms::RoomReceiptRef::new)
        .map_err(|error| {
            refusal(
                "room::item-invalid",
                Some(written.to_owned()),
                &format!("name an operator item key as `spice room inbox` lists it: {error}"),
            )
        })
}

fn result_ref(written: &str) -> Result<wire::rooms::QuestionResultRef, Outcome<CommandAnswer>> {
    written
        .parse::<RoomAuthorityStamp>()
        .map(wire::rooms::QuestionResultRef::new)
        .map_err(|error| {
            refusal(
                "room::result-invalid",
                Some(written.to_owned()),
                &format!("name a question result handle as its ASK minted it: {error}"),
            )
        })
}

/// One line per moved item and result, then the answer and its return key.
fn render_control(outcome: Outcome<CommandAnswer>) -> Outcome<CommandAnswer> {
    let Outcome::Reply(Reply {
        position,
        body: CommandAnswer::RoomControl(answer),
    }) = outcome
    else {
        return outcome;
    };
    let mut lines = Vec::new();
    for moved in &answer.items {
        lines.push(format!("item {} {}", moved.item, moved.state));
    }
    for moved in &answer.results {
        lines.push(format!("result {} {}", moved.result, moved.state));
    }
    if let Some(publication) = &answer.answer {
        lines.push(format!("answer {publication}"));
    }
    if let Some(key) = &answer.returned {
        lines.push(format!("returned {key}"));
    }
    if let Some((observation, verdict)) = &answer.assessed {
        lines.push(format!(
            "observation {observation} assessed {}",
            verdict.name()
        ));
    }
    Outcome::Reply(Reply {
        position,
        body: CommandAnswer::Text(TextAnswer {
            text: lines.join("\n"),
        }),
    })
}
