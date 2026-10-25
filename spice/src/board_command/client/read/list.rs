//! Bounded, relevance-ordered task list projection.

use super::*;

#[derive(Clone, Copy)]
pub(in crate::board_command) enum TaskListStatus {
    Pending,
    Waiting,
    Completed,
    Deleted,
}

#[derive(Clone, Default)]
pub(in crate::board_command) struct TaskListOptions {
    pub(in crate::board_command) all: bool,
    pub(in crate::board_command) project: Option<String>,
    pub(in crate::board_command) limit: Option<usize>,
    pub(in crate::board_command) status: Option<TaskListStatus>,
}

pub(super) struct TaskListPageOptions {
    selection: TaskListOptions,
    max_bytes: Option<usize>,
    max_text: Option<usize>,
    after: Option<TaskHandle>,
}

impl TaskListPageOptions {
    pub(super) fn from_matches(matches: &ArgMatches) -> Result<Self, Outcome<CommandAnswer>> {
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
        let limit = numeric_option(matches, "limit", "task::list-limit")?;
        let max_bytes = numeric_option(matches, "max-bytes", "task::list-max-bytes")?;
        let max_text = numeric_option(matches, "max-text", "task::list-max-text")?;
        let _ = read_budget(limit, max_bytes, max_text, "task::list-limits")?;
        let after = matches
            .get_one::<String>("after")
            .map(|written| {
                written.parse::<TaskHandle>().map_err(|error| {
                    refusal(
                        "task::list-after",
                        Some(written.clone()),
                        &error.to_string(),
                    )
                })
            })
            .transpose()?;
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
            selection: TaskListOptions {
                all,
                project,
                limit,
                status,
            },
            max_bytes,
            max_text,
            after,
        })
    }

    fn budget(&self) -> wire::output::Budget {
        read_budget(
            self.selection.limit,
            self.max_bytes,
            self.max_text,
            "task::list-limits",
        )
        .expect("CLI parsing and test fixtures admit task list limits")
    }
}

#[derive(serde::Serialize)]
struct TaskListRow {
    handle: TaskHandle,
    text: String,
}

#[cfg(test)]
pub(in crate::board_command) fn task_list(
    board: &BoardAnswer,
    options: &TaskListOptions,
) -> String {
    task_list_page(
        board,
        &TaskListPageOptions {
            selection: options.clone(),
            max_bytes: None,
            max_text: None,
            after: None,
        },
    )
    .expect("test task list cursor is present in its selected scope")
}

