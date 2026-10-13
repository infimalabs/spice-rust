//! Current group organization and retained pre-cutover markers.

use super::*;

/// A controller and an output it constructs, so the controller's
/// authorization gates the output.
fn gated(journal: &mut Journal, controller: i64, output: i64) -> (TaskStamp, TaskStamp) {
    let controller = task(journal, controller, None);
    let born = TaskStamp::for_micros(20_000 + output).expect("task stamp");
    let preparation = TaskPreparation {
        construction: born.clone(),
        ..prepared_by(&controller)
    };
    let output = task(journal, output, Some(preparation));
    (controller, output)
}

/// One write by `actor` moving `design` into group 1 and optionally attempting
/// to add a retained marker to each output in `marked`.
fn enter(journal: &mut Journal, actor: Actor, design: &TaskStamp, marked: &[&TaskStamp]) -> Commit {
    let heads: Vec<TaskHead> = marked
        .iter()
        .map(|output| statement(journal, output))
        .collect();
    let mut write = write_as(actor);
    let fact = write
        .fact(moved(journal, design, Some(1)), FactCause::Root)
        .expect("group fact builds");
    for head in heads {
        write.task_head(fact, move |seq| TaskHead {
            engagement: Some(seq),
            revision: seq,
            ..head
        });
    }
    journal.apply(write)
}

/// Simulate the exact task-head bytes a pre-cutover journal already retained.
/// The marker names the operator group fact that grouped its controller.
fn retain_legacy_marker(journal: &Journal, output: &TaskStamp, marker: Seq) {
    let mut head = statement(journal, output);
    head.engagement = Some(marker);
    head.revision = marker;
    let body = wire::encode(&head).expect("legacy statement encodes");
    let position = i64::try_from(marker.get()).expect("fixture position fits SQLite");
    assert_eq!(
        journal
            .connection
            .execute(
                "UPDATE task_heads SET head_seq = ?1, body = ?2 WHERE stamp = ?3",
                rusqlite::params![position, body, output.as_str()],
            )
            .expect("legacy statement stores"),
        1
    );
}

/// Rewrite `output`'s statement with `engagement` and no other change.
fn rewrite(journal: &mut Journal, output: &TaskStamp, engagement: Option<Seq>) -> Commit {
    let head = statement(journal, output);
    let mut write = write();
    let fact = write
        .fact(
            wire::body::TaskEdited {
                task: output.clone(),
                revision: 2,
                dependencies: None,
                intent: None,
                evidence: None,
            },
            FactCause::Root,
        )
        .expect("edit fact builds");
    write.task_head(fact, move |revision| TaskHead {
        engagement,
        revision,
        ..head
    });
    journal.apply(write)
}

#[test]
fn group_moves_grant_no_markers_and_current_authority_refuses_retained_markers() {
    let mut journal = Journal::in_memory().expect("journal");
    let (design, output) = gated(&mut journal, 1, 2);
    let (_other, unanchored) = gated(&mut journal, 3, 4);

    let attempted = enter(&mut journal, Actor::Operator, &design, &[&output]);
    refused(&journal, attempted);
    enter(&mut journal, Actor::Operator, &design, &[])
        .expect("group organization needs no output authorization");
    assert_eq!(statement(&journal, &output).engagement, None);

    let marker = cursor(&journal).expect("group move has a position");
    retain_legacy_marker(&journal, &output, marker);
    let error = journal
        .verify()
        .expect_err("retained marker is not current authority");
    assert!(error.to_string().contains("4b805a018"), "{error}");

    let preserved = rewrite(&mut journal, &output, Some(marker));
    refused(&journal, preserved);
    let dropped = rewrite(&mut journal, &output, None);
    refused(&journal, dropped);
    let minted = rewrite(&mut journal, &unanchored, Some(marker));
    refused(&journal, minted);
}
