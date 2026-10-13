use super::*;

use wire::intent::{WorkingGroupAnchor, WorkingGroupPage, WorkingGroupRead};
use wire::rooms::{RoomAuthorityStamp, WorkingGroup, WorkingGroupArrange, WorkingGroupMove};

pub(super) fn client_group_list(
    repository: &Repository,
    contract: &Contract,
    _matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    match execute_group_read(repository, contract) {
        Ok((position, page)) => Outcome::Reply(Reply {
            position,
            body: CommandAnswer::Text(TextAnswer {
                text: render_groups(&page),
            }),
        }),
        Err(outcome) => outcome,
    }
}

/// Move one design into `--to GROUP`, into a fresh group with `--new`, or out
/// of every group with neither.
pub(super) fn client_group_move(
    repository: &Repository,
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
    repository: &Repository,
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

/// The current partition a change witnesses.
fn observed(repository: &Repository) -> Result<WorkingGroupPage, Outcome<CommandAnswer>> {
    let contract = crate::cli::contract(&["group", "list"])
        .expect("working-group read contract is registered");
    execute_group_read(repository, contract).map(|(_, page)| page)
}

fn execute_group_read(
    repository: &Repository,
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
        }) => Outcome::Reply(Reply {
            position,
            body: CommandAnswer::Text(TextAnswer {
                text: format!(
                    "revision {} {}",
                    change.revision,
                    match change.status {
                        wire::intent::WorkingGroupChangeStatus::Moved => "moved",
                        wire::intent::WorkingGroupChangeStatus::Ended => "moved; its group ended",
                        wire::intent::WorkingGroupChangeStatus::Arranged => "arranged",
                    }
                ),
            }),
        }),
        other => other,
    }
}

fn render_groups(page: &WorkingGroupPage) -> String {
    let mut text = format!("revision {}\n", page.revision);
    if page.groups.is_empty() {
        text.push_str("no working groups\n");
    }
    for group in &page.groups {
        text.push_str(&format!("{}\n", group.group));
        for anchor in &group.anchors {
            match anchor {
                WorkingGroupAnchor::Available { handle, .. } => {
                    text.push_str(&format!("  {handle}\n"));
                }
                WorkingGroupAnchor::Unavailable { born } => {
                    text.push_str(&format!("  unavailable:{born}\n"));
                }
            }
        }
    }
    text
}
