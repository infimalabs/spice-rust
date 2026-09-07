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
    let before = bounded(before);
    let mut after = bounded(after);
    if before == after {
        after = changed_summary(after);
    }
    changes.push(body::TaskFieldEdit {
        field: field.to_owned(),
        before,
        after,
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

fn changed_summary(summary: String) -> String {
    const SUFFIX: &str = " [changed]";

    let retained = body::TASK_EDIT_FIELD_SUMMARY_CHARS - SUFFIX.chars().count();
    format!(
        "{}{}",
        summary.chars().take(retained).collect::<String>(),
        SUFFIX
    )
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

    #[test]
    fn changed_summaries_remain_distinct_after_whitespace_normalization() {
        let mut changes = Vec::new();
        changed(
            &mut changes,
            "description",
            "the old contract",
            "the  old contract",
        );

        let [change] = changes.as_slice() else {
            panic!("one actual contract change is retained");
        };
        assert_ne!(change.before, change.after);
        assert_eq!(change.after, "the old contract [changed]");
        assert!(change.after.chars().count() <= body::TASK_EDIT_FIELD_SUMMARY_CHARS);
    }
}
