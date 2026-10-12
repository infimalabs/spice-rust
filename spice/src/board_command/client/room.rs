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
    mutate_task(repository, contract, matches, |room, heads| {
        Ok((
            CommandKind::RoomPost,
            vec![statement_witness(heads)],
            serde_json::to_value(RoomPost {
                room,
                text: required(matches, "TEXT"),
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
