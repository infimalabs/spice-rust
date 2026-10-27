use super::*;

use wire::intent::{WorkingGroupAnchor, WorkingGroupPage, WorkingGroupRead};
use wire::rooms::{
    RoomAuthorityStamp, WorkingGroup, WorkingGroupArrange, WorkingGroupDisband, WorkingGroupMove,
};

pub(super) fn dispatch_client(
    repository: Host<'_>,
    contract: &Contract,
    matches: &ArgMatches,
) -> Option<Outcome<CommandAnswer>> {
    Some(match contract.path {
        ["group", "list"] => client_group_list(repository, contract, matches),
        ["group", "move"] => client_group_move(repository, contract, matches),
        ["group", "arrange"] => client_group_arrange(repository, contract, matches),
        ["group", "disband"] => client_group_disband(repository, contract, matches),
        _ => return None,
    })
}

pub(super) fn client_group_list(
    repository: Host<'_>,
    contract: &Contract,
    _matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let (position, page) = match execute_group_read(repository, contract) {
        Ok(read) => read,
        Err(outcome) => return outcome,
    };
    let cycles = match anchor_cycles(repository, &page) {
        Ok(cycles) => cycles,
        Err(outcome) => return outcome,
    };
    Outcome::Reply(Reply {
        position,
        body: CommandAnswer::Text(TextAnswer {
            text: render_groups(&page, &cycles),
        }),
    })
}

/// Each available anchor's pass cycle, judged by one scoped board read.
fn anchor_cycles(
    repository: Host<'_>,
    page: &WorkingGroupPage,
) -> Result<
    std::collections::BTreeMap<TaskStamp, wire::rooms::DesignCyclePresentation>,
    Outcome<CommandAnswer>,
> {
    let tasks = page
        .groups
        .iter()
        .flat_map(|group| &group.anchors)
        .filter_map(|anchor| match anchor {
            WorkingGroupAnchor::Available { born, .. } => Some(born.clone()),
            WorkingGroupAnchor::Unavailable { .. } => None,
        })
        .collect::<Vec<_>>();
    if tasks.is_empty() {
        return Ok(std::collections::BTreeMap::new());
    }
    let board = current_board(
        repository,
        BoardRead::Tasks {
            tasks,
            dependencies: false,
            claims: false,
            history: false,
        },
    )?;
    Ok(board
        .tasks
        .iter()
        .filter_map(|task| {
            Some((
                task.statement.body.born.clone(),
                design_cycle(&board, task)?,
            ))
        })
        .collect())
}

/// Move one design into `--to GROUP`, into a fresh group with `--new`, or out
/// of every group with neither.
pub(super) fn client_group_move(
    repository: Host<'_>,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let Some(written) = matches.get_one::<String>("ANCHOR") else {
        return refusal("group::anchor-missing", None, "name the design to move");
    };
    let design = match anchor(written) {
        Ok(design) => design,
        Err(outcome) => return outcome,
    };
    let to = match matches
        .get_one::<String>("to")
        .map(|written| group(written))
    {
        Some(Ok(to)) => Some(to),
        Some(Err(outcome)) => return outcome,
        None => None,
    };
    let founding = matches.get_flag("new");
    if founding && to.is_some() {
        return refusal(
            "group::destination-ambiguous",
            None,
            "name an existing group with --to or found one with --new, not both",
        );
    }
    let page = match observed(repository) {
        Ok(page) => page,
        Err(outcome) => return outcome,
    };
    let to = if founding {
        match mint_group(&page) {
            Ok(group) => Some(group),
            Err(outcome) => return outcome,
        }
    } else {
        to
    };
    let expected = vec![
        Witness::Group {
            revision: page.revision,
        },
        Witness::Existence {
            task: design.clone(),
        },
    ];
    render_change(crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::WorkingGroupMove,
        expected,
        &WorkingGroupMove { design, to },
    ))
}

/// Restate the whole partition, one `GROUP=ANCHOR[,ANCHOR...]` per group in
/// intended order.
pub(super) fn client_group_arrange(
    repository: Host<'_>,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let groups = match matches
        .get_many::<String>("GROUPING")
        .into_iter()
        .flatten()
        .map(|written| grouping(written))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(groups) => groups,
        Err(outcome) => return outcome,
    };
    let page = match observed(repository) {
        Ok(page) => page,
        Err(outcome) => return outcome,
    };
    render_change(crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::WorkingGroupArrange,
        vec![Witness::Group {
            revision: page.revision,
        }],
        &WorkingGroupArrange { groups },
    ))
}

/// End one standing group at the partition revision observed immediately
/// before the write.
pub(super) fn client_group_disband(
    repository: Host<'_>,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let Some(written_group) = matches.get_one::<String>("GROUP") else {
        return refusal("group::identity-missing", None, "name the working group");
    };
    let group = match group(written_group) {
        Ok(group) => group,
        Err(outcome) => return outcome,
    };
    let page = match observed(repository) {
        Ok(page) => page,
        Err(outcome) => return outcome,
    };
    render_change(crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::WorkingGroupDisband,
        vec![Witness::Group {
            revision: page.revision,
        }],
        &WorkingGroupDisband { group },
    ))
}

