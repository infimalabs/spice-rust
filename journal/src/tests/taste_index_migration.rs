use super::*;

#[test]
fn schema_thirty_two_backfills_taste_evidence() {
    let scratch = Scratch::new();
    let path = scratch.path().join("schema-thirty-two.sqlite3");
    let journal = authority_file_fixture(&path);
    insert_fact(
        &journal,
        fact(
            3,
            None,
            FactKind::TasteConfirmed,
            json!({"taste": "alpha", "locus": "fixture:confirmed"}),
        ),
    );
    insert_fact(
        &journal,
        fact(
            4,
            None,
            FactKind::TasteCleared,
            json!({"taste": "alpha", "locus": "fixture:cleared"}),
        ),
    );
    for position in 5..=20 {
        insert_fact(
            &journal,
            fact(
                position,
                None,
                FactKind::TasteReminded,
                serde_json::to_value(wire::body::TasteReminded {
                    directive: uuid(&format!("019dbfa2-a16d-7000-8000-{position:012}")),
                    key: format!("fixture-key-{position}"),
                    lane: uuid("019dbfa2-a16d-7000-8000-000000000021"),
                    taste: "alpha".to_owned(),
                    content: digest("a"),
                    occupant: uuid("019dbfa2-a16d-7000-8000-000000000022"),
                    compaction: position,
                    locus: wire::body::TranscriptLocus {
                        source: None,
                        member: 1,
                        offset: position,
                        length: 3,
                        event: 0,
                        line: 1,
                        column: 1,
                    },
                    text: "fixture reminder".to_owned(),
                    expires: instant(),
                })
                .expect("taste reminder encodes"),
            ),
        );
    }
    journal
        .connection
        .execute_batch(
            "DROP TABLE lane_seats; \
             DROP TABLE taste_judgments; \
             DROP TABLE taste_fires; \
             UPDATE cursor_head SET seq = 20 WHERE singleton = 1; \
             UPDATE journal_schema SET version = 32, state = 'finalized'; \
             PRAGMA user_version = 32;",
        )
        .expect("downgrade taste predicates to schema thirty-two");
    drop(journal);

    let migrated = Journal::open(&path).expect("schema thirty-two taste evidence migrates");
    assert_eq!(
        migrated.migration().map(|step| (step.from(), step.to())),
        Some((32, SCHEMA_VERSION))
    );
    let page = migrated
        .read(|read| read.taste_evidence_page("alpha", seq(20), seq(5), 12))
        .expect("backfilled taste evidence reads");
    assert_eq!((page.population, page.confirmed, page.cleared), (3, 1, 1));
    assert_eq!(
        page.rows
            .iter()
            .map(|row| row.envelope.seq)
            .collect::<Vec<_>>(),
        [seq(5), seq(4), seq(3)]
    );
}
