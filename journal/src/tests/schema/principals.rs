//! The crossing that adds the principal head index.

use rusqlite::types::Value;

use super::*;

fn table_rows(journal: &Journal, table: &str) -> Vec<Vec<Value>> {
    let mut statement = journal
        .connection
        .prepare(&format!("SELECT * FROM {table} ORDER BY 1"))
        .expect("prepare retained rows");
    let width = statement.column_count();
    statement
        .query_map([], |row| {
            (0..width)
                .map(|column| row.get::<_, Value>(column))
                .collect()
        })
        .expect("query retained rows")
        .collect::<Result<_, _>>()
        .expect("read retained rows")
}

#[test]
fn schema_58_crosses_to_an_empty_principal_index_with_every_fact_and_receipt_byte_identical() {
    let scratch = Scratch::new();
    let path = scratch.path().join("authority.sqlite3");
    let journal = authority_file_fixture(&path);
    restore_predecessor(&journal, 58);
    let facts = table_rows(&journal, "fact_envelopes");
    let receipts = table_rows(&journal, "command_receipts");
    assert!(
        !facts.is_empty() && !receipts.is_empty(),
        "the fixture retains facts and receipts to preserve"
    );
    drop(journal);

    let journal = Journal::open(&path).expect("schema-58 authority crosses forward");
    let migration = journal.migration().expect("the crossing is reported");
    assert_eq!((migration.from(), migration.to()), (58, SCHEMA_VERSION));
    assert_eq!(table_rows(&journal, "fact_envelopes"), facts);
    assert_eq!(table_rows(&journal, "command_receipts"), receipts);
    let principals = journal
        .read(|read| read.principal_heads())
        .expect("the crossed principal index reads");
    assert!(principals.is_empty(), "the crossing bears no principal");
    journal.verify().expect("crossed authority verifies");
    drop(journal);

    let reopened = Journal::open(&path).expect("current authority reopens");
    assert_eq!(reopened.migration(), None, "the crossing runs exactly once");
}
