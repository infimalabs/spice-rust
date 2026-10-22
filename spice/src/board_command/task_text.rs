//! A task's description or acceptance as one text that a whole replacement
//! rewrites through the `task modify` intent.
//!
//! The text carries a [`Domain::TaskText`] digest. A replacement names the
//! digest of the text it read; the edit admits it only when that digest still
//! names the statement the edit witnesses. A replacement therefore never lands
//! over text its writer did not see.

use wire::intent::BoardStatement;

use super::*;

/// One replaceable text of a task statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskText {
    Description,
    Acceptance,
}

impl TaskText {
    /// The field's name, as `task modify` and a resource path spell it.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Description => "description",
            Self::Acceptance => "acceptance",
        }
    }

    /// The current text: the description verbatim, or each acceptance item
    /// as a `- ` line whose continuation lines are indented two spaces.
    pub(crate) fn current(self, statement: &BoardStatement) -> String {
        match self {
            Self::Description => statement.description.clone().unwrap_or_default(),
            Self::Acceptance => statement
                .acceptance
                .iter()
                .map(|item| {
                    let mut lines = item.lines();
                    let first = lines.next().unwrap_or_default();
                    std::iter::once(format!("- {first}"))
                        .chain(lines.map(|line| {
                            if line.is_empty() {
                                String::new()
                            } else {
                                format!("  {line}")
                            }
                        }))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    /// The digest a replacement of the current text names.
    pub(crate) fn digest(self, statement: &BoardStatement) -> Digest {
        Digest::compute(Domain::TaskText, self.current(statement).as_bytes())
    }
}

/// `description` collapsed to one line: its line count and the resource that
/// expands exactly it, or `-` when there is none.
pub(crate) fn collapsed_description(handle: &TaskHandle, description: Option<&str>) -> String {
    let lines = description
        .filter(|text| !text.trim().is_empty())
        .map_or(0, |text| text.lines().count());
    collapsed("description", lines, "line", handle, TaskText::Description)
}

/// `acceptance` collapsed to one line under `label` (`questions` for a
/// Design): its item count and the resource that expands exactly it.
pub(crate) fn collapsed_acceptance(
    handle: &TaskHandle,
    label: &str,
    acceptance: &[String],
) -> String {
    let items = acceptance
        .iter()
        .filter(|item| !item.trim().is_empty())
        .count();
    collapsed(label, items, "item", handle, TaskText::Acceptance)
}

fn collapsed(label: &str, count: usize, unit: &str, handle: &TaskHandle, text: TaskText) -> String {
    match count {
        0 => format!("{label}: -"),
        1 => format!("{label}: 1 {unit} spice://task/{handle}/{}", text.name()),
        count => format!(
            "{label}: {count} {unit}s spice://task/{handle}/{}",
            text.name()
        ),
    }
}

/// The acceptance items a written text names, in order.
///
/// A line beginning `- ` (or a lone `-`) opens an item; every other line
/// continues the open one, less a two-space indent. Trailing blank lines of
/// an item are not part of it.
pub(crate) fn acceptance_items(text: &str) -> Result<Vec<String>, String> {
    let mut items: Vec<Vec<&str>> = Vec::new();
    for line in text.lines() {
        if let Some(first) = line.strip_prefix("- ").or((line == "-").then_some("")) {
            items.push(vec![first]);
        } else if let Some(item) = items.last_mut() {
            item.push(line.strip_prefix("  ").unwrap_or(line));
        } else if !line.trim().is_empty() {
            return Err(format!(
                "acceptance text opens each item with `- `; the first line is `{line}`"
            ));
        }
    }
    Ok(items
        .into_iter()
        .map(|mut lines| {
            while lines.len() > 1 && lines.last().is_some_and(|line| line.trim().is_empty()) {
                lines.pop();
            }
            lines.join("\n")
        })
        .collect())
}

/// Map a replacement of `text` onto the `task modify` intent for `task`, or
/// refuse it when `named` is not the digest of the text the edit replaces.
pub(crate) fn text_edit(
    task: &BoardTask,
    text: TaskText,
    named: Option<&Digest>,
    replacement: &str,
) -> Result<(CommandKind, Vec<Witness>, serde_json::Value), Outcome<CommandAnswer>> {
    let statement = &task.statement.body;
    let current = text.digest(statement);
    if named != Some(&current) {
        let (reason, detail) = match named {
            Some(named) => (
                "task::text-digest-moved",
                format!(
                    "the {} changed since the read that named digest {named}; its current digest is {current}, so read it again and replace what it says now",
                    text.name()
                ),
            ),
            None => (
                "task::text-digest-missing",
                format!(
                    "a replacement of the {} names the digest of the text it replaces, currently {current}; read it first",
                    text.name()
                ),
            ),
        };
        return Err(refusal(reason, Some(current.to_string()), &detail));
    }
    let replacement = replacement.trim_end_matches(['\n', '\r']);
    let (description, acceptance) = match text {
        TaskText::Description => (Some(replacement.to_owned()), Vec::new()),
        TaskText::Acceptance => {
            let items = acceptance_items(replacement)
                .map_err(|detail| refusal("task::acceptance-shape", None, &detail))?;
            if items.iter().all(|item| item.trim().is_empty()) {
                return Err(refusal(
                    "task::acceptance-empty",
                    None,
                    "a replacement names at least one `- ` item; acceptance is replaced, never cleared",
                ));
            }
            (None, items)
        }
    };
    Ok((
        CommandKind::TaskEdit,
        vec![statement_witness(task)],
        serde_json::to_value(TaskEdit {
            intent: None,
            task: statement.handle.clone(),
            title: None,
            pitch: None,
            description,
            acceptance,
            requirements: None,
            priority: None,
            project: None,
            flow: None,
        })
        .expect("typed intent encodes"),
    ))
}

/// The current evidence of one task, read as an edit of it reads it.
pub(crate) fn current_task(
    repository: &Repository,
    task: &TaskHandle,
) -> Result<BoardTask, Outcome<CommandAnswer>> {
    let board = current_board(repository, single_task_read(task))?;
    task_heads(&board, task).cloned()
}

/// Replace `text` of `task` through the `task modify` intent, as the CLI's
/// actor, when `named` is the digest of the text it replaces.
pub(crate) fn replace_text(
    repository: &Repository,
    task: &TaskHandle,
    text: TaskText,
    named: Option<&Digest>,
    replacement: &str,
) -> Outcome<CommandAnswer> {
    let contract =
        crate::cli::contract(&["task", "modify"]).expect("task modify contract is registered");
    mutate_from_board(repository, contract, single_task_read(task), |board| {
        text_edit(task_heads(&board, task)?, text, named, replacement)
    })
}

fn single_task_read(task: &TaskHandle) -> BoardRead {
    BoardRead::Tasks {
        claims: false,
        tasks: vec![
            task.born()
                .expect("a parsed handle carries a canonical task stamp"),
        ],
        dependencies: false,
        history: false,
    }
}
