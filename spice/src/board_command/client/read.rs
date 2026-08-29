use super::*;

pub(super) fn client_status(
    repository: &Repository,
    contract: &Contract,
) -> Outcome<CommandAnswer> {
    match crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::BoardRead,
        Vec::new(),
        &BoardRead { family: None },
    ) {
        Outcome::Reply(Reply {
            position,
            body: CommandAnswer::Board(board),
        }) => match task_status(&board) {
            Ok(text) => Outcome::Reply(Reply {
                position,
                body: CommandAnswer::Text(TextAnswer { text }),
            }),
            Err(outcome) => outcome,
        },
        Outcome::Reply(_) => protocol_fault(
            "task::status-answer",
            "task status board read returned another answer shape",
        ),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

pub(super) fn client_list(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let options = match TaskListOptions::from_matches(matches) {
        Ok(options) => options,
        Err(outcome) => return outcome,
    };
    match crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::BoardRead,
        Vec::new(),
        &BoardRead { family: None },
    ) {
        Outcome::Reply(Reply {
            position,
            body: CommandAnswer::Board(board),
        }) => Outcome::Reply(Reply {
            position,
            body: CommandAnswer::Text(TextAnswer {
                text: task_list(&board, &options),
            }),
        }),
        Outcome::Reply(_) => protocol_fault(
            "task::list-answer",
            "task list board read returned another answer shape",
        ),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

pub(super) fn client_show(
    repository: &Repository,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let task = match required(matches, "HANDLE").parse::<TaskHandle>() {
        Ok(task) => task,
        Err(error) => {
            return refusal(
                "task::show-handle",
                matches.get_one::<String>("HANDLE").cloned(),
                &error.to_string(),
            );
        }
    };
    match crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::BoardRead,
        Vec::new(),
        &BoardRead { family: None },
    ) {
        Outcome::Reply(Reply {
            position,
            body: CommandAnswer::Board(board),
        }) => {
            let task = match task_heads(&board, &task) {
                Ok(task) => task,
                Err(outcome) => return outcome,
            };
            let artifacts = match task_show_artifacts(repository, task) {
                Ok(artifacts) => artifacts,
                Err(outcome) => return outcome,
            };
            Outcome::Reply(Reply {
                position,
                body: CommandAnswer::Text(TextAnswer {
                    text: task_show(&board, task, &artifacts),
                }),
            })
        }
        Outcome::Reply(_) => protocol_fault(
            "task::show-answer",
            "task show board read returned another answer shape",
        ),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

#[derive(Clone, Copy)]
pub(in crate::board_command) enum TaskListStatus {
    Pending,
    Waiting,
    Completed,
    Deleted,
}

pub(in crate::board_command) struct TaskListOptions {
    pub(in crate::board_command) all: bool,
    pub(in crate::board_command) project: Option<String>,
    pub(in crate::board_command) limit: Option<usize>,
    pub(in crate::board_command) status: Option<TaskListStatus>,
}

impl TaskListOptions {
    fn from_matches(matches: &ArgMatches) -> Result<Self, Outcome<CommandAnswer>> {
        let all = matches.get_flag("all");
        let project = matches.get_one::<String>("project").map(|written| {
            written
                .trim()
                .strip_prefix("project:")
                .unwrap_or(written.trim())
                .to_owned()
        });
        if all && project.is_some() {
            return Err(refusal(
                "task::list-scope",
                None,
                "choose either --all or --project PROJECT",
            ));
        }
        if project.as_ref().is_some_and(String::is_empty) {
            return Err(refusal(
                "task::list-project",
                matches.get_one::<String>("project").cloned(),
                "name a non-empty project after --project",
            ));
        }
        let limit = matches
            .get_one::<String>("limit")
            .map(|written| {
                written.parse::<usize>().map_err(|_| {
                    refusal(
                        "task::list-limit",
                        Some(written.clone()),
                        "name a positive integer after --limit",
                    )
                })
            })
            .transpose()?;
        if limit == Some(0) {
            return Err(refusal(
                "task::list-limit",
                Some("0".to_owned()),
                "name a positive integer after --limit",
            ));
        }
        let status = matches
            .get_one::<String>("status")
            .map(|status| match status.as_str() {
                "pending" => TaskListStatus::Pending,
                "waiting" => TaskListStatus::Waiting,
                "completed" => TaskListStatus::Completed,
                "deleted" => TaskListStatus::Deleted,
                _ => unreachable!("contract parser owns task list status values"),
            });
        Ok(Self {
            all,
            project,
            limit,
            status,
        })
    }
}

pub(in crate::board_command) fn task_list(
    board: &BoardAnswer,
    options: &TaskListOptions,
) -> String {
    let mut rows = board
        .tasks
        .iter()
        .filter(|task| list_scope_matches(board, task, options))
        .filter(|task| list_status_matches(task, options))
        .collect::<Vec<_>>();
    let truncation = if let Some(limit) = options.limit {
        rows.sort_by(|left, right| {
            task_newest_position(right)
                .cmp(&task_newest_position(left))
                .then_with(|| right.statement.body.handle.cmp(&left.statement.body.handle))
        });
        let original = rows.len();
        rows.truncate(limit);
        let withheld = original - rows.len();
        (withheld > 0).then_some((limit, original, withheld))
    } else {
        None
    };

    let (scope, empty) = if options.all {
        (
            "scope global --all".to_owned(),
            "no tasks in global scope".to_owned(),
        )
    } else if let Some(project) = &options.project {
        (
            format!("scope explicit-project project:{project}"),
            "no tasks in scope; use --all for global rows or --project PROJECT for one project"
                .to_owned(),
        )
    } else {
        (
            format!("scope actor-route filter {}", actor_route_filter(board)),
            "no tasks in scope; use --all for global rows or --project PROJECT for one project"
                .to_owned(),
        )
    };
    if rows.is_empty() {
        return format!("{scope}\n{empty}");
    }
    let mut output = scope;
    if let Some((limit, original, withheld)) = truncation {
        output.push('\n');
        output.push_str(&format!(
            "limited --limit {limit} original {original} withheld {withheld}"
        ));
    }
    for task in rows {
        output.push('\n');
        output.push_str(&render_task_list_row(task));
    }
    output
}

fn list_scope_matches(board: &BoardAnswer, task: &BoardTask, options: &TaskListOptions) -> bool {
    if let Some(project) = &options.project {
        let candidate = task.statement.body.project.as_str();
        return candidate == project
            || candidate
                .strip_prefix(project)
                .is_some_and(|suffix| suffix.starts_with('.'));
    }
    options.all || actor_route_matches(board, task)
}

/// Whether the actor may *read* the row.
///
/// Stem scoped, not subscription scoped, and deliberately wider than the
/// allocation scope in `board::next::selectable`, which states the rule and
/// why the two must stay apart.
fn actor_route_matches(board: &BoardAnswer, task: &BoardTask) -> bool {
    let statement = &task.statement.body;
    statement.project == board.actor_route.project
        || task.origin_thread == board.actor_route.origin_thread
        || (!task.visibility.is_hidden()
            && board.project_policy.public_stems.iter().any(|stem| {
                statement.project == *stem
                    || statement
                        .project
                        .strip_prefix(stem)
                        .is_some_and(|suffix| suffix.starts_with('.'))
            }))
}

fn list_status_matches(task: &BoardTask, options: &TaskListOptions) -> bool {
    let pending = task.workflow.body.status == "pending"
        && task.workflow.body.completion.is_none()
        && task.workflow.body.deferral.is_none();
    let waiting = task.workflow.body.status == "pending"
        && task.workflow.body.completion.is_none()
        && task.workflow.body.deferral.is_some();
    match options.status {
        Some(TaskListStatus::Pending) => pending,
        Some(TaskListStatus::Waiting) => waiting,
        Some(TaskListStatus::Completed) => task.workflow.body.status == "completed",
        // A withdrawal leaves the row readable but off the working board, so the
        // only way to reach one is to ask for it by name or to ask for all.
        Some(TaskListStatus::Deleted) => task.workflow.body.status == "deleted",
        None if options.all => true,
        None => {
            pending
                || waiting
                    && task
                        .claim
                        .as_ref()
                        .is_some_and(|claim| claim.body.disposition == ClaimDisposition::Active)
        }
    }
}

fn task_newest_position(task: &BoardTask) -> Seq {
    let mut newest = task.statement.position.max(task.workflow.position);
    if let Some(claim) = &task.claim {
        newest = newest.max(claim.position);
    }
    if let Some(lease) = &task.lease {
        newest = newest.max(lease.position);
    }
    newest
}

fn render_task_list_row(task: &BoardTask) -> String {
    // A terminal row shows how it ended, not the phase it stopped in. Falling
    // through to the phase would print a withdrawn task as `[todo]`, which is
    // the one thing it is not, and the predecessor prints `[deleted]` here.
    let state = match task.workflow.body.status.as_str() {
        "completed" => "done",
        "deleted" => "deleted",
        _ => task.workflow.body.phase.as_str(),
    };
    let priority = task
        .statement
        .body
        .tags
        .iter()
        .find_map(|tag| tag.strip_prefix(board::PRIORITY_TAG_PREFIX))
        // A spelling the allocator does not score is rendered as written. It
        // ranks as if no priority were declared, and printing the word rather
        // than a letter is how a reader sees which task carries it.
        .map(|priority| {
            board::priority_from_wire(priority).map_or_else(
                || priority.to_owned(),
                |kind| board::priority_label(kind).to_owned(),
            )
        });
    let priority = priority.map_or_else(String::new, |value| format!(" P:{value}"));
    format!(
        "{} [{state}]{priority} {} {}",
        task.statement.body.handle,
        task.statement.body.project,
        task.presentation_title()
    )
}

fn task_show_artifacts(
    repository: &Repository,
    task: &BoardTask,
) -> Result<Vec<artifact::Artifact>, Outcome<CommandAnswer>> {
    let state = repository
        .admit_state()
        .map_err(|error| repository_state_failure(&error))?;
    let store = artifact::Store::new(state.artifacts(), state.locks().join("task-artifacts.lock"));
    store
        .list(&task.statement.body.born)
        .map_err(artifact_operation_failure)
}

pub(in crate::board_command) fn task_show(
    board: &BoardAnswer,
    task: &BoardTask,
    artifacts: &[artifact::Artifact],
) -> String {
    let statement = &task.statement.body;
    let workflow = &task.workflow.body;
    let mut lines = vec![
        format!("handle {}", statement.handle),
        format!("title {}", task.presentation_title()),
        format!(
            "description {}",
            statement.description.as_deref().unwrap_or("-")
        ),
        format!("origin {}", statement.provenance),
        format!("project {}", statement.project),
        format!("phase {}", workflow.phase),
        format!("review {}", workflow.review),
        format!("status {}", workflow.status),
        format!("version {}", statement.revision),
        format!("claim {}", claim_line(task)),
        format!("claim_thread {}", claim_thread(task)),
        format!("creator_context {}", task.origin_thread),
    ];
    if statement.tags.is_empty() {
        lines.push("tags -".to_owned());
    } else {
        lines.push(format!(
            "tags {}",
            statement
                .tags
                .iter()
                .map(|tag| format!("+{tag}"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    lines.push("acceptance:".to_owned());
    if statement.acceptance.is_empty() {
        lines.push("  -".to_owned());
    } else {
        for item in &statement.acceptance {
            push_bulleted(&mut lines, item);
        }
    }
    if !statement.dependencies.is_empty() {
        lines.push("depends:".to_owned());
        for dependency in &statement.dependencies {
            lines.push(render_dependency(board, dependency));
        }
    }
    lines.push("artifacts:".to_owned());
    if artifacts.is_empty() {
        lines.push("  -".to_owned());
    } else {
        for artifact in artifacts {
            lines.push(format!(
                "  {} {} {} {} B {}",
                artifact.id, artifact.name, artifact.media_type, artifact.bytes, artifact.retention
            ));
            lines.push(format!(
                "     spice task artifact show {} {}",
                statement.handle, artifact.id
            ));
        }
    }
    lines.push(next_command_line(board, task));
    lines.join("\n")
}

fn claim_line(task: &BoardTask) -> String {
    let Some(claim) = &task.claim else {
        return "- until -".to_owned();
    };
    let until = task
        .lease
        .as_ref()
        .map_or_else(|| "-".to_owned(), |lease| lease.body.expiry.to_string());
    format!("{} until {}", claim.body.actor, until)
}

fn claim_thread(task: &BoardTask) -> String {
    task.claim
        .as_ref()
        .map_or_else(|| "-".to_owned(), |claim| claim.body.occupant.to_string())
}

fn push_bulleted(lines: &mut Vec<String>, text: &str) {
    let mut parts = text.lines();
    if let Some(first) = parts.next() {
        lines.push(format!("  - {first}"));
    } else {
        lines.push("  -".to_owned());
        return;
    }
    for continuation in parts {
        lines.push(format!("    {continuation}"));
    }
}

fn render_dependency(board: &BoardAnswer, dependency: &TaskStamp) -> String {
    match task_heads_by_stamp(board, dependency) {
        Ok(dependency) => {
            let mut state = dependency.workflow.body.status.clone();
            if dependency
                .claim
                .as_ref()
                .is_some_and(|claim| claim.body.disposition == ClaimDisposition::Active)
            {
                state.push_str(" claim=");
                state.push_str(
                    &dependency
                        .claim
                        .as_ref()
                        .expect("active claim checked")
                        .body
                        .actor
                        .to_string(),
                );
            }
            format!("  after {} {}", dependency.statement.body.handle, state)
        }
        Err(_) => format!("  after {dependency} (missing)"),
    }
}

fn next_command_line(board: &BoardAnswer, task: &BoardTask) -> String {
    if task.workflow.body.phase.as_str() == "review" {
        if active_claimed_by_principal(board, task) {
            return format!(
                "next: spice task review {} --finding clean --note \"description current; ...\"",
                task.statement.body.handle
            );
        }
        return "next: spice task next".to_owned();
    }
    if !active_claimed_by_principal(board, task) {
        return "next: spice task next".to_owned();
    }
    "next: spice task done --validation \"...\"".to_owned()
}

fn active_claimed_by_principal(board: &BoardAnswer, task: &BoardTask) -> bool {
    task.claim.as_ref().is_some_and(|claim| {
        claim.body.actor == board.principal && claim.body.disposition == ClaimDisposition::Active
    })
}

fn actor_route_filter(board: &BoardAnswer) -> String {
    let projects = board
        .project_policy
        .public_stems
        .iter()
        .map(|stem| format!("project:{stem}"))
        .collect::<Vec<_>>()
        .join(" or ");
    format!(
        "( project:{} or origin_thread.is:{} or ( {projects} ) )",
        board.actor_route.project, board.actor_route.origin_thread
    )
}

pub(in crate::board_command) fn task_status(
    board: &BoardAnswer,
) -> Result<String, Outcome<CommandAnswer>> {
    let entries = actor_entries(board)?;
    let routed = board
        .tasks
        .iter()
        .filter(|task| actor_route_matches(board, task))
        .map(|task| task.statement.body.handle.clone())
        .collect::<std::collections::BTreeSet<_>>();
    // The board owns which prerequisites are satisfied. Deciding it again here
    // is how the same row gets counted ready and blocked at once: `ready` asks
    // the board through `eligible_entry`, so `blocked` has to ask the board too.
    let satisfied = board::satisfied_tasks(&entries);
    let pending_actor = |entry: &&Entry| {
        routed.contains(entry.handle())
            && entry.workflow().status == "pending"
            && entry.workflow().completion.is_none()
    };
    let active = |entry: &&Entry| {
        entry
            .claim()
            .is_some_and(|claim| claim.disposition == ClaimDisposition::Active)
    };
    let rows = entries.iter().filter(pending_actor).collect::<Vec<_>>();
    let claim = rows
        .iter()
        .copied()
        .find(|entry| {
            active(entry)
                && entry
                    .claim()
                    .is_some_and(|claim| claim.actor == board.principal)
        })
        .map_or_else(|| "-".to_owned(), |entry| entry.handle().to_string());
    let active_count = rows.iter().copied().filter(active).count();
    let review = rows
        .iter()
        .copied()
        .filter(|entry| entry.workflow().phase.as_str() == "review" && !active(entry))
        .count();
    let waiting = rows
        .iter()
        .filter(|entry| entry.workflow().deferral.is_some())
        .count();
    let blocked = rows
        .iter()
        .filter(|entry| {
            entry
                .statement()
                .dependencies
                .iter()
                .any(|dependency| !satisfied.contains(dependency))
        })
        .count();
    let ready = rows
        .iter()
        .copied()
        .filter(|entry| {
            entry.workflow().phase.as_str() != "review"
                && board::eligible_entry(&entries, entry, &board.principal)
        })
        .count();
    let stale = rows
        .iter()
        .filter(|entry| {
            entry
                .claim()
                .is_some_and(|claim| claim.disposition == ClaimDisposition::Lapsed)
        })
        .count();
    let oops = entries
        .iter()
        .filter(|entry| {
            entry.project().visibility().hidden_stem() == Some("oops")
                && entry.workflow().status == "pending"
                && entry.workflow().completion.is_none()
        })
        .count();
    Ok(format!(
        "claim {claim}\nactor {}\nfilter {}\npublic task project depth {}..{} dotted segments\n\
         active {active_count}\nready {ready}\n\
         review {review}\nblocked {blocked}\nwaiting {waiting}\n\
         stale {stale}\noops {oops}",
        board.actor_route.actor,
        actor_route_filter(board),
        board.project_policy.public_min_depth,
        board.project_policy.public_max_depth,
    ))
}
