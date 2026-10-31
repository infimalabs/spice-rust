use super::*;

const INDEX: &str = "task_heads_project_stamp";

fn index_exists(journal: &Journal) -> bool {
    journal
        .connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'index' AND name = ?1)",
            [INDEX],
            |row| row.get(0),
        )
        .expect("task-head project index presence reads")
}

fn task_head_rows(journal: &Journal) -> Vec<(String, i64, Vec<u8>)> {
    let mut statement = journal
        .connection
        .prepare("SELECT stamp, head_seq, body FROM task_heads ORDER BY stamp")
        .expect("task heads prepare");
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("task heads read")
        .collect::<Result<_, _>>()
        .expect("task heads decode")
}

#[test]
fn schema_104_project_index_retires_without_rewriting_task_heads() {
    let fresh = Journal::in_memory().expect("fresh journal creates the current schema");
    assert!(
        !index_exists(&fresh),
        "a fresh journal omits the retired index"
    );

    let scratch = Scratch::new();
    let path = scratch.path().join("project-index.sqlite3");
    let journal = authority_file_fixture(&path);
    journal
        .connection
        .execute_batch(
            "CREATE INDEX task_heads_project_stamp
             ON task_heads(json_extract(CAST(body AS TEXT), '$.project'), stamp);",
        )
        .expect("schema-104 project index is planted");
    let before = task_head_rows(&journal);
    assert!(
        !before.is_empty(),
        "the crossing fixture retains a task head"
    );
    restore_predecessor(&journal, 104);
    drop(journal);

    let crossed = Journal::open(&path).expect("schema-104 authority crosses");
    let migration = crossed.migration().expect("the crossing is reported");
    assert_eq!((migration.from(), migration.to()), (104, SCHEMA_VERSION));
    assert!(!index_exists(&crossed), "the retired index is dropped");
    assert_eq!(
        task_head_rows(&crossed),
        before,
        "task heads stay unchanged"
    );
}
