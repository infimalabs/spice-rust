use super::*;

const COMMAND: &str = "018f82d4-6f59-7d06-92e3-bce913116399";
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce913116398";
const LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116397";

#[test]
fn retained_lane_replies_migrate_quiet_liveness_without_reclassifying_current_states() {
    for kind in [
        "lane::read",
        "lane::arrange",
        "lane::retune",
        "team::configure",
        "team::arrange",
        "lane::recall",
    ] {
        for existing_speech in [false, true] {
            let predecessor = response(kind, "running-stale", existing_speech);
            let connection = fixture(kind, &predecessor);

            migrate(&connection).expect("retained lane reply migrates");

            let mut expected = response(kind, "running", existing_speech);
            let body = &mut expected["outcome"]["body"];
            let residency = if kind == "lane::recall" {
                &mut body["residency"]
            } else {
                body
            };
            if !existing_speech {
                for lane in residency["lanes"].as_array_mut().expect("lane rows") {
                    lane["speech"] = json!({"backend": "disabled"});
                }
            }
            let bytes = stored(&connection);
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes).expect("migrated JSON"),
                expected,
                "{kind}, existing speech: {existing_speech}"
            );
            migrate(&connection).expect("current reply remains admissible");
            assert_eq!(stored(&connection), bytes);
        }
    }
}

#[test]
fn unknown_liveness_refuses_without_rewriting_the_receipt() {
    let predecessor = response("lane::arrange", "unknown", false);
    let connection = fixture("lane::arrange", &predecessor);
    let before = stored(&connection);

    assert!(matches!(
        migrate(&connection),
        Err(OpenError::Migration(MigrationError::StoredContent {
            from: 29,
            to: 30,
            table: "command_receipts",
            ..
        }))
    ));
    assert_eq!(stored(&connection), before);
}

#[test]
fn earlier_lane_crossings_admit_retired_liveness_without_publishing_speech() {
    let predecessor = response("lane::arrange", "running-stale", false);
    let bytes = canonical_predecessor(&predecessor).expect("earlier reply crosses");
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).expect("predecessor JSON"),
        response("lane::arrange", "running", false)
    );
}

fn response(kind: &str, liveness: &str, existing_speech: bool) -> Value {
    let lanes = [liveness, "running", "working", "vacant"].map(|liveness| {
        let mut lane = json!({
            "lane": LANE,
            "team": LANE,
            "worktree": "/workspace",
            "life": "drive",
            "vendor": "codex",
            "lineage": [],
            "bound": false,
            "disposition": "occupied",
            "liveness": liveness
        });
        if existing_speech {
            lane["speech"] = json!({
                "backend": "system",
                "voice": "Selected voice",
                "words_per_minute": 180
            });
        }
        lane
    });
    let residency = json!({
        "epoch": EPOCH,
        "state": "seated",
        "topology_revision": 1,
        "lanes": lanes,
        "subscriptions": []
    });
    let body = if kind == "lane::recall" {
        json!({"barrier": 1, "repository": "/workspace", "residency": residency, "recent": []})
    } else {
        residency
    };
    serde_json::to_value(wire::Response {
        v: wire::WireVersion::CURRENT,
        id: COMMAND.parse().expect("command UUID"),
        epoch: EPOCH.parse().expect("epoch UUID"),
        outcome: wire::Outcome::Reply(wire::Reply {
            position: wire::ReplyPosition::Snapshot(wire::Seq::new(1).expect("positive cursor")),
            body,
        }),
    })
    .expect("response JSON")
}

fn fixture(kind: &str, document: &Value) -> Connection {
    let connection = Connection::open_in_memory().expect("in-memory receipt store");
    connection
        .execute_batch(
            "CREATE TABLE command_receipts (
                command_id TEXT PRIMARY KEY,
                command_kind TEXT NOT NULL,
                response BLOB NOT NULL
            ) STRICT;",
        )
        .expect("receipt schema");
    connection
        .execute(
            "INSERT INTO command_receipts VALUES (?1, ?2, ?3)",
            params![
                COMMAND,
                kind,
                serde_json::to_vec(document).expect("stored JSON")
            ],
        )
        .expect("store predecessor receipt");
    connection
}

fn stored(connection: &Connection) -> Vec<u8> {
    connection
        .query_row("SELECT response FROM command_receipts", [], |row| {
            row.get(0)
        })
        .expect("stored response")
}
