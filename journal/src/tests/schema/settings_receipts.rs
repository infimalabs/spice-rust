//! Retained global-settings answers cross from one operator identity, or a
//! directory whose entries carry no code, to the coded principal directory
//! used by the current command binding.

use super::*;

const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c3";
const NAMED_COMMAND: &str = "018f82d4-6f59-7d06-92e3-bce913116301";
const EARLY_COMMAND: &str = "018f82d4-6f59-7d06-92e3-bce913116302";
const PRINCIPAL: &str = "018f82d4-6f59-7d06-92e3-bce913116303";
const SECOND: &str = "018f82d4-6f59-7d06-92e3-bce913116304";
const CODELESS_COMMAND: &str = "018f82d4-6f59-7d06-92e3-bce913116305";

fn settings_command(id: &str, fast_mode: bool) -> Command<serde_json::Value> {
    Command {
        v: WireVersion::CURRENT,
        id: uuid(id),
        epoch: uuid(EPOCH),
        kind: CommandKind::GlobalSettingsSet,
        expected: Vec::new(),
        body: json!({"fast_mode": fast_mode}),
    }
}

/// Retain `command`'s answer carrying `field` beside its fast-mode value.
fn insert_settings_receipt(
    journal: &Journal,
    command: &Command<serde_json::Value>,
    field: Option<(&str, serde_json::Value)>,
) {
    let mut body =
        serde_json::Map::from_iter([("fast_mode".to_owned(), command.body["fast_mode"].clone())]);
    if let Some((name, value)) = field {
        body.insert(name.to_owned(), value);
    }
    let response = Response {
        v: WireVersion::CURRENT,
        id: command.id,
        epoch: command.epoch,
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(2),
                last: seq(2),
            }),
            body: serde_json::Value::Object(body),
        }),
    };
    journal
        .connection
        .execute(
            "INSERT INTO command_receipts(
                 command_id, actor, command_kind, request, evaluated_witnesses, response,
                 first_seq, last_seq, snapshot_seq, epoch
             ) VALUES (?1, 'operator', 'settings::set', ?2, ?3, ?4, 2, 2, NULL, ?5)",
            params![
                command.id.to_string(),
                command
                    .canonical_json()
                    .expect("canonical settings request")
                    .into_bytes(),
                wire::encode(&Vec::<Witness>::new()).expect("encode empty witnesses"),
                wire::encode(&response).expect("encode retired settings response"),
                command.epoch.to_string(),
            ],
        )
        .expect("insert retired settings receipt");
}

fn replay(
    journal: &Journal,
    command: Command<serde_json::Value>,
) -> wire::intent::GlobalSettingsAnswer {
    let admitted = command
        .admit(
            &wire::Accepted {
                wire: WireVersion::CURRENT,
                actor: Actor::Operator,
                epoch: uuid(EPOCH),
                release: "1.2.3".parse().expect("fixture release"),
            },
            &wire::principal::PrincipalRoles::default(),
        )
        .expect("settings retry admits");
    let Some(crate::CommandReplay::Exact(Outcome::Reply(reply))) = journal
        .read(|read| read.command_replay(&admitted))
        .expect("settings retry reads")
    else {
        panic!("retained settings command replays exactly");
    };
    let wire::intent::CommandAnswer::GlobalSettings(answer) = reply.body else {
        panic!("settings receipt keeps its answer binding");
    };
    answer
}

fn admit(journal: &Journal, at: u64, principal: &str, name: &str) {
    insert_fact(
        journal,
        fact(
            at,
            None,
            FactKind::PrincipalAdmitted,
            json!({
                "principal": principal,
                "profile": {"name": name},
                "roles": ["operator"],
            }),
        ),
    );
}

#[test]
fn retained_settings_receipts_cross_once_to_a_coded_principal_directory() {
    let scratch = Scratch::new();
    let path = scratch.path().join("settings-receipts.sqlite3");
    let journal = authority_file_fixture(&path);
    let named = settings_command(NAMED_COMMAND, true);
    let early = settings_command(EARLY_COMMAND, false);
    let codeless = settings_command(CODELESS_COMMAND, true);
    insert_settings_receipt(
        &journal,
        &named,
        Some((
            "operator",
            json!({
                "name": "Ada Example",
                "email": "ada@example.invalid",
                "initials": "AE",
                "gravatar": "ab".repeat(32),
            }),
        )),
    );
    insert_settings_receipt(&journal, &early, None);
    insert_settings_receipt(
        &journal,
        &codeless,
        Some((
            "principals",
            json!([{"principal": SECOND, "identity": {"name": "Grace Example"}}]),
        )),
    );
    admit(&journal, 3, PRINCIPAL, "Ada Example");
    admit(&journal, 4, SECOND, "Grace Example");
    restore_predecessor(&journal, 64);
    drop(journal);

    let journal = Journal::open(&path).expect("schema-64 settings receipts cross");
    assert_eq!(
        journal
            .migration()
            .map(|migration| (migration.from(), migration.to())),
        Some((64, SCHEMA_VERSION)),
    );
    let named = replay(&journal, named);
    assert!(named.fast_mode);
    let bootstrap = named
        .principals
        .principal(&Actor::Human(uuid(PRINCIPAL)))
        .expect("retired operator becomes the bootstrap principal");
    assert_eq!(bootstrap.code.to_string(), "3");
    assert_eq!(bootstrap.identity.name.as_deref(), Some("Ada Example"));
    assert_eq!(bootstrap.identity.initials.as_deref(), Some("AE"));

    let early = replay(&journal, early);
    assert!(!early.fast_mode);
    assert_eq!(early.principals.identity(&Actor::Operator), None);

    let codeless = replay(&journal, codeless);
    let second = codeless
        .principals
        .principal(&Actor::Human(uuid(SECOND)))
        .expect("a codeless entry keeps its principal");
    assert_eq!(second.code.to_string(), "4");
    assert_eq!(second.identity.name.as_deref(), Some("Grace Example"));
    drop(journal);

    let reopened = Journal::open(&path).expect("crossed authority reopens");
    assert_eq!(reopened.migration(), None, "the crossing runs exactly once");
}
