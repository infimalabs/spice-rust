use super::*;
use wire::rooms::{RoomAuthorityStamp, RoomCharge, RoomContributionKind, RoomQuestion, RoomSource};

pub(super) fn dispatch_room_client(
    repository: Host<'_>,
    contract: &Contract,
    matches: &ArgMatches,
) -> Option<Outcome<CommandAnswer>> {
    let outcome = match contract.path {
        ["room", "post"] => client_room_post(repository, contract, matches),
        ["room", "leave"] => client_room_leave(repository, contract, matches),
        ["room", "contribute"] => client_room_contribute(repository, contract, matches),
        ["room", "ack" | "nak" | "reply" | "recover" | "assess"] => {
            client_room_control(repository, contract, matches)
        }
        _ => return None,
    };
    Some(outcome)
}

pub(super) fn client_room_post(
    repository: Host<'_>,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    mutate_task_from_board(repository, contract, matches, |_board, room, heads| {
        let text = matches
            .get_one::<String>("TEXT")
            .cloned()
            .unwrap_or_default();
        Ok((
            CommandKind::RoomPost,
            vec![statement_witness(heads)],
            serde_json::to_value(RoomPost { room, text }).expect("typed room post encodes"),
        ))
    })
}

/// Contribute one charge or question to an open Design's room as the
/// connected lane, witnessing the Design's statement and workflow as the board
/// reads and resolving each `--cite` key to the exact source it names.
fn client_room_contribute(
    repository: Host<'_>,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    mutate_task_from_board(repository, contract, matches, |board, room, heads| {
        let kind = match (matches.get_one::<String>("charge"), matches.get_flag("ask")) {
            (Some(written), _) => written
                .parse::<RoomCharge>()
                .map(RoomContributionKind::Charge)
                .map_err(|error| {
                    refusal(
                        "room::contribution-charge-invalid",
                        Some(written.clone()),
                        &error.to_string(),
                    )
                })?,
            (None, true) => RoomContributionKind::Question,
            (None, false) => {
                return Err(refusal(
                    "room::contribution-kind-missing",
                    None,
                    "weigh one question item with --charge=C --question N, or pose an open \
                     question with --ask",
                ));
            }
        };
        let question = matches
            .get_one::<String>("question")
            .map(|written| {
                written
                    .parse::<u16>()
                    .ok()
                    .and_then(RoomQuestion::new)
                    .ok_or_else(|| {
                        refusal(
                            "room::contribution-question-invalid",
                            Some(written.clone()),
                            "name a question item by its one-based position in the Design's \
                             acceptance",
                        )
                    })
            })
            .transpose()?;
        let anchor = &heads.statement.body.born;
        let sources = matches
            .get_many::<String>("cite")
            .into_iter()
            .flatten()
            .map(|key| cite(board, anchor, key))
            .collect::<Result<Vec<_>, _>>()?;
        let mut expected = vec![statement_witness(heads), workflow_witness(heads)];
        expected.extend(sources.iter().map(|source| wire::Witness::Source {
            source: source.authority_digest(),
        }));
        Ok((
            CommandKind::RoomContribute,
            expected,
            serde_json::to_value(wire::intent::RoomContribute {
                room,
                kind,
                question,
                text: required(matches, "TEXT"),
                sources,
            })
            .expect("typed room contribution encodes"),
        ))
    })
}

/// The exact source one `--cite` key names, resolved by its form the way
/// `spice://` resolves it: a task handle or stamp cites that task's statement
/// head fact, a hex digest an artifact, a publication stamp a contribution in
/// this room, and a base-52 journal code that fact. The resident still
/// refuses any source that does not exist before the contribution.
pub(in crate::board_command) fn cite(
    board: &BoardAnswer,
    room: &wire::TaskStamp,
    key: &str,
) -> Result<RoomSource, Outcome<CommandAnswer>> {
    let statement = |task: &BoardTask| RoomSource::Journal {
        fact: wire::SequenceCode::new(task.statement.position.get()),
    };
    if let Ok(handle) = key.parse::<TaskHandle>() {
        return task_heads(board, &handle).map(statement).map_err(|_| {
            refusal(
                "room::contribution-cite-unknown",
                Some(key.to_owned()),
                "cite a task by a handle the board shows",
            )
        });
    }
    if let Ok(digest) = key.parse::<wire::Digest>() {
        return Ok(RoomSource::Artifact { digest });
    }
    if let Ok(stamp) = key.parse::<wire::TaskStamp>() {
        if let Some(task) = board
            .tasks
            .iter()
            .find(|task| task.statement.body.born == stamp)
        {
            return Ok(statement(task));
        }
        if let Ok(publication) = key.parse::<RoomAuthorityStamp>() {
            return Ok(RoomSource::Contribution {
                room: room.clone(),
                publication,
            });
        }
    }
    key.parse::<wire::SequenceCode>()
        .map(|fact| RoomSource::Journal { fact })
        .map_err(|_| {
            refusal(
                "room::contribution-cite-invalid",
                Some(key.to_owned()),
                "cite a task handle or stamp, a contribution's publication stamp, a journal \
                 code, or an artifact digest",
            )
        })
}

/// Leave one room as the connected lane and say what still keeps it there.
fn client_room_leave(
    repository: Host<'_>,
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
    let mut lines = vec![format!(
        "left {} publication {}",
        answer.room, answer.publication
    )];
    if answer.kept.is_empty() {
        lines.push(
            "no longer listening; a later room message or Design request restores listening"
                .to_owned(),
        );
        return lines.join("\n");
    }
    lines.push("still listening until these native duties end; leaving ends none:".to_owned());
    for duty in &answer.kept {
        lines.push(match duty {
            Duty::Work { task } => {
                format!("  work {task}: active claim on the anchor or related work")
            }
            Duty::Stop { key } => format!("  stop {key}: live stop request for this lane's claim"),
        });
    }
    lines.join("\n")
}

/// Submit one operator ACK, NAK, reply, recovery, or assessment.
fn client_room_control(
    repository: Host<'_>,
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
        ["room", "reply"] => {
            return Ok(RoomControl::Reply {
                item: item_ref(&required(matches, "ITEM"))?,
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

/// One line per moved item, followed by any reply publication and return key.
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
    if let Some(publication) = &answer.publication {
        lines.push(format!("publication {publication}"));
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
