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

/// Return `output` to a predecessor's record of an anchored Design's output,
/// which carried no engagement, at the schema before engagement.
fn as_predecessor(journal: &Journal, output: &TaskStamp) {
    let head = TaskHead {
        engagement: None,
        ..statement(journal, output)
    };
    journal
        .connection
        .execute(
            "UPDATE task_heads SET body = ?1 WHERE stamp = ?2",
            rusqlite::params![
                wire::encode(&head).expect("statement encodes"),
                output.as_str()
            ],
        )
        .expect("strip engagement");
    journal
        .connection
        .execute_batch(&format!(
            "UPDATE journal_schema SET version = {0} WHERE singleton = 1;
             PRAGMA user_version = {0};",
            SCHEMA_VERSION - 1
        ))
        .expect("restore predecessor identity");
}

#[test]
fn a_crossing_engages_each_output_a_retained_group_anchors_through_its_head() {
    let mut journal = Journal::in_memory().expect("journal");
    let (design, output) = gated(&mut journal, 1, 2);
    let (_, passive) = gated(&mut journal, 3, 4);
    store(
        &mut journal,
        Actor::Operator,
        1,
        vec![design.clone()],
        &[&output],
    )
    .expect("the operator anchors the Design");
    let granted = statement(&journal, &output).engagement;
    as_predecessor(&journal, &output);

    let migration = crate::migrate::admit(&mut journal.connection)
        .expect("the predecessor crosses")
        .expect("the crossing is reported");
    assert_eq!(
        (migration.from(), migration.to()),
        (SCHEMA_VERSION - 1, SCHEMA_VERSION)
    );
    assert_eq!(statement(&journal, &output).engagement, granted);
    assert_eq!(statement(&journal, &passive).engagement, None);
    rewrite(&mut journal, &output, granted).expect("the crossed output stays writable");
    journal.verify().expect("crossed history verifies");
}

#[test]
fn a_crossing_refuses_an_anchor_whose_group_head_is_no_operator_save() {
    let mut journal = Journal::in_memory().expect("journal");
    let (design, output) = gated(&mut journal, 1, 2);
    store(
        &mut journal,
        Actor::Operator,
        1,
        vec![design.clone()],
        &[&output],
    )
    .expect("the operator anchors the Design");
    as_predecessor(&journal, &output);
    journal
        .connection
        .execute(
            "UPDATE group_heads SET head_seq = ?1",
            [i64::try_from(statement(&journal, &design).revision.get()).expect("seq fits")],
        )
        .expect("point the group head at a task fact");
    let retained = statement(&journal, &output);

    let refusal = crate::migrate::admit(&mut journal.connection)
        .expect_err("an unreadable anchor refuses the crossing")
        .to_string();
    assert!(refusal.contains("298de2d58"), "{refusal}");
    assert_eq!(statement(&journal, &output), retained);
    let version: u32 = journal
        .connection
        .query_row("SELECT version FROM journal_schema", [], |row| row.get(0))
        .expect("identity reads");
    assert_eq!(
        version,
        SCHEMA_VERSION - 1,
        "the refusal leaves the identity"
    );
}
