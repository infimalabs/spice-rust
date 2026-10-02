use super::*;

mod list;
pub(super) use list::actor_route_matches;
#[cfg(test)]
pub(in crate::board_command) use list::{TaskListOptions, TaskListStatus, task_list};
use list::{TaskListPageOptions, task_list_page};

pub(super) fn client_status(
    repository: &Repository,
    contract: &Contract,
) -> Outcome<CommandAnswer> {
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
    repository: &Repository,
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
            notes: options.context || options.history,
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
            let acknowledgments = board
                .acknowledgments
                .get(&task.statement.body.born)
                .map_or(&[][..], Vec::as_slice);
            if let Some(after) = options.after {
                let known = board
                    .notes
                    .get(&task.statement.body.born)
                    .is_some_and(|notes| notes.iter().any(|note| note.position == after));
                if !known {
                    return refusal(
                        "task::show-after-unknown",
                        Some(after.to_string()),
                        "name a note position present on this task",
                    );
                }
            }
            let origin_context = if options.context {
                match task_show_origin_context(
                    repository,
                    &task_acknowledgment_keys(&task.statement.body.provenance, acknowledgments),
                ) {
                    Ok(context) => context,
                    Err(outcome) => return outcome,
                }
            } else {
                None
            };
            Outcome::Reply(Reply {
                position,
                body: CommandAnswer::Text(TextAnswer {
                    text: task_show_bounded(
                        &board,
                        task,
                        &artifacts,
                        TaskShowHistory {
                            notes: board
                                .notes
                                .get(&task.statement.body.born)
                                .map_or(&[], Vec::as_slice),
                            acknowledgments,
                            closure: board.closures.get(&task.statement.body.born),
                        },
                        origin_context.as_deref(),
                        &options,
                    ),
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
pub(in crate::board_command) struct TaskShowOptions {
    pub(in crate::board_command) context: bool,
    pub(in crate::board_command) history: bool,
    pub(in crate::board_command) limit: Option<usize>,
    pub(in crate::board_command) max_bytes: Option<usize>,
    pub(in crate::board_command) max_text: Option<usize>,
    pub(in crate::board_command) after: Option<Seq>,
}

impl TaskShowOptions {
    fn from_matches(matches: &ArgMatches) -> Result<Self, Outcome<CommandAnswer>> {
        let context = matches.get_flag("context");
        let history = matches.get_flag("history");
        let limit = numeric_option(matches, "limit", "task::show-limit")?;
        let max_bytes = numeric_option(matches, "max-bytes", "task::show-max-bytes")?;
        let max_text = numeric_option(matches, "max-text", "task::show-max-text")?;
        let _ = read_budget(limit, max_bytes, max_text, "task::show-limits")?;
        let after = matches
            .get_one::<String>("after")
            .map(|written| {
                let value = written.parse::<u64>().ok().and_then(Seq::new);
                value.ok_or_else(|| {
                    refusal(
                        "task::show-after",
                        Some(written.clone()),
                        "name a positive journal position after --after",
                    )
                })
            })
            .transpose()?;
        if (limit.is_some() || after.is_some()) && !history {
            return Err(refusal(
                "task::show-history-required",
                None,
                "add --history when paging historical rows",
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
            context,
            history,
            limit,
            max_bytes,
            max_text,
            after,
        })
    }

    fn budget(self) -> wire::output::Budget {
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

/// Resolve every steering key on one task record inside one journal snapshot.
fn task_show_origin_context(
    repository: &Repository,
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

/// Durable history shown with one task: its notes and every fact whose
/// origin names an acknowledged steering key.
#[derive(Clone, Copy, Default)]
pub(in crate::board_command) struct TaskShowHistory<'a> {
    pub(in crate::board_command) notes: &'a [wire::intent::TaskDetailNote],
    pub(in crate::board_command) acknowledgments: &'a [wire::intent::TaskAcknowledgment],
    pub(in crate::board_command) closure: Option<&'a wire::intent::TaskDetailClosure>,
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
        &TaskShowOptions {
            context: true,
            history: true,
            limit: Some(maximum.rows),
            max_bytes: Some(maximum.bytes),
            max_text: Some(maximum.text),
            after: None,
        },
    )
}

pub(in crate::board_command) fn task_show_bounded(
    board: &BoardAnswer,
    task: &BoardTask,
    artifacts: &[artifact::Artifact],
    history: TaskShowHistory<'_>,
    origin_context: Option<&str>,
    options: &TaskShowOptions,
) -> String {
    let statement = &task.statement.body;
    let workflow = &task.workflow.body;
    let environment = board.environment();
    let budget = options.budget();
    let mut lines = vec![
        format!("handle {}", statement.handle),
        format!("title {}", task.presentation_title()),
        format!("project {}", statement.project),
        format!("phase {}", workflow.phase),
        format!("status {}", workflow.status),
        format!("claim {}", claim_line(task)),
    ];
    if options.context {
        append_task_show_context(&mut lines, task, history, origin_context, budget);
    }
    if has_taste_review(&statement.tags) {
        lines.push("taste_review required".to_owned());
    }
    lines.extend(requirement_lines(board, statement, &environment));
    lines.push(format!(
        "environment {}",
        if statement.requirements.are_satisfied_by(&environment) {
            "satisfied"
        } else {
            "blocked"
        }
    ));
    lines.push("acceptance:".to_owned());
    if !substantive_acceptance(&statement.acceptance) {
        lines.push("  -".to_owned());
    } else {
        for item in statement
            .acceptance
            .iter()
            .filter(|item| !item.trim().is_empty())
        {
            push_bulleted(&mut lines, item);
        }
    }
    lines.push("depends:".to_owned());
    if statement.dependencies.is_empty() {
        lines.push("  -".to_owned());
    } else {
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
    if options.history {
        lines.extend(bounded_history_lines(
            statement.handle.clone(),
            history,
            *options,
        ));
    }
    lines.extend(phase_guidance_lines(task));
    lines.push(next_command_line(board, task));
    lines.join("\n")
}

fn append_task_show_context(
    lines: &mut Vec<String>,
    task: &BoardTask,
    history: TaskShowHistory<'_>,
    origin_context: Option<&str>,
    budget: wire::output::Budget,
) {
    let statement = &task.statement.body;
    let workflow = &task.workflow.body;
    lines.extend([
        format!(
            "description {}",
            budget.clip(statement.description.as_deref().unwrap_or("-"))
        ),
        format!("origin {}", statement.provenance),
        task_flow_line(&workflow.flow),
        format!("review {}", workflow.review),
        format!("version {}", statement.revision),
        format!("claim_thread {}", claim_thread(task)),
    ]);
    lines.extend(acknowledgment_lines(
        &statement.provenance,
        history.acknowledgments,
    ));
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
    let tags = visible_tags(&statement.tags).collect::<Vec<_>>();
    if tags.is_empty() {
        lines.push("tags -".to_owned());
    } else {
        lines.push(format!(
            "tags {}",
            tags.iter()
                .map(|tag| format!("+{tag}"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
}

#[derive(serde::Serialize)]
struct TaskShowHistoryRow {
    position: Seq,
    text: String,
}

fn bounded_history_lines(
    handle: TaskHandle,
    history: TaskShowHistory<'_>,
    options: TaskShowOptions,
) -> Vec<String> {
    let budget = options.budget();
    let mut lines = closure_lines_bounded(history.closure, budget);
    lines.push("notes:".to_owned());
    let mut notes = history.notes.iter().collect::<Vec<_>>();
    notes.sort_by_key(|note| std::cmp::Reverse(note.position));
    if let Some(after) = options.after {
        if let Some(index) = notes.iter().position(|note| note.position == after) {
            notes = notes.split_off(index);
        } else {
            lines.push(format!("  cursor {after} is not a note on this task"));
            return lines;
        }
    }
    let mut admission = wire::output::Admission::new(budget);
    for note in notes {
        let origin = history
            .acknowledgments
            .iter()
            .find(|row| row.position == note.position)
            .map_or_else(String::new, |row| format!(" ack:{}", row.key));
        let text = budget.clip(&note.text);
        let mut rendered = format!("  {} {} {}{origin}", note.position, note.at, note.author);
        for line in text.text.lines() {
            rendered.push('\n');
            if !line.is_empty() {
                rendered.push_str("    ");
                rendered.push_str(line);
            }
        }
        if let Some(withheld) = text.withheld_bytes {
            rendered.push_str(&format!("\n    … [+{withheld} bytes]"));
        }
        admission
            .offer(TaskShowHistoryRow {
                position: note.position,
                text: rendered,
            })
            .expect("a task history text row always serializes");
    }
    let page = admission.finish(|resume| task_show_continuation(&handle, options, resume));
    if page.rows.is_empty() && page.omission.is_none() {
        lines.push("  -".to_owned());
    } else {
        lines.extend(page.rows.into_iter().map(|row| row.text));
        if let Some(omission) = page.omission {
            lines.push(omission.to_string());
        }
    }
    lines
}

fn closure_lines_bounded(
    closure: Option<&wire::intent::TaskDetailClosure>,
    budget: wire::output::Budget,
) -> Vec<String> {
    let Some(closure) = closure else {
        return Vec::new();
    };
    let displaced = closure
        .claimed
        .as_ref()
        .map_or_else(String::new, |actor| format!(" claim-freed={actor}"));
    let mut lines = vec![format!(
        "closed {} {} {} phase={}{displaced}",
        closure.position, closure.at, closure.author, closure.phase
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
    options: TaskShowOptions,
    resume: wire::output::Resume<'_, TaskShowHistoryRow>,
) -> wire::SpiceCommand {
    let (row, widened) = match resume {
        wire::output::Resume::At(row) => (row, None),
        wire::output::Resume::Widened { record, bytes } => (record, Some(bytes)),
        wire::output::Resume::Export(row) => (row, Some(wire::output::INTERACTIVE.maximum().bytes)),
    };
    let limits = options.budget().limits();
    wire::SpiceCommand::new(
        "task",
        [
            "show".to_owned(),
            handle.to_string(),
            "--history".to_owned(),
            "--limit".to_owned(),
            limits.rows.to_string(),
            "--max-bytes".to_owned(),
            widened.unwrap_or(limits.bytes).to_string(),
            "--max-text".to_owned(),
            limits.text.to_string(),
            "--after".to_owned(),
            row.position.to_string(),
        ],
    )
    .expect("task show continuation has structural arguments")
}

/// One line per distinct steering key naming every task fact it caused.
fn acknowledgment_lines(
    origin: &wire::Origin,
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

fn requirement_lines(
    board: &BoardAnswer,
    statement: &wire::intent::BoardStatement,
    environment: &wire::Environment,
) -> Vec<String> {
    let mut lines = vec!["requirements:".to_owned()];
    if statement.requirements.is_empty() {
        lines.push("  -".to_owned());
        return lines;
    }
    for requirement in statement.requirements.as_slice() {
        match requirement {
            wire::EnvironmentRequirement::JournalSchema { value: required } => {
                let observed = board.schema;
                let observation = wire::EnvironmentObservation::journal_schema(observed)
                    .expect("an admitted journal schema is positive");
                lines.push(format!(
                    "  - journal-schema required={} observed={observed} satisfied={}",
                    required,
                    requirement.is_satisfied_by(&observation),
                ));
            }
            wire::EnvironmentRequirement::TrustGrant { value: required } => {
                let observed = board
                    .trust
                    .iter()
                    .find(|head| head.body.digest == *required)
                    .map(|head| &head.body);
                let standing = observed.map_or("missing", |head| match head.state {
                    wire::TrustState::Pended => "pended",
                    wire::TrustState::Granted => "granted",
                    wire::TrustState::Revoked => "revoked",
                });
                let producer = observed
                    .and_then(|head| head.worktree.as_deref())
                    .unwrap_or("-");
                let satisfied = wire::EnvironmentRequirements::new(vec![requirement.clone()])
                    .expect("one admitted requirement is canonical")
                    .are_satisfied_by(environment);
                lines.push(format!(
                    "  - trust-grant required={required} standing={standing} producer={producer} satisfied={satisfied}"
                ));
            }
            wire::EnvironmentRequirement::ApplianceVersion { value: required } => {
                let observation = wire::EnvironmentObservation::ApplianceVersion {
                    value: board.appliance.clone(),
                };
                lines.push(format!(
                    "  - appliance-version after={required} observed={} satisfied={}",
                    board.appliance,
                    requirement.is_satisfied_by(&observation),
                ));
            }
        }
    }
    lines
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
        body::SourceBoundary::Incomplete { reason } => {
            lines.push(format!("claim_source unavailable {reason}"));
        }
    }
    match &context.recovery {
        wire::intent::TaskTranscriptRecovery::Transcript { session } => {
            let body::SourceBoundary::Proven { heads } = &context.source else {
                lines.push("claim_context unavailable recovery-source-incomplete".to_owned());
                return lines;
            };
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
    if workflow.status != "pending" || workflow.phase.as_str() != "plan" {
        return Vec::new();
    }
    let handle = &statement.handle;
    if has_taste_review(&statement.tags) {
        return vec![
            "phase_guidance:".to_owned(),
            "  phase:plan is holding this task for taste review.".to_owned(),
            format!(
                "  Update the task statement with spice task modify {handle} so the complete current taste policy rechecks it."
            ),
            "  Taste review clears only after that recheck finds no current matches; planning changes board state, not repository files."
                .to_owned(),
        ];
    }
    if workflow.flow.len() == 1 && workflow.flow.first() == Some(&workflow.phase) {
        return vec![
            "phase_guidance:".to_owned(),
            "  phase:plan is this task's entire flow; connect at least one pending child task carrying acceptance, because acceptance on this task alone cannot complete it."
                .to_owned(),
            format!(
                "  Create an accepted child with spice task add \"...\" --project PROJECT --acceptance \"...\", then connect it with spice task depends {handle} CHILD."
            ),
            "  Planning changes board state, not repository files; do not edit or commit implementation."
                .to_owned(),
            format!(
                "  Record out-of-scope discoveries with spice task note {handle} \"...\"; then complete the planning bookend with spice task done --validation \"...\"."
            ),
        ];
    }
    vec![
        "phase_guidance:".to_owned(),
        "  phase:plan makes the execution contract explicit on this task or decomposes it into dependency-connected child tasks."
            .to_owned(),
        format!(
            "  Add acceptance with spice task modify {handle} --acceptance \"...\"; or create accepted children with spice task add \"...\" --project PROJECT --acceptance \"...\" and connect them with spice task depends {handle} CHILD."
        ),
        "  Planning changes board state, not repository files; do not edit or commit implementation."
            .to_owned(),
        format!(
            "  Record out-of-scope discoveries with spice task note {handle} \"...\"; then advance with spice task done --validation \"...\"."
        ),
    ]
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
    if has_taste_review(&task.statement.body.tags) {
        return format!(
            "next: spice task modify {} --title \"...\"",
            task.statement.body.handle,
        );
    }
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
        .filter(|entry| has_taste_review(&entry.statement().tags))
        .map(|entry| entry.handle().to_string())
        .collect::<Vec<_>>();
    let taste_review = if holds.is_empty() {
        "-".to_owned()
    } else {
        holds.join(" ")
    };
    let review = rows
        .iter()
        .copied()
        .filter(|entry| entry.workflow().phase.as_str() == "review" && !active(entry))
        .count();
    // The parked split is shared with the empty-drain readout so the two
    // surfaces cannot disagree on a count.
    let parked = ParkedCounts::measure(&entries, &rows);
    let status = environment_status(&rows, &environment);
    let ready = rows
        .iter()
        .copied()
        .filter(|entry| {
            entry.workflow().phase.as_str() != "review"
                && board::eligible_entry(&entries, entry, &board.principal, &environment)
        })
        .count();
    Ok(format!(
        "claim {claim}\ntaste-review {taste_review}\nactor {}\nfilter {}\n\
         public task project depth {}..{} dotted segments\n\
         active {active}\nready {ready}\n\
         review {review}\nblocked {blocked}\nwaiting {waiting}\n\
         environment journal-schema required={} observed={} \
         trust-grant required={} \
         appliance-version after={} observed={} \
         satisfied={} blocked={}\n\
         stale {stale}\noops {oops}",
        board.actor_route.actor,
        actor_route_filter(board),
        board.project_policy.public_min_depth,
        board.project_policy.public_max_depth,
        status.schemas,
        board.schema,
        status.trust,
        status.appliance,
        board.appliance,
        status.satisfied,
        status.blocked,
        active = parked.active,
        blocked = parked.blocked,
        waiting = parked.waiting,
        stale = parked.stale,
        oops = parked.oops,
    ))
}

/// Name the actor's own claim with the phase it holds and the lease posture.
///
/// The phase is what decides the lane's next action, and a lane that resumed,
/// compacted, or lost its assignment answer reads status before it reads any
/// one task. A lease the resident rolls while this occupant stays seated needs
/// no reclaim, so the posture is the actionable part of the rest of the line,
/// not the expiry.
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

struct EnvironmentStatus {
    satisfied: usize,
    blocked: usize,
    schemas: String,
    trust: String,
    appliance: String,
}

fn environment_status(rows: &[&Entry], environment: &wire::Environment) -> EnvironmentStatus {
    let blocked = rows
        .iter()
        .filter(|entry| !board::environment_satisfied(entry, environment))
        .count();
    let mut schemas = Vec::new();
    let mut trust = Vec::new();
    let mut appliance = Vec::new();
    for requirement in rows
        .iter()
        .flat_map(|entry| entry.statement().requirements.as_slice())
    {
        match requirement {
            wire::EnvironmentRequirement::JournalSchema { value } => schemas.push(value.get()),
            wire::EnvironmentRequirement::TrustGrant { value } => trust.push(value.to_string()),
            wire::EnvironmentRequirement::ApplianceVersion { value } => appliance.push(value),
        }
    }
    schemas.sort_unstable();
    schemas.dedup();
    trust.sort();
    trust.dedup();
    appliance.sort_by(|left, right| {
        left.precedence(right)
            .then_with(|| left.as_str().cmp(right.as_str()))
    });
    appliance.dedup();
    EnvironmentStatus {
        satisfied: rows.len() - blocked,
        blocked,
        schemas: listed(schemas.into_iter().map(|value| value.to_string())),
        trust: listed(trust.into_iter()),
        appliance: listed(appliance.into_iter().map(ToString::to_string)),
    }
}

fn listed(values: impl Iterator<Item = String>) -> String {
    let joined = values.collect::<Vec<_>>().join(",");
    if joined.is_empty() {
        "-".to_owned()
    } else {
        joined
    }
}
