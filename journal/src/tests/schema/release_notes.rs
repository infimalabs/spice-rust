use super::*;
use crate::{FactCause, JournalWrite};

#[test]
fn new_git_boundaries_update_typed_release_authority_in_the_fact_transaction() {
    let mut journal = authority_fixture();
    let mut write = JournalWrite::new(
        Actor::Operator,
        "task:1kKRRsDP".parse().expect("fixture origin"),
        instant(),
        wire::Producer::CommandDoor,
    );
    write
        .fact(
            wire::body::TaskDone {
                task: "1kKRRsDP".parse().expect("fixture task"),
                validation: "release authority fixture".to_owned(),
                disposition: None,
                terminal: None,
                git: Some(wire::body::GitBoundary {
                    head: LANDING.to_owned(),
                    integrated: LANDING.to_owned(),
                    upstream: None,
                    upstream_head: None,
                    local_commits: 1,
                }),
                source: wire::body::SourceBoundary::Proven { heads: Vec::new() },
            },
            FactCause::Existing(seq(2)),
        )
        .expect("terminal fact builds");
    journal.apply(write).expect("terminal fact commits");

    let authority = journal
        .read(|read| read.release_note_authority())
        .expect("release authority reads");
    assert_eq!(authority.landings.len(), 1);
    assert_eq!(authority.landings[0].commit, LANDING);
}

const LANDING: &str = "1111111111111111111111111111111111111111";

#[test]
fn retained_git_boundaries_cross_to_typed_release_authority() {
    let scratch = Scratch::new();
    let path = scratch.path().join("release-landings.sqlite3");
    let journal = authority_file_fixture(&path);
    restore_predecessor(&journal, 85);
    insert_terminal(&journal, 3, Some(2), LANDING);
    advance(&journal, 3);
    drop(journal);

    let journal = Journal::open(&path).expect("retained landing crosses");
    let authority = journal
        .read(|read| read.release_note_authority())
        .expect("release authority reads");

    assert_eq!(
        journal.schema_version().expect("schema reads"),
        SCHEMA_VERSION
    );
    assert_eq!(authority.landings.len(), 1);
    assert_eq!(authority.landings[0].commit, LANDING);
    assert_eq!(authority.landings[0].task.as_str(), "1kKRRsDP");
    assert_eq!(authority.landings[0].project, "journal");
    assert_eq!(authority.landings[0].title, "Lay journal baseline");
    assert!(authority.landings[0].contributes);
    journal.verify().expect("crossed authority verifies");
}

#[test]
fn ambiguous_retained_commit_refuses_before_schema_mutation() {
    let scratch = Scratch::new();
    let path = scratch.path().join("ambiguous-release-landings.sqlite3");
    let journal = authority_file_fixture(&path);
    restore_predecessor(&journal, 85);
    insert_terminal(&journal, 3, Some(2), LANDING);
    insert_terminal(&journal, 4, Some(3), LANDING);
    advance(&journal, 4);
    drop(journal);

    let error = Journal::open(&path).expect_err("ambiguous retained landing refuses");
    assert!(error.to_string().contains(LANDING));
    assert!(
        error
            .to_string()
            .contains(crate::release_notes::PRE_PROJECTION_RELEASE)
    );

    let connection = rusqlite::Connection::open(&path).expect("refused authority reopens raw");
    let version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("predecessor identity remains");
    let projected: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'release_landings')",
            [],
            |row| row.get(0),
        )
        .expect("projection absence reads");
    assert_eq!(version, 85);
    assert!(!projected);
}

fn insert_terminal(journal: &Journal, seq: u64, cause: Option<u64>, commit: &str) {
    insert_fact(
        journal,
        fact(
            seq,
            cause,
            FactKind::TaskDone,
            json!({
                "task": "1kKRRsDP",
                "validation": "release authority fixture",
                "git": {
                    "head": commit,
                    "integrated": commit,
                    "local_commits": 1
                },
                "source": {"proven": {"heads": []}}
            }),
        ),
    );
    journal
        .connection
        .execute(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) VALUES (?1, 'task:1kKRRsDP', ?2, 1, ?3)",
            rusqlite::params![
                i64::try_from(seq).expect("fixture sequence fits SQLite"),
                cause.map(|cause| i64::try_from(cause).expect("fixture cause fits SQLite")),
                i64::try_from(seq - 1).expect("fixture depth fits SQLite")
            ],
        )
        .expect("insert retained landing thread");
}

fn advance(journal: &Journal, seq: u64) {
    journal
        .connection
        .execute_batch(&format!(
            "UPDATE cursor_head SET seq = {seq} WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = {seq} WHERE singleton = 1;"
        ))
        .expect("advance retained cursors");
}
