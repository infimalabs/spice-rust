use super::*;
use wire::principal::{NamedActor, PrincipalDirectory};

mod environment;
use environment::{environment_status, requirement_lines};
mod list;
pub(super) use list::actor_route_matches;
#[cfg(test)]
pub(in crate::board_command) use list::{TaskListOptions, TaskListStatus, task_list};
use list::{TaskListPageOptions, task_list_page};
mod room;
pub(in crate::board_command) use room::{TaskShowRoom, claim_room_context};
use room::{resumes_in_room, task_show_room};

pub(super) fn client_status(repository: Host<'_>, contract: &Contract) -> Outcome<CommandAnswer> {
    match crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::BoardRead,
        Vec::new(),
        &BoardRead::All { family: None },
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
    repository: Host<'_>,
    contract: &Contract,
    matches: &ArgMatches,
) -> Outcome<CommandAnswer> {
    let options = match TaskListPageOptions::from_matches(matches) {
        Ok(options) => options,
        Err(outcome) => return outcome,
    };
    match crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::BoardRead,
        Vec::new(),
        &BoardRead::All { family: None },
    ) {
        Outcome::Reply(Reply {
            position,
            body: CommandAnswer::Board(board),
        }) => match task_list_page(&board, &options) {
            Ok(text) => Outcome::Reply(Reply {
                position,
                body: CommandAnswer::Text(TextAnswer { text }),
            }),
            Err(outcome) => outcome,
        },
        Outcome::Reply(_) => protocol_fault(
            "task::list-answer",
            "task list board read returned another answer shape",
        ),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

pub(super) fn client_show(
    repository: Host<'_>,
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
    let options = match TaskShowOptions::from_matches(matches) {
        Ok(options) => options,
        Err(outcome) => return outcome,
    };
    match crate::command_client::execute_typed(
        repository,
        contract,
        CommandKind::BoardRead,
        Vec::new(),
        &BoardRead::Tasks {
            tasks: vec![
                task.born()
                    .expect("a parsed handle carries a canonical task stamp"),
            ],
            dependencies: true,
            claims: false,
            // A collapsed show counts the history it does not print.
            history: !options.full || options.context || options.history,
        },
    ) {
        Outcome::Reply(Reply {
            position,
            body: CommandAnswer::Board(board),
        }) => {
            let task = match named_task_heads(&board, &task) {
                Ok(task) => task,
                Err(outcome) => return outcome,
            };
            let artifacts = match task_show_artifacts(repository, task) {
                Ok(artifacts) => artifacts,
                Err(outcome) => return outcome,
            };
            let principals = if options.history {
                match task_show_principals(repository) {
                    Ok(principals) => Some(principals),
                    Err(outcome) => return outcome,
                }
            } else {
                None
            };
            let room = if options.context || options.history {
                match task_show_room(repository, task, &options) {
                    Ok(room) => Some(room),
                    Err(outcome) => return outcome,
                }
            } else {
                None
            };
            let history = TaskShowHistory::of(&board, task, principals.as_ref());
            let origin_context = if options.context {
                match task_show_origin_context(
                    repository,
                    &task_acknowledgment_keys(
                        &task.statement.body.provenance,
                        history.acknowledgments,
                    ),
                ) {
                    Ok(context) => context,
                    Err(outcome) => return outcome,
                }
            } else {
                None
            };
            let text = match task_show_bounded(
                &board,
                task,
                &artifacts,
                history,
                origin_context.as_deref(),
                room,
                &options,
            ) {
                Ok(text) => text,
                Err(outcome) => return outcome,
            };
            Outcome::Reply(Reply {
                position,
                body: CommandAnswer::Text(TextAnswer { text }),
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

/// What one `task show` prints beyond the statement head. Without `full`, the
/// description, acceptance, and unrequested history each collapse to one line
/// naming its count and the `spice://` resource that expands exactly it.
#[derive(Clone)]
pub(in crate::board_command) struct TaskShowOptions {
    pub(in crate::board_command) full: bool,
    pub(in crate::board_command) context: bool,
    pub(in crate::board_command) history: bool,
    pub(in crate::board_command) limit: Option<usize>,
    pub(in crate::board_command) max_bytes: Option<usize>,
    pub(in crate::board_command) max_text: Option<usize>,
    pub(in crate::board_command) after: Option<String>,
}

impl TaskShowOptions {
    fn from_matches(matches: &ArgMatches) -> Result<Self, Outcome<CommandAnswer>> {
        let full = matches.get_flag("full");
        let context = matches.get_flag("context");
        let history = matches.get_flag("history");
        let limit = numeric_option(matches, "limit", "task::show-limit")?;
        let max_bytes = numeric_option(matches, "max-bytes", "task::show-max-bytes")?;
        let max_text = numeric_option(matches, "max-text", "task::show-max-text")?;
        let _ = read_budget(limit, max_bytes, max_text, "task::show-limits")?;
        let after = matches
            .get_one::<String>("after")
            .map(|written| written.trim())
            .filter(|written| !written.is_empty())
            .map(str::to_owned);
        if matches.contains_id("after") && after.is_none() {
            return Err(refusal(
                "task::show-after",
                matches.get_one::<String>("after").cloned(),
                "name the exact detail cursor after --after",
            ));
        }
        if (limit.is_some() || after.is_some()) && !(context || history) {
            return Err(refusal(
                "task::show-detail-required",
                None,
                "add --context or --history when paging detail rows",
            ));
        }
        if (max_bytes.is_some() || max_text.is_some()) && !(context || history) {
            return Err(refusal(
                "task::show-detail-required",
                None,
                "add --context or --history when raising detail limits",
            ));
        }
        Ok(Self {
            full,
            context,
            history,
            limit,
            max_bytes,
            max_text,
            after,
        })
    }

    fn budget(&self) -> wire::output::Budget {
        read_budget(
            self.limit,
            self.max_bytes,
            self.max_text,
            "task::show-limits",
        )
        .expect("CLI parsing admits task show limits")
    }
}

fn numeric_option(
    matches: &ArgMatches,
    name: &str,
    reason: &str,
) -> Result<Option<usize>, Outcome<CommandAnswer>> {
    matches
        .get_one::<String>(name)
        .map(|written| {
            written.parse::<usize>().map_err(|_| {
                refusal(
                    reason,
                    Some(written.clone()),
                    &format!("name a non-negative integer after --{name}"),
                )
            })
        })
        .transpose()
}

fn read_budget(
    rows: Option<usize>,
    bytes: Option<usize>,
    text: Option<usize>,
    reason: &str,
) -> Result<wire::output::Budget, Outcome<CommandAnswer>> {
    wire::output::INTERACTIVE
        .budget(wire::output::Request { rows, bytes, text })
        .map_err(|error| refusal(reason, Some(error.to_string()), &error.to_string()))
}

fn task_show_artifacts(
    repository: Host<'_>,
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

/// Resolve every steering key on one task record inside one journal snapshot.
fn task_show_origin_context(
    repository: Host<'_>,
    keys: &[&str],
) -> Result<Option<Box<str>>, Outcome<CommandAnswer>> {
    if keys.is_empty() {
        return Ok(None);
    }
    let state = repository
        .admit_state()
        .map_err(|error| repository_state_failure(&error))?;
    let reader = journal::JournalReader::open(state.journal()).map_err(|error| {
        store_fault(
            "task::steering-journal-unreadable",
            &format!(
                "cannot inspect steering origins {}: {error}",
                keys.join(" ")
            ),
        )
    })?;
    let blocks = reader
        .read(|read| {
            keys.iter()
                .map(|key| {
                    read_steering_provenance(read, key)
                        .map(|provenance| render_task_steering_origin(key, provenance.as_ref()))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|error| {
            store_fault(
                "task::steering-index-unreadable",
                &format!(
                    "cannot inspect steering origins {}: {error}",
                    keys.join(" ")
                ),
            )
        })?;
    Ok(Some(blocks.join("\n").into_boxed_str()))
}

/// Every born principal's recorded name, read from the local journal, so a
/// task's history names its human authors.
fn task_show_principals(
    repository: Host<'_>,
) -> Result<PrincipalDirectory, Outcome<CommandAnswer>> {
    let state = repository
        .admit_state()
        .map_err(|error| repository_state_failure(&error))?;
    let unreadable = |error: &dyn std::fmt::Display| {
        store_fault(
            "task::principal-names-unreadable",
            &format!("cannot read principal names: {error}"),
        )
    };
    let reader =
        journal::JournalReader::open(state.journal()).map_err(|error| unreadable(&error))?;
    reader
        .read(|read| read.principal_roles())
        .map(|roles| PrincipalDirectory::names(&roles))
        .map_err(|error| unreadable(&error))
}

/// Durable history shown with one task: its closure, its workflow steps, every
/// acknowledged steering key on its record, and the principal names its human
/// authors read under. Its conversation is its room.
#[derive(Clone, Copy, Default)]
pub(in crate::board_command) struct TaskShowHistory<'a> {
    pub(in crate::board_command) acknowledgments: &'a [wire::intent::TaskAcknowledgment],
    pub(in crate::board_command) closure: Option<&'a wire::intent::TaskDetailClosure>,
    pub(in crate::board_command) steps: &'a [wire::intent::TaskDetailStep],
    pub(in crate::board_command) principals: Option<&'a PrincipalDirectory>,
}

impl<'a> TaskShowHistory<'a> {
    /// `task`'s history as a history-bearing `board` read carries it, with
    /// `principals` naming its human authors when they were read.
    pub(in crate::board_command) fn of(
        board: &'a BoardAnswer,
        task: &BoardTask,
        principals: Option<&'a PrincipalDirectory>,
    ) -> Self {
        let born = &task.statement.body.born;
        Self {
            acknowledgments: board.acknowledgments.get(born).map_or(&[], Vec::as_slice),
            closure: board.closures.get(born),
            steps: board.steps.get(born).map_or(&[], Vec::as_slice),
            principals,
        }
    }
}

/// The history `spice task show HANDLE --history` prints, collapsed to one
/// line: its closure and workflow steps counted, with the resource that
/// expands exactly it.
fn collapsed_history(handle: &TaskHandle, history: TaskShowHistory<'_>) -> String {
    match history.steps.len() + usize::from(history.closure.is_some()) {
        0 => "history: -".to_owned(),
        1 => format!("history: 1 step spice://task/{handle}/history"),
        steps => format!("history: {steps} steps spice://task/{handle}/history"),
    }
}

/// The description and acceptance (a Design's questions) whole, each line
/// indented under its label, as `task show --full` prints them.
fn push_full_statement(lines: &mut Vec<String>, statement: &wire::intent::BoardStatement) {
    lines.push("description:".to_owned());
    match statement
        .description
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        Some(text) => lines.extend(text.lines().map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("  {line}")
            }
        })),
        None => lines.push("  -".to_owned()),
    }
    lines.push(format!("{}:", task_items_label(statement)));
    if !substantive_acceptance(&statement.acceptance) {
        lines.push("  -".to_owned());
    } else {
        for item in statement
            .acceptance
            .iter()
            .filter(|item| !item.trim().is_empty())
        {
            push_bulleted(lines, item);
        }
    }
}

fn task_flow_line(flow: &[wire::Phase]) -> String {
    let phase_bytes = flow.iter().map(|phase| phase.as_str().len()).sum::<usize>();
    let mut line =
        String::with_capacity("flow ".len() + phase_bytes + flow.len().saturating_sub(1));
    line.push_str("flow ");
    for (index, phase) in flow.iter().enumerate() {
        if index != 0 {
            line.push(',');
        }
        line.push_str(phase.as_str());
    }
    line
}

#[cfg(test)]
pub(in crate::board_command) fn task_show(
    board: &BoardAnswer,
    task: &BoardTask,
    artifacts: &[artifact::Artifact],
) -> String {
    task_show_with_origin(board, task, artifacts, TaskShowHistory::default(), None)
}

#[cfg(test)]
pub(in crate::board_command) fn task_show_with_origin(
    board: &BoardAnswer,
    task: &BoardTask,
    artifacts: &[artifact::Artifact],
    history: TaskShowHistory<'_>,
    origin_context: Option<&str>,
) -> String {
    let maximum = wire::output::INTERACTIVE.maximum();
    task_show_bounded(
        board,
        task,
        artifacts,
        history,
        origin_context,
        None,
        &TaskShowOptions {
            full: true,
            context: true,
            history: true,
            limit: Some(maximum.rows),
            max_bytes: Some(maximum.bytes),
            max_text: Some(maximum.text),
            after: None,
        },
    )
    .expect("unpaged task detail has no unknown cursor")
}

fn task_items_label(statement: &wire::intent::BoardStatement) -> &'static str {
    match statement.intent {
        Some(wire::TaskIntent::Design) => "questions",
        _ => "acceptance",
    }
}

fn task_header_lines(board: &BoardAnswer, task: &BoardTask) -> Vec<String> {
    let statement = &task.statement.body;
    let workflow = &task.workflow.body;
    vec![
        format!("handle {}", statement.handle),
        format!("title {}", task.presentation_title()),
        format!("pitch: {}", statement.pitch),
        format!("project {}", statement.project),
        format!("filing {}", statement.filing.as_str()),
        format!(
            "priority {}",
            statement.priority.map_or("-", wire::TaskPriority::as_str)
        ),
        format!(
            "intent {}",
            statement.intent.map_or("-", wire::TaskIntent::as_str)
        ),
        format!("phase {}", workflow.phase),
        format!("status {}", workflow.status),
        format!("claim {}", claim_line(task)),
        completion_contract(
            &statement.acceptance,
            &statement.born,
            &statement.dependencies,
            statement.intent,
            board.tasks.iter(),
        ),
    ]
}

pub(in crate::board_command) fn task_show_bounded(
    board: &BoardAnswer,
    task: &BoardTask,
    artifacts: &[artifact::Artifact],
    history: TaskShowHistory<'_>,
    origin_context: Option<&str>,
    room: Option<TaskShowRoom>,
    options: &TaskShowOptions,
) -> Result<String, Outcome<CommandAnswer>> {
    let statement = &task.statement.body;
    let environment = board.environment();
    let mut lines = task_header_lines(board, task);
    lines.extend(
        task.design_standing()
            .map(|standing| format!("standing {standing}")),
    );
    lines.extend(design_cycle_lines(board, task));
    if let Some(preparation) = &statement.preparation {
        lines.extend([
            format!("preparation {}", stamp_name(board, &preparation.parent)),
            format!("controller {}", stamp_name(board, &preparation.controller)),
            format!(
                "construction {}",
                stamp_name(board, &preparation.construction)
            ),
        ]);
    }
    if let Some(hold) = &statement.taste {
        // Settlement admits no rewording, so a settled statement's match is
        // retained evidence rather than a review the task still owes.
        lines.push(if task.workflow.body.status.is_pending() {
            "taste_review required".to_owned()
        } else {
            "taste_review historical".to_owned()
        });
        lines.extend(hold.matches.iter().map(taste_match_line));
    }
    lines.extend(requirement_lines(board, statement, &environment));
    lines.push(format!(
        "environment {}",
        if statement.requirements.are_satisfied_by(&environment)
            && statement.resources.are_satisfied_by(&environment)
        {
            "satisfied"
        } else {
            "blocked"
        }
    ));
    if options.full {
        push_full_statement(&mut lines, statement);
    } else {
        lines.push(crate::board_command::collapsed_description(
            &statement.handle,
            statement.description.as_deref(),
        ));
        lines.push(crate::board_command::collapsed_acceptance(
            &statement.handle,
            task_items_label(statement),
            &statement.acceptance,
        ));
        if !options.history {
            lines.push(collapsed_history(&statement.handle, history));
        }
    }
    lines.push("depends:".to_owned());
    // A Design waits on each construction root it released, so those rows
    // read as prerequisites beside the statement's own edges.
    let released = if wire::intent::is_open_design(statement.intent, &task.workflow.body) {
        board
            .tasks
            .iter()
            .filter(|row| {
                row.workflow.body.status.is_pending()
                    && wire::intent::released_construction_root(
                        &row.statement.body.born,
                        &row.statement.body.dependencies,
                        row.statement.body.preparation.as_ref(),
                    ) == Some(&statement.born)
            })
            .map(|row| &row.statement.body.born)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if statement.dependencies.is_empty() && released.is_empty() {
        lines.push("  -".to_owned());
    } else {
        for dependency in &statement.dependencies {
            lines.push(render_dependency(board, dependency));
        }
        for dependency in released {
            lines.push(format!("{} released", render_dependency(board, dependency)));
        }
    }
    lines.extend(artifact_lines(&statement.handle, artifacts));
    if options.context || options.history {
        lines.extend(bounded_detail_lines(
            board,
            task,
            history,
            origin_context,
            room,
            options,
            options.budget(),
        )?);
    }
    lines.extend(phase_guidance_lines(task));
    lines.push(next_command_line(board, task)?);
    Ok(lines.join("\n"))
}

/// The `artifacts:` section: each artifact with the command that shows it.
fn artifact_lines(handle: &wire::TaskHandle, artifacts: &[artifact::Artifact]) -> Vec<String> {
    if artifacts.is_empty() {
        return vec!["artifacts:".to_owned(), "  -".to_owned()];
    }
    let mut lines = vec!["artifacts:".to_owned()];
    for artifact in artifacts {
        lines.push(format!(
            "  {} {} {} {} B {}",
            artifact.id, artifact.name, artifact.media_type, artifact.bytes, artifact.retention
        ));
        lines.push(format!(
            "     spice task artifact show {handle} {}",
            artifact.id
        ));
    }
    lines
}

fn task_show_context_lines(
    task: &BoardTask,
    history: TaskShowHistory<'_>,
    origin_context: Option<&str>,
    budget: wire::output::Budget,
) -> Vec<String> {
    let statement = &task.statement.body;
    let workflow = &task.workflow.body;
    let mut lines = vec![
        format!("origin {}", statement.provenance),
        task_flow_line(&workflow.flow),
        format!("review {}", workflow.review_str()),
        format!("version {}", statement.revision),
        format!("claim_thread {}", claim_thread(task)),
    ];
    lines.extend(
        acknowledgment_lines(&statement.provenance, history.acknowledgments)
            .into_iter()
            .map(|line| budget.clip(&line).to_string()),
    );
    if let Some(origin_context) = origin_context {
        let clipped = budget.clip(origin_context);
        lines.extend(clipped.text.lines().map(str::to_owned));
        if let Some(withheld) = clipped.withheld_bytes {
            lines.push(format!(
                "origin_context withheld {withheld} bytes; recover with the session context commands below"
            ));
        }
    }
    lines.extend(creation_context_lines(statement));
    lines.extend(claim_context_lines(task));
    lines.push(format!(
        "spice doc ledger {} --output {}-ledger.md",
        statement.handle, statement.handle
    ));
    lines.extend(git_context_lines(task));
    lines.extend(parked_commit_lines(task));
    lines
}

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum TaskShowDetailSection {
    Context,
    Closure,
    Step,
    Room,
}

#[derive(serde::Serialize)]
struct TaskShowDetailRow {
    cursor: String,
    section: TaskShowDetailSection,
    text: String,
}

fn bounded_detail_lines(
    board: &BoardAnswer,
    task: &BoardTask,
    history: TaskShowHistory<'_>,
    origin_context: Option<&str>,
    room: Option<TaskShowRoom>,
    options: &TaskShowOptions,
    budget: wire::output::Budget,
) -> Result<Vec<String>, Outcome<CommandAnswer>> {
    let mut rows = Vec::new();
    if options.context {
        rows.extend(
            task_show_context_lines(task, history, origin_context, budget)
                .into_iter()
                .enumerate()
                .map(|(index, text)| TaskShowDetailRow {
                    cursor: format!("context:{index}"),
                    section: TaskShowDetailSection::Context,
                    text,
                }),
        );
    }
    if options.history {
        if let Some(closure) = history.closure {
            rows.push(TaskShowDetailRow {
                cursor: format!("closure:{}", closure.position),
                section: TaskShowDetailSection::Closure,
                text: closure_lines_bounded(closure, history.principals, budget).join("\n"),
            });
        }
        let mut steps = history.steps.iter().collect::<Vec<_>>();
        steps.sort_by_key(|step| std::cmp::Reverse(step.position));
        rows.extend(steps.into_iter().map(|step| TaskShowDetailRow {
            cursor: format!("step:{}", step.position),
            section: TaskShowDetailSection::Step,
            text: render_task_step(board, step, history.principals, budget),
        }));
    }
    if resumes_in_room(options) {
        rows.clear();
    } else if let Some(after) = &options.after {
        let Some(index) = rows.iter().position(|row| row.cursor == *after) else {
            return Err(refusal(
                "task::show-after-unknown",
                Some(after.clone()),
                "name a detail cursor present in the selected context or history",
            ));
        };
        rows = rows.split_off(index);
    }

    let mut admission = wire::output::Admission::new(budget);
    for row in rows {
        admission
            .offer(row)
            .expect("a task detail text row always serializes");
    }
    let room_empty = room.as_ref().is_some_and(TaskShowRoom::is_empty);
    if let Some(room) = room {
        room.offer(&mut admission);
    }
    let page = admission
        .finish(|resume| task_show_continuation(&task.statement.body.handle, options, resume));
    let mut lines = Vec::new();
    let mut steps_started = false;
    let mut room_started = false;
    for row in page.rows {
        if matches!(row.section, TaskShowDetailSection::Step) && !steps_started {
            lines.push("workflow:".to_owned());
            steps_started = true;
        }
        if matches!(row.section, TaskShowDetailSection::Room) && !room_started {
            lines.push("room:".to_owned());
            room_started = true;
        }
        lines.push(row.text);
    }
    if room_empty {
        lines.extend(["room:".to_owned(), "  -".to_owned()]);
    }
    if let Some(omission) = page.omission {
        lines.push(omission.to_string());
    }
    Ok(lines)
}

fn render_task_step(
    board: &BoardAnswer,
    step: &wire::intent::TaskDetailStep,
    principals: Option<&PrincipalDirectory>,
    budget: wire::output::Budget,
) -> String {
    let author = named(principals, &step.author);
    let mut rendered = format!("  {}", step_heading(step, &author));
    if let Some(text) = step_text(step) {
        let text = budget.clip(text);
        push_indented(&mut rendered, &text.text);
        if let Some(withheld) = text.withheld_bytes {
            rendered.push_str(&format!("\n    … [+{withheld} bytes]"));
        }
    }
    if let Some(disposition) = step_disposition(step, |stamp| stamp_name(board, stamp)) {
        push_indented(&mut rendered, &disposition);
    }
    if let Some(followups) = step_followups(step, |stamp| stamp_name(board, stamp)) {
        push_indented(&mut rendered, &followups);
    }
    rendered
}

/// Append `text` under a history row's heading, four columns in.
fn push_indented(rendered: &mut String, text: &str) {
    for line in text.lines() {
        rendered.push('\n');
        if !line.is_empty() {
            rendered.push_str("    ");
            rendered.push_str(line);
        }
    }
}

/// `actor` named by `principals` when they were read, else its identity alone.
fn named<'a>(principals: Option<&'a PrincipalDirectory>, actor: &'a wire::Actor) -> NamedActor<'a> {
    principals.map_or(
        NamedActor {
            actor,
            principal: None,
        },
        |principals| principals.named(actor),
    )
}

fn closure_lines_bounded(
    closure: &wire::intent::TaskDetailClosure,
    principals: Option<&PrincipalDirectory>,
    budget: wire::output::Budget,
) -> Vec<String> {
    let displaced = closure
        .claimed
        .as_ref()
        .map_or_else(String::new, |actor| format!(" claim-freed={actor}"));
    let mut lines = vec![format!(
        "closed {} {} {} phase={}{displaced}",
        closure.position,
        closure.at,
        named(principals, &closure.author),
        closure.phase
    )];
    let validation = budget.clip(&closure.validation);
    lines.extend(validation.text.lines().map(|line| {
        if line.is_empty() {
            String::new()
        } else {
            format!("    {line}")
        }
    }));
    if let Some(withheld) = validation.withheld_bytes {
        lines.push(format!("    … [+{withheld} bytes]"));
    }
    lines
}

fn task_show_continuation(
    handle: &TaskHandle,
    options: &TaskShowOptions,
    resume: wire::output::Resume<'_, TaskShowDetailRow>,
) -> wire::SpiceCommand {
    let (row, widened) = match resume {
        wire::output::Resume::At(row) => (row, None),
        wire::output::Resume::Widened { record, bytes } => (record, Some(bytes)),
        wire::output::Resume::Export(row) => (row, Some(wire::output::INTERACTIVE.maximum().bytes)),
    };
    let limits = options.budget().resume();
    let mut arguments = vec!["show".to_owned(), handle.to_string()];
    if options.full {
        arguments.push("--full".to_owned());
    }
    if options.context {
        arguments.push("--context".to_owned());
    }
    if options.history {
        arguments.push("--history".to_owned());
    }
    arguments.extend([
        "--limit".to_owned(),
        limits.rows.to_string(),
        "--max-bytes".to_owned(),
        widened.unwrap_or(limits.bytes).to_string(),
        "--max-text".to_owned(),
        limits.text.to_string(),
        "--after".to_owned(),
        row.cursor.clone(),
    ]);
    wire::SpiceCommand::new("task", arguments)
        .expect("task show continuation has structural arguments")
}

/// One line per distinct steering key naming every task fact it caused.
fn acknowledgment_lines(
    origin: &wire::TaskProvenance,
    acknowledgments: &[wire::intent::TaskAcknowledgment],
) -> Vec<String> {
    task_acknowledgment_keys(origin, acknowledgments)
        .into_iter()
        .map(|key| {
            let carriers = acknowledgments
                .iter()
                .filter(|row| row.key == key)
                .map(|row| format!(" {}@{}", row.kind, row.position))
                .collect::<String>();
            format!("acknowledgment ack:{key}{carriers}")
        })
        .collect()
}

fn creation_context_lines(statement: &wire::intent::BoardStatement) -> Vec<String> {
    let output = task_creation_context_output(&statement.creation);
    match &statement.creation {
        body::TaskCreationContext::Unavailable { .. } => {
            vec![format!("creation_context {output}")]
        }
        body::TaskCreationContext::Transcript {
            session, boundary, ..
        } => {
            vec![
                format!(
                    "creation_context session={} lane={} vendor={} stream={} source={} member={} offset={}",
                    session,
                    boundary.lane,
                    boundary.vendor,
                    boundary.stream,
                    boundary.source,
                    boundary.next.member,
                    boundary.next.offset,
                ),
                output.into(),
            ]
        }
    }
}

/// One place a taste held the statement: where it sits and which taste, plus
/// what matched and the taste's message when the reader may see them.
fn taste_match_line(found: &wire::TasteMatch) -> String {
    let place = format!(
        "  {} line {} taste {}",
        found.field.as_str(),
        found.line,
        found.taste
    );
    match &found.finding {
        Some(finding) => format!(
            "{place} trigger {:?} column {}: {}",
            finding.trigger, finding.column, finding.message
        ),
        None => place,
    }
}

fn claim_line(task: &BoardTask) -> String {
    let Some(claim) = &task.claim else {
        return "- until -".to_owned();
    };
    if claim.body.disposition != ClaimDisposition::Active {
        return "- until -".to_owned();
    }
    let until = task
        .lease
        .as_ref()
        .map_or_else(|| "-".to_owned(), |lease| lease.body.expiry.to_string());
    format!("{} until {}", claim.body.actor, until)
}

fn claim_thread(task: &BoardTask) -> String {
    task.claim_context.as_ref().map_or_else(
        || "-".to_owned(),
        |context| match &context.recovery {
            wire::intent::TaskTranscriptRecovery::Transcript { session } => session.clone(),
            wire::intent::TaskTranscriptRecovery::Unavailable { .. } => "-".to_owned(),
        },
    )
}

fn claim_context_lines(task: &BoardTask) -> Vec<String> {
    let Some(context) = &task.claim_context else {
        return Vec::new();
    };
    let mut lines = vec![format!(
        "claim_context fact={} lane={} occupant={} epoch={}",
        context.claimed, context.lane, context.occupant, context.epoch
    )];
    match &context.source {
        body::SourceBoundary::Proven { heads } if heads.is_empty() => {
            lines.push("claim_source proven empty".to_owned());
        }
        body::SourceBoundary::Proven { heads } => {
            for head in heads {
                lines.push(format!(
                    "claim_source lane={} vendor={} stream={} source={} member={} offset={}",
                    head.lane,
                    head.vendor,
                    head.stream,
                    head.source,
                    head.next.member,
                    head.next.offset,
                ));
            }
        }
    }
    match &context.recovery {
        wire::intent::TaskTranscriptRecovery::Transcript { session } => {
            let body::SourceBoundary::Proven { heads } = &context.source;
            let mut registered = heads
                .iter()
                .filter(|head| head.stream == body::TASK_CREATION_CONTEXT_STREAM);
            let Some(boundary) = registered.next() else {
                lines.push("claim_context unavailable recovery-source-missing".to_owned());
                return lines;
            };
            if registered.next().is_some() {
                lines.push("claim_context unavailable recovery-source-ambiguous".to_owned());
                return lines;
            }
            lines.push(shell_words::join([
                "spice".to_owned(),
                "session".to_owned(),
                "context".to_owned(),
                session.clone(),
                "--lane".to_owned(),
                boundary.lane.to_string(),
                "--vendor".to_owned(),
                boundary.vendor.to_string(),
                "--stream".to_owned(),
                boundary.stream.clone(),
                "--source".to_owned(),
                boundary.source.to_string(),
                "--member".to_owned(),
                boundary.next.member.to_string(),
                "--offset".to_owned(),
                boundary.next.offset.to_string(),
            ]));
        }
        wire::intent::TaskTranscriptRecovery::Unavailable { reason } => {
            lines.push(format!("claim_context unavailable {reason}"));
        }
    }
    lines
}

fn parked_commit_lines(task: &BoardTask) -> Vec<String> {
    let Some(parked) = &task.parked_commits else {
        return Vec::new();
    };
    let mut lines = vec![
        format!("parked_baseline {}", parked.baseline),
        format!("parked_head {}", parked.head),
        "parked_commits:".to_owned(),
    ];
    lines.extend(parked.commits.iter().map(|commit| format!("  - {commit}")));
    if let Some(snapshot) = &parked.snapshot {
        lines.push(format!("parked_snapshot {snapshot}"));
    }
    lines
}

fn git_context_lines(task: &BoardTask) -> Vec<String> {
    let Some(context) = &task.git_context else {
        return Vec::new();
    };
    let boundary = &context.boundary;
    let mut lines = vec![format!(
        "git_context kind={} fact={} head={} integrated={} local_commits={}",
        context.kind.as_str(),
        context.fact,
        boundary.head,
        boundary.integrated,
        boundary.local_commits,
    )];
    if let Some(upstream) = &boundary.upstream {
        lines.push(format!("git_upstream {upstream}"));
    }
    if let Some(upstream_head) = &boundary.upstream_head {
        lines.push(format!("git_upstream_head {upstream_head}"));
    }
    let mut command = vec!["git".to_owned(), "show".to_owned()];
    if context.kind == wire::intent::TaskGitContextKind::Claim {
        command.extend([
            "--no-patch".to_owned(),
            "--format=fuller".to_owned(),
            "--end-of-options".to_owned(),
        ]);
        command.push(boundary.head.clone());
    } else {
        command.extend([
            "-m".to_owned(),
            "--first-parent".to_owned(),
            "--stat".to_owned(),
            "--patch".to_owned(),
            "--end-of-options".to_owned(),
        ]);
        command.push(boundary.integrated.clone());
    }
    lines.push(shell_words::join(command));
    lines
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

fn phase_guidance_lines(task: &BoardTask) -> Vec<String> {
    let statement = &task.statement.body;
    let workflow = &task.workflow.body;
    if !statement.filing.is_filed()
        || !workflow.status.is_pending()
        || workflow.phase.as_str() != "plan"
    {
        return Vec::new();
    }
    if workflow.flow.len() == 1 && workflow.flow.first() == Some(&workflow.phase) {
        return vec![
            "phase_guidance:".to_owned(),
            "  phase:plan is this task's entire flow.".to_owned(),
            "  Create an accepted output with spice task add \"...\" --acceptance \"...\". Without an explicit origin it is associated with the ambient planning controller and depends on that Plan; --preparation HANDLE associates it explicitly. A sub-Plan waits on its parent Plan like any other constructed task. Origin alone creates no edge."
                .to_owned(),
            "  Explicit --depends adds ordinary prerequisites. A Design dependency is an ordinary editable edge; design decide removes the accepted outputs' exact edges, closes the rejected ones, and from then on the Design waits on each released output as its prerequisite until that output settles. Never add that controller-to-output edge by hand: it is derived from the output's retained preparation."
                .to_owned(),
            "  Planning changes board state, not repository files; do not edit or commit implementation."
                .to_owned(),
            format!(
                "  Record out-of-scope discoveries with spice task note {} \"...\"; then complete the planning bookend with spice task done --validation \"...\".",
                statement.handle,
            ),
        ];
    }
    vec![
        "phase_guidance:".to_owned(),
        "  phase:plan makes the execution contract explicit on this task or decomposes it into dependency-connected child tasks."
            .to_owned(),
        "  Ordinary output creation without an explicit origin associates with the ambient controller. Use --preparation HANDLE for explicit association; a preparation Plan may run while its Design is unfinished. Origin alone does not schedule work."
            .to_owned(),
        "  Planning changes board state, not repository files; do not edit or commit implementation."
            .to_owned(),
        format!(
            "  Record out-of-scope discoveries with spice task note {} \"...\"; then advance with spice task done --validation \"...\".",
            statement.handle,
        ),
    ]
}

fn render_dependency(board: &BoardAnswer, dependency: &TaskStamp) -> String {
    match task_heads_by_stamp(board, dependency) {
        Ok(dependency) => {
            let mut state = dependency.workflow.body.status.as_str().to_owned();
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

pub(super) fn next_command_line(
    board: &BoardAnswer,
    task: &BoardTask,
) -> Result<String, Outcome<CommandAnswer>> {
    let entries = actor_entries(board)?;
    let entry = entries
        .iter()
        .find(|entry| entry.born() == &task.statement.body.born)
        .ok_or_else(|| {
            protocol_fault(
                "task::detail-next-missing",
                "task detail is absent from its board",
            )
        })?;
    let transition = accepted_transition(
        &board.principal,
        &board.residency.body,
        &entries,
        entry,
        &board.environment(),
    );
    let handle = &task.statement.body.handle;
    if transition != NextTransition::File
        && task.statement.body.taste.is_some()
        && task.workflow.body.status.is_pending()
    {
        return Ok(format!("next: spice task modify {handle} --title \"...\""));
    }
    if let Some(next) = design_cycle_next(board, task) {
        return Ok(next);
    }
    Ok(match transition {
        NextTransition::File => format!("next: spice task file {handle}"),
        NextTransition::Review => format!(
            "next: spice task review {handle} --finding clean --note \"description current; ...\""
        ),
        NextTransition::Done => "next: spice task done --validation \"...\"".to_owned(),
        NextTransition::Propose => design_holder_next(handle),
        NextTransition::Wake => format!("next: spice task wake {handle}"),
        NextTransition::Next => "next: spice task next".to_owned(),
        NextTransition::Settled | NextTransition::Inert => "next: -".to_owned(),
    })
}

fn actor_route_filter(board: &BoardAnswer) -> String {
    format!(
        "( filing:filed or origin_thread.is:{} )",
        board.actor_route.origin_thread
    )
}

pub(in crate::board_command) fn task_status(
    board: &BoardAnswer,
) -> Result<String, Outcome<CommandAnswer>> {
    let entries = actor_entries(board)?;
    let environment = board.environment();
    let rows = actor_rows(board, &entries);
    let active = |entry: &&Entry| {
        entry
            .claim()
            .is_some_and(|claim| claim.disposition == ClaimDisposition::Active)
    };
    let claim = rows
        .iter()
        .copied()
        .find(|entry| {
            active(entry)
                && entry
                    .claim()
                    .is_some_and(|claim| claim.actor == board.principal)
        })
        .map_or_else(|| "-".to_owned(), |entry| actor_claim_lease(board, entry));
    // A taste hold is a statement rewording, not ordinary phase work, and the
    // lane meets status before it reads any one task.
    let holds = rows
        .iter()
        .copied()
        .filter(|entry| entry.statement().taste.is_some())
        .map(|entry| entry.handle().to_string())
        .collect::<Vec<_>>();
    let taste_review = if holds.is_empty() {
        "-".to_owned()
    } else {
        holds.join(" ")
    };
    let unspecified = rows
        .iter()
        .copied()
        .filter(|entry| {
            matches!(
                project_acceptance(
                    &entry.statement().acceptance,
                    entry.born(),
                    &entry.statement().dependencies,
                    entries.iter(),
                ),
                Acceptance::Missing
            )
        })
        .map(|entry| entry.handle().to_string())
        .collect::<Vec<_>>();
    let unspecified = if unspecified.is_empty() {
        "-".to_owned()
    } else {
        unspecified.join(" ")
    };
    // The parked split is shared with the empty-drain readout so the two
    // surfaces cannot disagree on a count.
    let satisfied = board::resolved_tasks(&entries);
    let execution_rows = rows
        .iter()
        .copied()
        .filter(|entry| !satisfied.speculative_design_work(entry.statement(), entry.workflow()))
        .collect::<Vec<_>>();
    let parked = ParkedCounts::measure(board, &entries, &execution_rows, &rows);
    let review = execution_rows
        .iter()
        .copied()
        .filter(|entry| {
            entry.workflow().phase.as_str() == "review"
                && !active(entry)
                && !board.awaiting_selection(entry.born())
        })
        .count();
    let status = environment_status(&execution_rows, &environment);
    let ready = execution_rows
        .iter()
        .copied()
        .filter(|entry| {
            entry.workflow().phase.as_str() != "review"
                && board::eligible_entry(&satisfied, entry, &board.principal, &environment)
        })
        .count();
    let designs = wire::rooms::DesignCycle::ALL
        .iter()
        .map(|cycle| format!(" {}={}", cycle.key(), parked.designs.get(*cycle)))
        .collect::<String>();
    Ok(format!(
        "claim {claim}\ntaste-review {taste_review}\ncompletion-unspecified {unspecified}\nactor {}\nfilter {}\n\
         active {active}\nready {ready}\n\
         review {review}\nblocked {blocked}\n\
         waiting {workflow_waiting}\ndeferred {deferred}\n\
         designs{designs}\n\
         environment journal-schema required={} observed={} \
         trust-grant required={} \
         appliance-version after={} observed={} \
         resource-pool required={} \
         satisfied={} blocked={}\n\
         stale {stale}\ndrafts {drafts}",
        board.actor_route.actor,
        actor_route_filter(board),
        status.schemas,
        board.schema,
        status.trust,
        status.appliance,
        board.appliance,
        status.resources,
        status.satisfied,
        status.blocked,
        active = parked.active,
        blocked = parked.blocked,
        workflow_waiting = parked.workflow_waiting,
        deferred = parked.deferred,
        stale = parked.stale,
        drafts = actor_drafts(board, &entries),
    ))
}

/// Name the actor's own claim with the phase it holds and the lease posture.
///
/// The phase is what decides the lane's next action, and a lane that resumed,
/// compacted, or lost its assignment answer reads status before it reads any
/// one task. A lease the resident rolls while this occupant stays seated needs
/// no manual action, so the posture is the actionable part of the line, not
/// the expiry.
fn actor_claim_lease(board: &BoardAnswer, entry: &Entry) -> String {
    let handle = entry.handle();
    let phase = entry.workflow().phase.as_str();
    let Some(lease) = entry.lease() else {
        return format!("{handle} phase:{phase}");
    };
    let rolling = entry
        .claim()
        .is_some_and(|claim| resident_rolls_lease(board, claim, lease));
    let posture = if rolling { "rolling until" } else { "until" };
    format!("{handle} phase:{phase} {posture} {}", lease.expiry)
}

/// Whether the resident renews this lease on its own rolling horizon.
///
/// This mirrors the resident's renewal eligibility: a live epoch-matched claim
/// held by a lane's current occupant, excluding a lease iced on the canonical
/// indefinite horizon.
fn resident_rolls_lease(board: &BoardAnswer, claim: &ClaimHead, lease: &LeaseHead) -> bool {
    let residency = &board.residency.body;
    residency.state == wire::ResidentState::Seated
        && claim.epoch == residency.epoch
        && !spice::claim_lease::is_indefinite(lease)
        && residency
            .lanes
            .iter()
            .any(|lane| lane.lane == claim.lane && lane.occupant == Some(claim.occupant))
}