pub(super) fn task_list_page(
    board: &BoardAnswer,
    options: &TaskListPageOptions,
) -> Result<String, Outcome<CommandAnswer>> {
    let budget = options.budget();
    let mut tasks = board
        .tasks
        .iter()
        .filter(|task| list_scope_matches(board, task, &options.selection))
        .filter(|task| list_status_matches(task, &options.selection))
        .collect::<Vec<_>>();
    tasks.sort_by(|left, right| {
        task_relevance(board, left)
            .cmp(&task_relevance(board, right))
            .then_with(|| task_newest_position(right).cmp(&task_newest_position(left)))
            .then_with(|| right.statement.body.handle.cmp(&left.statement.body.handle))
    });
    if let Some(after) = &options.after {
        let Some(index) = tasks
            .iter()
            .position(|task| task.statement.body.handle == *after)
        else {
            return Err(refusal(
                "task::list-after-unknown",
                Some(after.to_string()),
                "name a task present in the selected scope and status",
            ));
        };
        tasks = tasks.split_off(index);
    }

    let (scope, empty) = if options.selection.all {
        (
            "scope global --all".to_owned(),
            "no tasks in global scope".to_owned(),
        )
    } else if let Some(project) = &options.selection.project {
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
    let mut admission = wire::output::Admission::new(budget);
    for task in tasks {
        let title = budget.clip(task.presentation_title()).to_string();
        admission
            .offer(TaskListRow {
                handle: task.statement.body.handle.clone(),
                text: render_task_list_row(task, &title),
            })
            .expect("a task list text row always serializes");
    }
    let page = admission.finish(|resume| task_list_continuation(options, resume));
    if page.rows.is_empty() && page.omission.is_none() {
        return Ok(format!("{scope}\n{empty}"));
    }
    let mut output = scope;
    for row in page.rows {
        output.push('\n');
        output.push_str(&row.text);
    }
    if let Some(omission) = page.omission {
        output.push('\n');
        output.push_str(&omission.to_string());
    }
    Ok(output)
}

fn task_list_continuation(
    options: &TaskListPageOptions,
    resume: wire::output::Resume<'_, TaskListRow>,
) -> wire::SpiceCommand {
    let (row, widened) = match resume {
        wire::output::Resume::At(row) => (row, None),
        wire::output::Resume::Widened { record, bytes } => (record, Some(bytes)),
        wire::output::Resume::Export(row) => (row, Some(wire::output::INTERACTIVE.maximum().bytes)),
    };
    let limits = options.budget().resume();
    let mut arguments = vec!["list".to_owned()];
    if options.selection.all {
        arguments.push("--all".to_owned());
    } else if let Some(project) = &options.selection.project {
        arguments.extend(["--project".to_owned(), project.clone()]);
    }
    if let Some(status) = options.selection.status {
        arguments.extend([
            "--status".to_owned(),
            match status {
                TaskListStatus::Pending => "pending",
                TaskListStatus::Waiting => "waiting",
                TaskListStatus::Completed => "completed",
                TaskListStatus::Deleted => "deleted",
            }
            .to_owned(),
        ]);
    }
    arguments.extend([
        "--limit".to_owned(),
        limits.rows.to_string(),
        "--max-bytes".to_owned(),
        widened.unwrap_or(limits.bytes).to_string(),
        "--max-text".to_owned(),
        limits.text.to_string(),
        "--after".to_owned(),
        row.handle.to_string(),
    ]);
    wire::SpiceCommand::new("task", arguments)
        .expect("task list continuation has structural arguments")
}

fn list_scope_matches(board: &BoardAnswer, task: &BoardTask, options: &TaskListOptions) -> bool {
    if let Some(project) = &options.project {
        return task.statement.body.project == *project;
    }
    options.all || actor_route_matches(board, task)
}

/// Whether the actor may read the row by default: every filed row, every
/// Draft for the operator, plus every row created on the actor's own origin
/// thread.
///
/// Deliberately wider than allocation scope while keeping another agent
/// thread's Drafts out of default detailed reads.
pub(in crate::board_command::client) fn actor_route_matches(
    board: &BoardAnswer,
    task: &BoardTask,
) -> bool {
    let filing = task.statement.body.filing;
    filing.is_filed()
        || (filing.is_draft() && wire::principal::ActingPrincipal::of(&board.principal).is_some())
        || task.origin_thread == board.actor_route.origin_thread
}

fn list_status_matches(task: &BoardTask, options: &TaskListOptions) -> bool {
    let pending = task.workflow.body.status.is_pending() && task.workflow.body.deferral.is_none();
    let waiting = task.workflow.body.status.is_pending() && task.workflow.body.deferral.is_some();
    match options.status {
        Some(TaskListStatus::Pending) => pending,
        Some(TaskListStatus::Waiting) => waiting,
        Some(TaskListStatus::Completed) => task.workflow.body.status.is_completed(),
        // A withdrawal leaves the row readable but off the working board, so the
        // only way to reach one is to ask for it by name or to ask for all.
        Some(TaskListStatus::Deleted) => task.workflow.body.status.is_deleted(),
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

fn task_relevance(board: &BoardAnswer, task: &BoardTask) -> u8 {
    if task.claim.as_ref().is_some_and(|claim| {
        claim.body.actor == board.principal && claim.body.disposition == ClaimDisposition::Active
    }) {
        return 0;
    }
    if task.workflow.body.status.is_pending() {
        return if task.workflow.body.deferral.is_none() {
            1
        } else {
            2
        };
    }
    if task.workflow.body.status.is_completed() {
        3
    } else if task.workflow.body.status.is_deleted() {
        4
    } else {
        unreachable!("workflow status is live, completed, or deleted")
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

fn render_task_list_row(task: &BoardTask, title: &str) -> String {
    // A terminal row shows how it ended, not the phase it stopped in. Falling
    // through to the phase would print a withdrawn task as `[todo]`, which is
    // the one thing it is not, and the predecessor prints `[deleted]` here.
    let state = if task.workflow.body.status.is_completed() {
        "done"
    } else if task.workflow.body.status.is_deleted() {
        "deleted"
    } else {
        task.workflow.body.phase.as_str()
    };
    let priority = task
        .statement
        .body
        .priority
        .map_or_else(String::new, |priority| {
            format!(" P:{}", board::priority_label(priority))
        });
    format!(
        "{} [{state}]{priority} {} {title}\n  {}",
        task.statement.body.handle, task.statement.body.project, task.statement.body.pitch,
    )
}
