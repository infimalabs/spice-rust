use super::*;

use wire::intent::{
    WORKING_GROUP_PAGE_LIMIT_MAX, WorkingGroupAnchor, WorkingGroupPage, WorkingGroupRead,
};
use wire::rooms::{RoomAuthorityStamp, WorkingGroup, WorkingGroupPut};

const DEFAULT_PAGE_LIMIT: u16 = 25;

pub(super) fn client_group_list(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let after = match optional_group(matches, "after") {
        Ok(after) => after,
        Err(outcome) => return outcome,
    };
    let limit = match numeric_limit(matches) {
        Ok(limit) => limit,
        Err(outcome) => return outcome,
    };
    match execute_group_read(
        repository,
        contract,
        WorkingGroupRead {
            group: None,
            after,
            limit,
        },
    ) {
        Ok((position, page)) => Outcome::Reply(Reply {
            position,
            body: CommandAnswer::Text(TextAnswer {
                text: render_groups(&page),
            }),
        }),
        Err(outcome) => outcome,
    }
}

pub(super) fn client_group_put(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let anchors = match anchors(matches) {
        Ok(anchors) => anchors,
        Err(outcome) => return outcome,
    };
    let group = match optional_group(matches, "group") {
        Ok(Some(group)) => group,
        Ok(None) => match mint_group(repository) {
            Ok(group) => group,
            Err(outcome) => return outcome,
        },
        Err(outcome) => return outcome,
    };
    let observed = match exact_group(repository, &group) {
        Ok(observed) => observed,
        Err(outcome) => return outcome,
    };
    let mut expected = vec![Witness::Group {
        group: group.clone(),
        head: observed.as_ref().map(|view| view.head),
    }];
    expected.extend(
        anchors
            .iter()
            .cloned()
            .map(|task| Witness::Existence { task }),
    );
    let body = WorkingGroupPut {
        group: WorkingGroup {
            group: group.clone(),
            anchors,
        },
    };
    render_change(crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::WorkingGroupPut,
        expected,
        &body,
    ))
}

fn exact_group(
    repository: &Repository,
    group: &RoomAuthorityStamp,
) -> Result<Option<wire::intent::WorkingGroupView>, Outcome<CommandAnswer>> {
    let contract = crate::cli::contract(&["group", "list"])
        .expect("working-group read contract is registered");
    execute_group_read(
        repository,
        contract,
        WorkingGroupRead {
            group: Some(group.clone()),
            after: None,
            limit: 1,
        },
    )
    .map(|(_, page)| page.groups.into_iter().next())
}

fn execute_group_read(
    repository: &Repository,
    contract: &Contract,
    body: WorkingGroupRead,
) -> Result<(ReplyPosition, WorkingGroupPage), Outcome<CommandAnswer>> {
    match crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::WorkingGroupRead,
        Vec::new(),
        &body,
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

fn mint_group(repository: &Repository) -> Result<RoomAuthorityStamp, Outcome<CommandAnswer>> {
    let mut micros = jiff::Timestamp::now().as_microsecond();
    loop {
        if let Ok(group) = RoomAuthorityStamp::for_micros(micros)
            && exact_group(repository, &group)?.is_none()
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

fn optional_group(
    matches: &ArgMatches,
    name: &str,
) -> Result<Option<RoomAuthorityStamp>, Outcome<CommandAnswer>> {
    matches
        .get_one::<String>(name)
        .map(|written| {
            written.parse::<RoomAuthorityStamp>().map_err(|error| {
                refusal(
                    "group::identity-invalid",
                    Some(written.clone()),
                    &error.to_string(),
                )
            })
        })
        .transpose()
}

fn numeric_limit(matches: &ArgMatches) -> Result<u16, Outcome<CommandAnswer>> {
    let Some(written) = matches.get_one::<String>("limit") else {
        return Ok(DEFAULT_PAGE_LIMIT);
    };
    match written.parse::<u16>() {
        Ok(limit) if (1..=WORKING_GROUP_PAGE_LIMIT_MAX).contains(&limit) => Ok(limit),
        _ => Err(refusal(
            "group::limit-invalid",
            Some(written.clone()),
            "use a limit from 1 through 100",
        )),
    }
}

fn anchors(matches: &ArgMatches) -> Result<Vec<TaskStamp>, Outcome<CommandAnswer>> {
    matches
        .get_many::<String>("ANCHOR")
        .into_iter()
        .flatten()
        .map(|written| {
            let handle = written.parse::<TaskHandle>().map_err(|error| {
                refusal(
                    "group::anchor-invalid",
                    Some(written.clone()),
                    &error.to_string(),
                )
            })?;
            handle.born().map_err(|error| {
                refusal(
                    "group::anchor-invalid",
                    Some(written.clone()),
                    &error.to_string(),
                )
            })
        })
        .collect()
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
                    "{} {}",
                    change.group,
                    match change.status {
                        wire::intent::WorkingGroupChangeStatus::Stored => "stored",
                        wire::intent::WorkingGroupChangeStatus::Removed => "ended",
                    }
                ),
            }),
        }),
        other => other,
    }
}

fn render_groups(page: &WorkingGroupPage) -> String {
    let mut text = String::new();
    if let Some(group) = &page.unavailable {
        text.push_str(&format!("unavailable:{group}\n"));
    } else if page.groups.is_empty() {
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
    if let Some(after) = &page.continuation {
        text.push_str(&format!("continue: spice group list --after {after}\n"));
    }
    text
}
