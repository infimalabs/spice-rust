use super::*;

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

pub(super) fn client_room_coordinate(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    mutate_task_from_board(repository, contract, matches, |board, room, heads| {
        let target = required(matches, "ADDRESS")
            .parse::<wire::OccupantAddress>()
            .map_err(|error| refusal("room::address-invalid", None, &error.to_string()))?;
        let lane = board
            .addresses
            .iter()
            .find_map(|(lane, address)| (address.address() == Some(target)).then_some(*lane))
            .ok_or_else(|| {
                refusal(
                    "room::address-unavailable",
                    Some(target.to_string()),
                    "name the current base-52 address of a seated occupant",
                )
            })?;
        let occupant = board
            .residency
            .body
            .lanes
            .iter()
            .find(|row| row.lane == lane)
            .and_then(|row| row.occupant)
            .expect("an addressable residency lane has its proved occupant");
        let action = if contract.path == ["room", "resume"] {
            wire::intent::RoomCoordinateAction::Resume
        } else {
            wire::intent::RoomCoordinateAction::Start
        };
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