/// The current partition a change witnesses.
fn observed(repository: Host<'_>) -> Result<WorkingGroupPage, Outcome<CommandAnswer>> {
    let contract = crate::cli::contract(&["group", "list"])
        .expect("working-group read contract is registered");
    execute_group_read(repository, contract).map(|(_, page)| page)
}

fn execute_group_read(
    repository: Host<'_>,
    contract: &Contract,
) -> Result<(ReplyPosition, WorkingGroupPage), Outcome<CommandAnswer>> {
    match crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::WorkingGroupRead,
        Vec::new(),
        &WorkingGroupRead {},
    ) {
        Outcome::Reply(Reply {
            position,
            body: CommandAnswer::WorkingGroupPage(page),
        }) => Ok((position, page)),
        Outcome::Reply(_) => Err(protocol_fault(
            "group::read-answer",
            "working-group read returned another answer shape",
        )),
        Outcome::Refusal(refusal) => Err(Outcome::Refusal(refusal)),
        Outcome::Fault(fault) => Err(Outcome::Fault(fault)),
    }
}

/// A fresh group identity no current group holds.
fn mint_group(page: &WorkingGroupPage) -> Result<RoomAuthorityStamp, Outcome<CommandAnswer>> {
    let mut micros = jiff::Timestamp::now().as_microsecond();
    loop {
        if let Ok(group) = RoomAuthorityStamp::for_micros(micros)
            && page.groups.iter().all(|held| held.group != group)
        {
            return Ok(group);
        }
        micros = micros.checked_sub(1).ok_or_else(|| {
            environment_fault(
                "group::stamp-clock-invalid",
                "working-group identity allocation exhausted the representable clock",
            )
        })?;
    }
}

fn group(written: &str) -> Result<RoomAuthorityStamp, Outcome<CommandAnswer>> {
    written.parse::<RoomAuthorityStamp>().map_err(|error| {
        refusal(
            "group::identity-invalid",
            Some(written.to_owned()),
            &error.to_string(),
        )
    })
}

fn anchor(written: &str) -> Result<TaskStamp, Outcome<CommandAnswer>> {
    let invalid = |error: &dyn std::fmt::Display| {
        refusal(
            "group::anchor-invalid",
            Some(written.to_owned()),
            &error.to_string(),
        )
    };
    written
        .parse::<TaskHandle>()
        .map_err(|error| invalid(&error))?
        .born()
        .map_err(|error| invalid(&error))
}

fn grouping(written: &str) -> Result<WorkingGroup, Outcome<CommandAnswer>> {
    let Some((stamp, anchors)) = written.split_once('=') else {
        return Err(refusal(
            "group::grouping-invalid",
            Some(written.to_owned()),
            "write each group as GROUP=ANCHOR[,ANCHOR...]",
        ));
    };
    Ok(WorkingGroup {
        group: group(stamp)?,
        anchors: anchors
            .split(',')
            .filter(|anchor| !anchor.is_empty())
            .map(anchor)
            .collect::<Result<_, _>>()?,
    })
}

fn render_change(outcome: Outcome<CommandAnswer>) -> Outcome<CommandAnswer> {
    match outcome {
        Outcome::Reply(Reply {
            position,
            body: CommandAnswer::WorkingGroupChange(change),
        }) => {
            let text = format!(
                "revision {} {}",
                wire::SequenceCode::new(change.revision),
                match change.status {
                    wire::intent::WorkingGroupChangeStatus::Moved => "moved",
                    wire::intent::WorkingGroupChangeStatus::Ended => "moved; its group ended",
                    wire::intent::WorkingGroupChangeStatus::Arranged => "arranged",
                    wire::intent::WorkingGroupChangeStatus::Disbanded => "disbanded",
                    wire::intent::WorkingGroupChangeStatus::Retuned => "fast mode set",
                }
            );
            Outcome::Reply(Reply {
                position,
                body: CommandAnswer::Text(TextAnswer { text }),
            })
        }
        other => other,
    }
}

fn render_groups(
    page: &WorkingGroupPage,
    cycles: &std::collections::BTreeMap<TaskStamp, wire::rooms::DesignCyclePresentation>,
) -> String {
    let mut text = format!("revision {}\n", wire::SequenceCode::new(page.revision));
    if page.groups.is_empty() {
        text.push_str("no working groups\n");
    }
    for group in &page.groups {
        text.push_str(&format!("{}\n", group.group));
        for anchor in &group.anchors {
            match anchor {
                WorkingGroupAnchor::Available {
                    born,
                    handle,
                    design,
                } => {
                    text.push_str(&format!("  {handle} {design}"));
                    if let Some(cycle) = cycles.get(born) {
                        text.push_str(&format!(
                            ": {}; next ({}): {}",
                            cycle.cycle.label(),
                            cycle.actor.label(),
                            cycle.action
                        ));
                    }
                    text.push('\n');
                }
                WorkingGroupAnchor::Unavailable { born } => {
                    text.push_str(&format!("  unavailable:{born}\n"));
                }
            }
        }
    }
    text
}
