//! Claim-bound task contract deltas carried by statement edit facts.

use super::*;

pub(super) fn append_dependency_reachability_witnesses(
    board: &BoardAnswer,
    starts: &[TaskStamp],
    expected: &mut Vec<Witness>,
) -> Result<(), Outcome<CommandAnswer>> {
    let mut pending = starts.to_vec();
    let mut visited = Vec::new();
    while let Some(task) = pending.pop() {
        if visited.contains(&task) {
            continue;
        }
        visited.push(task.clone());
        let heads = board
            .tasks
            .iter()
            .find(|heads| heads.statement.body.born == task)
            .ok_or_else(|| {
                protocol_fault(
                    "task::board-incoherent",
                    "a stored dependency has no visible board task",
                )
            })?;
        let statement = statement_witness(heads);
        if !expected.contains(&statement) {
            expected.push(statement);
        }
        pending.extend(heads.statement.body.dependencies.iter().cloned());
    }
    Ok(())
}

pub(super) fn task_edit_notice(entry: &Entry, next: &TaskHead) -> Option<body::TaskEditNotice> {
    let claim = entry
        .claim()
        .filter(|claim| claim.disposition == ClaimDisposition::Active)?;
    let previous = entry.statement();
    let mut changes = Vec::new();
    changed(&mut changes, "title", &previous.title, &next.title);
    changed(
        &mut changes,
        "description",
        previous.description.as_deref().unwrap_or("-"),
        next.description.as_deref().unwrap_or("-"),
    );
    changed(
        &mut changes,
        "acceptance",
        &joined(&previous.acceptance),
        &joined(&next.acceptance),
    );
    changed(&mut changes, "priority", priority(previous), priority(next));
    changed(&mut changes, "project", &previous.project, &next.project);
    changed(
        &mut changes,
        "dependencies",
        &dependencies(&previous.dependencies),
        &dependencies(&next.dependencies),
    );
    (!changes.is_empty()).then_some(body::TaskEditNotice {
        handle: TaskHandle::for_project(&next.project, &next.born),
        lane: claim.lane,
        claim: claim.claimed,
        previous: previous.revision,
        changes,
    })
}

fn changed(changes: &mut Vec<body::TaskFieldEdit>, field: &str, before: &str, after: &str) {
    if before == after {
        return;
    }
    changes.push(body::TaskFieldEdit {
        field: field.to_owned(),
        before: bounded(before),
        after: bounded(after),
    });
}

fn joined(values: &[String]) -> String {
    if values.is_empty() {
        "-".to_owned()
    } else {
        values.join(" | ")
    }
}

fn dependencies(values: &[TaskStamp]) -> String {
    if values.is_empty() {
        "-".to_owned()
    } else {
        values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn priority(statement: &TaskHead) -> &str {
    statement
        .tags
        .iter()
        .find_map(|tag| tag.strip_prefix(board::PRIORITY_TAG_PREFIX))
        .unwrap_or("none")
}

fn bounded(value: &str) -> String {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut characters = value.chars();
    let mut bounded = characters
        .by_ref()
        .take(body::TASK_EDIT_FIELD_SUMMARY_CHARS)
        .collect::<String>();
    if characters.next().is_some() {
        bounded.pop();
        bounded.push('…');
    }
    bounded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_are_bounded_on_character_boundaries() {
        let value = "雪".repeat(body::TASK_EDIT_FIELD_SUMMARY_CHARS + 1);
        let summary = bounded(&value);
        assert_eq!(summary.chars().count(), body::TASK_EDIT_FIELD_SUMMARY_CHARS);
        assert!(summary.ends_with('…'));
    }
}
