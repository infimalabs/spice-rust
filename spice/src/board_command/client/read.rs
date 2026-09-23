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
    let options = match TaskListOptions::from_matches(matches) {
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
        &BoardRead::Tasks {
            tasks: vec![
                task.born()
                    .expect("a parsed handle carries a canonical task stamp"),
            ],
            dependencies: true,
            claims: false,
        },
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
pub(super) fn actor_route_matches(board: &BoardAnswer, task: &BoardTask) -> bool {
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
    let environment = board.environment();
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
    ];
    lines.extend(creation_context_lines(statement));
    lines.extend(claim_context_lines(task));
    lines.push(format!("spice doc ledger {}", statement.handle));
    lines.extend(git_context_lines(task));
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
    lines.extend(phase_guidance_lines(task));
    lines.push(next_command_line(board, task));
    lines.join("\n")
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
    let (environment_satisfied, environment_blocked, required_schemas, required_trust) =
        environment_status(&rows, &environment);
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
         environment journal-schema required={required_schemas} observed={} \
         trust-grant required={required_trust} \
         satisfied={environment_satisfied} blocked={environment_blocked}\n\
         stale {stale}\noops {oops}",
        board.actor_route.actor,
        actor_route_filter(board),
        board.project_policy.public_min_depth,
        board.project_policy.public_max_depth,
        board.schema,
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

fn environment_status(
    rows: &[&Entry],
    environment: &wire::Environment,
) -> (usize, usize, String, String) {
    let blocked = rows
        .iter()
        .filter(|entry| !board::environment_satisfied(entry, environment))
        .count();
    let mut schemas = Vec::new();
    let mut trust = Vec::new();
    for requirement in rows
        .iter()
        .flat_map(|entry| entry.statement().requirements.as_slice())
    {
        match requirement {
            wire::EnvironmentRequirement::JournalSchema { value } => schemas.push(value.get()),
            wire::EnvironmentRequirement::TrustGrant { value } => trust.push(value.to_string()),
        }
    }
    schemas.sort_unstable();
    schemas.dedup();
    trust.sort();
    trust.dedup();
    let schemas = if schemas.is_empty() {
        "-".to_owned()
    } else {
        schemas
            .into_iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(",")
    };
    let trust = if trust.is_empty() {
        "-".to_owned()
    } else {
        trust.join(",")
    };
    (rows.len() - blocked, blocked, schemas, trust)
}
