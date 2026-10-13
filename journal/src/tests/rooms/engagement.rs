//! Working-group engagement as a Design's execution authorization.

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

/// One write by `actor` storing a group over `anchors` that grants every
/// output in `engaged` an engagement at the group fact.
fn store(
    journal: &mut Journal,
    actor: Actor,
    micros: i64,
    anchors: Vec<TaskStamp>,
    engaged: &[&TaskStamp],
) -> Commit {
    let heads: Vec<TaskHead> = engaged
        .iter()
        .map(|output| statement(journal, output))
        .collect();
    let mut write = write_as(actor);
    let fact = write
        .fact(group(micros, anchors), FactCause::Root)
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
fn only_an_operator_group_anchoring_the_controller_engages_each_output_once() {
    let mut journal = Journal::in_memory().expect("journal");
    let (design, output) = gated(&mut journal, 1, 2);
    let (other, unanchored) = gated(&mut journal, 3, 4);

    let lane = Actor::Lane(uuid("018f82d4-6f59-7d06-92e3-bce913116306"));
    let by_lane = store(&mut journal, lane, 1, vec![design.clone()], &[&output]);
    refused(&journal, by_lane);
    let stranded = store(&mut journal, Actor::Operator, 1, vec![design.clone()], &[]);
    refused(&journal, stranded);
    let foreign = store(
        &mut journal,
        Actor::Operator,
        1,
        vec![design.clone()],
        &[&output, &unanchored],
    );
    refused(&journal, foreign);

    store(
        &mut journal,
        Actor::Operator,
        1,
        vec![design.clone()],
        &[&output],
    )
    .expect("the operator engages the anchored Design's output");
    let granted = statement(&journal, &output).engagement;
    assert_eq!(granted, cursor(&journal));
    assert_eq!(statement(&journal, &unanchored).engagement, None);

    let dropped = rewrite(&mut journal, &output, None);
    refused(&journal, dropped);
    let elsewhere = Some(statement(&journal, &design).revision);
    let moved = rewrite(&mut journal, &output, elsewhere);
    refused(&journal, moved);
    rewrite(&mut journal, &output, granted).expect("an edit keeps the engagement");
    let minted = rewrite(&mut journal, &unanchored, granted);
    refused(&journal, minted);
    assert_eq!(statement(&journal, &other).engagement, None);
    journal.verify().expect("engaged history verifies");
}
