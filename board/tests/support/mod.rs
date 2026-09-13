use board::{Entry, ProjectPolicy, Snapshot, TaskError};
use journal::Journal;
use wire::TaskStamp;

/// Build one cursor-consistent snapshot from an explicit fixture task set.
pub fn bounded_snapshot(
    journal: &Journal,
    projects: &ProjectPolicy,
    tasks: &[TaskStamp],
) -> Result<Snapshot, TaskError> {
    let (cursor, heads) = journal
        .read(|read| {
            let heads = tasks
                .iter()
                .map(|task| {
                    read.task_heads_for(task)?
                        .ok_or_else(|| journal::GuardError::Invalid(format!("missing {task}")))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok((read.cursor()?, heads))
        })
        .expect("fixture task generation");
    let entries = heads
        .into_iter()
        .map(|heads| {
            let (statement, workflow, claim, lease) = heads.into_bodies();
            Entry::new(statement, workflow, claim, lease, projects)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Snapshot::new(
        journal::SCHEMA_VERSION,
        cursor.map_or_else(wire::JournalCursor::empty, wire::JournalCursor::at),
        entries,
    )
    .expect("fixture snapshot schema"))
}
