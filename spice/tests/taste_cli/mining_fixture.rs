use super::*;

#[derive(Clone, Copy)]
struct SeededCorrection {
    key: &'static str,
    text: &'static str,
    lane: u8,
    fanout: u8,
    source: u8,
    coordinate: u32,
    decline_reason: Option<&'static str>,
}

pub(super) fn assert_recurring_operator_corrections() {
    let fixture = Fixture::new();
    seed_mining_corrections(&fixture);
    let tracked_before = fs::read_to_string(fixture.root.join("spice-rust.toml")).unwrap();
    let resident = Resident::start(&fixture);

    let first = run_concurrent_miners(&fixture);
    let predecessor = assert_initial_mining_result(&fixture, &first, &tracked_before);
    resident.stop(&fixture);

    assert_later_mining_result(&fixture, &predecessor, &tracked_before);
}

fn run_concurrent_miners(fixture: &Fixture) -> wire::Reply<serde_json::Value> {
    let (left, right) = std::thread::scope(|scope| {
        let left = scope.spawn(|| fixture.invoke(&["--json", "taste", "mine"]));
        let right = scope.spawn(|| fixture.invoke(&["--json", "taste", "mine"]));
        (left.join().unwrap(), right.join().unwrap())
    });
    assert!(
        left.status.success(),
        "left miner failed: {}",
        spoken(&left)
    );
    assert!(
        right.status.success(),
        "right miner failed: {}",
        spoken(&right)
    );
    let left = json_reply::<serde_json::Value>(&left);
    let right = json_reply::<serde_json::Value>(&right);
    let mut standings = [
        left.body["findings"][0]["standing"].as_str().unwrap(),
        right.body["findings"][0]["standing"].as_str().unwrap(),
    ];
    standings.sort_unstable();
    assert_eq!(standings, ["filed", "reused-active"]);
    if left.body["findings"][0]["standing"] == "filed" {
        left
    } else {
        right
    }
}

fn assert_initial_mining_result(
    fixture: &Fixture,
    first: &wire::Reply<serde_json::Value>,
    tracked_before: &str,
) -> wire::TaskHandle {
    assert_eq!(first.body["threshold"], 3);
    assert_eq!(first.body["scanned"], 12);
    assert_eq!(first.body["excluded"], 5);
    assert_eq!(first.body["qualifying"], 1);
    assert_eq!(first.body["findings"][0]["recurrence"], 3);
    assert_eq!(first.body["findings"][0]["standing"], "filed");
    assert_eq!(
        first.body["findings"][0]["message"],
        "Do not use grep for repository searches; use rg instead."
    );

    let proposals = mined_proposals(fixture);
    assert_eq!(proposals.len(), 1);
    let description = proposals[0].description.as_deref().unwrap();
    assert!(description.contains("Recurrence count: 3"), "{description}");
    assert!(description.contains("\"Do not use grep for repository searches; use rg instead.\""));
    assert!(description.contains("(declined)"), "{description}");
    assert!(description.contains("NACK reason"), "{description}");
    for key in ["1kMine01", "1kMine02", "1kMine03"] {
        assert!(description.contains(key), "{description}");
    }
    for key in ["1kMine0A", "1kMine0B", "1kMine0C", "1kMine0D"] {
        assert!(!description.contains(key), "{description}");
    }
    for source in [uuid(0x40), uuid(0x41), uuid(0x42)] {
        assert!(
            description.contains(&format!("source {source}")),
            "{description}"
        );
    }
    assert!(!description.contains("PRIVATE KEY"), "{description}");
    assert!(!description.contains("credential:"), "{description}");
    assert_eq!(proposals[0].acceptance.len(), 3);

    let second = json_reply::<serde_json::Value>(&fixture.invoke(&["--json", "taste", "mine"]));
    assert_eq!(second.body["findings"][0]["standing"], "reused-active");
    assert_eq!(mined_proposals(fixture).len(), 1);
    assert_eq!(
        fs::read_to_string(fixture.root.join("spice-rust.toml")).unwrap(),
        tracked_before
    );
    first.body["findings"][0]["task"]
        .as_str()
        .unwrap()
        .parse::<wire::TaskHandle>()
        .unwrap()
}

fn assert_later_mining_result(
    fixture: &Fixture,
    predecessor: &wire::TaskHandle,
    tracked_before: &str,
) {
    complete_mined_proposal(fixture, predecessor);
    let completed_resident = Resident::start(fixture);
    let completed = json_reply::<serde_json::Value>(&fixture.invoke(&["--json", "taste", "mine"]));
    assert_eq!(
        completed.body["findings"][0]["standing"],
        "reused-completed"
    );
    assert_eq!(completed.body["findings"][0]["task"], predecessor.as_str());
    assert_eq!(mined_proposals(fixture).len(), 1);
    assert_eq!(
        fs::read_to_string(fixture.root.join("spice-rust.toml")).unwrap(),
        tracked_before
    );
    completed_resident.stop(fixture);

    seed_later_mining_correction(fixture);
    let restarted = Resident::start(fixture);
    let later = json_reply::<serde_json::Value>(&fixture.invoke(&["--json", "taste", "mine"]));
    assert_eq!(later.body["findings"][0]["standing"], "filed");
    assert_eq!(later.body["findings"][0]["recurrence"], 4);
    let successor = later.body["findings"][0]["task"]
        .as_str()
        .unwrap()
        .parse::<wire::TaskHandle>()
        .unwrap();
    assert_ne!(&successor, predecessor);
    let proposals = mined_proposals(fixture);
    assert_eq!(proposals.len(), 2);
    let successor_add = proposals
        .iter()
        .find(|added| added.task == successor.born().unwrap())
        .unwrap();
    assert_eq!(successor_add.depends, [predecessor.born().unwrap()]);
    assert!(
        successor_add
            .description
            .as_deref()
            .unwrap()
            .contains("Recurrence count: 4")
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("spice-rust.toml")).unwrap(),
        tracked_before
    );
    restarted.stop(fixture);
}

fn mined_proposals(fixture: &Fixture) -> Vec<wire::body::TaskAdded> {
    journal::JournalReader::open(&fixture.journal)
        .unwrap()
        .fact_snapshot()
        .unwrap()
        .facts()
        .iter()
        .filter_map(|fact| fact.fact.read::<wire::body::TaskAdded>())
        .filter(|added| added.project.starts_with(".maxim_proposal.taste."))
        .collect()
}

pub(super) fn seed_mining_corrections(fixture: &Fixture) {
    let bus = open_bus(fixture);
    let rows = mining_corrections();
    publish_mining_corrections(&bus, &rows);
    answer_mining_corrections(&bus, &rows);
    bus.shutdown().unwrap();
}

fn mining_corrections() -> [SeededCorrection; 12] {
    [
        seeded(
            "1kMine01",
            "Do not use grep for repository searches; use rg instead.",
            0x20,
            0x40,
            1,
        ),
        seeded(
            "1kMine02",
            "Prefer rg rather than grep for repository searches.",
            0x21,
            0x41,
            1,
        ),
        SeededCorrection {
            decline_reason: Some("repository search should use the indexed path"),
            ..seeded(
                "1kMine03",
                "Never reach for grep in repository searches; always use rg.",
                0x20,
                0x42,
                3,
            )
        },
        SeededCorrection {
            decline_reason: Some("credential: -----BEGIN PRIVATE KEY----- abcdefghijklmnop"),
            ..seeded(
                "1kMine04",
                "Always validate configuration with cargo metadata.",
                0x20,
                0x43,
                4,
            )
        },
        seeded(
            "1kMine05",
            "Status update: do not use grep while I continue working.",
            0x20,
            0x44,
            5,
        ),
        seeded(
            "1kMine06",
            "```diff\n-do not use grep\n+use rg\n```",
            0x20,
            0x45,
            6,
        ),
        seeded(
            "1kMine07",
            "Do not print -----BEGIN PRIVATE KEY----- abcdefghijklmnop.",
            0x20,
            0x46,
            7,
        ),
        seeded(
            "1kMine08",
            "🌶️ last maxim: do not use grep; use rg.",
            0x20,
            0x47,
            8,
        ),
        // A second lane records the same operator fanout in a distinct real
        // tape generation and at the same local coordinates.
        SeededCorrection {
            fanout: 0x40,
            ..seeded(
                "1kMine0A",
                "Do not use grep for repository searches; use rg instead.",
                0x22,
                0x49,
                1,
            )
        },
        seeded("1kMine0B", "Use alpha beta one.", 0x20, 0x50, 10),
        seeded("1kMine0C", "Use alpha beta gamma delta.", 0x20, 0x51, 11),
        seeded("1kMine0D", "Use gamma delta three.", 0x20, 0x52, 12),
    ]
}

fn publish_mining_corrections(bus: &Bus, rows: &[SeededCorrection]) {
    let sent_origin = "taste:mining-fixture".parse::<wire::Origin>().unwrap();
    let sent_at = "2030-04-12T19:10:00.000Z".parse().unwrap();
    let mut sent = JournalWrite::new(
        Actor::Operator,
        sent_origin.clone(),
        sent_at,
        Producer::CommandDoor,
    );
    for row in rows {
        let body = SteerSent {
            directive: directive(row.key),
            fanout: Some(uuid(row.fanout)),
            key: row.key.to_owned(),
            lane: uuid(row.lane),
            text: row.text.to_owned(),
            control: None,
            attachments: Vec::new(),
            expires: "2031-01-01T00:00:00.000Z".parse().unwrap(),
        };
        let pending = sent.fact(body.clone(), FactCause::Root).unwrap();
        let origin = sent_origin.clone();
        sent.steering_head(pending, move |seq| {
            steer::Ledger::sent_head(&body, origin, sent_at, seq).unwrap()
        });
    }
    let outcome = bus.commit(sent).unwrap();
    assert!(
        matches!(outcome, CommitOutcome::Committed(_)),
        "mining steering did not commit: {outcome:?}"
    );
}

fn answer_mining_corrections(bus: &Bus, rows: &[SeededCorrection]) {
    let acked_at = "2030-04-12T19:11:00.000Z".parse().unwrap();
    let mut acked = JournalWrite::new(
        Actor::Spice,
        "taste:mining-fixture".parse().unwrap(),
        acked_at,
        Producer::AckScanner,
    );
    for &row in rows {
        let published = bus
            .read(move |read| read.steering_head(directive(row.key)))
            .unwrap()
            .unwrap();
        if let Some(reason) = row.decline_reason {
            let body = SteerDeclined {
                directive: directive(row.key),
                key: row.key.to_owned(),
                lane: uuid(row.lane),
                epoch: uuid(0x2f),
                locus: mining_locus(row.source, row.coordinate),
                reason: reason.to_owned(),
            };
            let pending = acked
                .fact(body.clone(), FactCause::Existing(published.body().created))
                .unwrap();
            let next = steer::Ledger::transition_head(
                published.body(),
                steer::Steering::Declined(&body),
                acked_at,
            )
            .unwrap();
            acked.steering_head(pending, move |_| next);
        } else {
            let body = SteerAcked {
                directive: directive(row.key),
                key: row.key.to_owned(),
                lane: uuid(row.lane),
                epoch: uuid(0x2f),
                locus: mining_locus(row.source, row.coordinate),
            };
            let pending = acked
                .fact(body.clone(), FactCause::Existing(published.body().created))
                .unwrap();
            let next = steer::Ledger::transition_head(
                published.body(),
                steer::Steering::Acked(&body),
                acked_at,
            )
            .unwrap();
            acked.steering_head(pending, move |_| next);
        }
    }
    assert!(matches!(
        bus.commit(acked).unwrap(),
        CommitOutcome::Committed(_)
    ));
}

const fn seeded(
    key: &'static str,
    text: &'static str,
    lane: u8,
    source: u8,
    coordinate: u32,
) -> SeededCorrection {
    SeededCorrection {
        key,
        text,
        lane,
        fanout: source,
        source,
        coordinate,
        decline_reason: None,
    }
}

pub(super) fn complete_mined_proposal(fixture: &Fixture, proposal: &wire::TaskHandle) {
    let bus = open_bus(fixture);
    let completed_at = "2030-04-12T19:12:00.000Z".parse::<Instant>().unwrap();
    let mut write = JournalWrite::new(
        Actor::Operator,
        "taste:mining-review".parse().unwrap(),
        completed_at,
        Producer::CommandDoor,
    );
    let done = write
        .fact(
            wire::body::TaskDone {
                task: proposal.born().unwrap(),
                validation: "operator reviewed the first mined recurrence".to_owned(),
                terminal: None,
                git: None,
                source: wire::body::SourceBoundary::unrecorded(),
            },
            FactCause::Root,
        )
        .unwrap();
    write.workflow_head(proposal.born().unwrap(), done, move |_| {
        wire::WorkflowHead {
            flow: vec!["todo".parse().unwrap(), "review".parse().unwrap()],
            phase: "review".parse().unwrap(),
            status: "completed".to_owned(),
            review: "clean".to_owned(),
            assignment: None,
            deferral: None,
            completion: Some(completed_at),
        }
    });
    assert!(matches!(
        bus.commit(write).unwrap(),
        CommitOutcome::Committed(_)
    ));
    bus.shutdown().unwrap();
}

pub(super) fn seed_later_mining_correction(fixture: &Fixture) {
    let bus = open_bus(fixture);
    let key = "1kMine09";
    let sent_origin = "taste:mining-fixture".parse::<wire::Origin>().unwrap();
    let sent_at = "2030-04-12T19:13:00.000Z".parse().unwrap();
    let mut sent = JournalWrite::new(
        Actor::Operator,
        sent_origin.clone(),
        sent_at,
        Producer::CommandDoor,
    );
    let body = SteerSent {
        directive: directive(key),
        fanout: Some(uuid(0x53)),
        key: key.to_owned(),
        lane: uuid(0x20),
        text: "Do not run grep for repository searches; choose rg.".to_owned(),
        control: None,
        attachments: Vec::new(),
        expires: "2031-01-01T00:00:00.000Z".parse().unwrap(),
    };
    let pending = sent.fact(body.clone(), FactCause::Root).unwrap();
    sent.steering_head(pending, move |seq| {
        steer::Ledger::sent_head(&body, sent_origin, sent_at, seq).unwrap()
    });
    assert!(matches!(
        bus.commit(sent).unwrap(),
        CommitOutcome::Committed(_)
    ));
    let published = bus
        .read(move |read| read.steering_head(directive(key)))
        .unwrap()
        .unwrap();
    let acked_at = "2030-04-12T19:14:00.000Z".parse().unwrap();
    let mut acked = JournalWrite::new(
        Actor::Spice,
        "taste:mining-fixture".parse().unwrap(),
        acked_at,
        Producer::AckScanner,
    );
    let body = SteerAcked {
        directive: directive(key),
        key: key.to_owned(),
        lane: uuid(0x20),
        epoch: uuid(0x2f),
        locus: mining_locus(0x53, 9),
    };
    let pending = acked
        .fact(body.clone(), FactCause::Existing(published.body().created))
        .unwrap();
    let next =
        steer::Ledger::transition_head(published.body(), steer::Steering::Acked(&body), acked_at)
            .unwrap();
    acked.steering_head(pending, move |_| next);
    assert!(matches!(
        bus.commit(acked).unwrap(),
        CommitOutcome::Committed(_)
    ));
    bus.shutdown().unwrap();
}

pub(super) fn seed_unavailable_placeholder(fixture: &Fixture) -> u64 {
    let bus = open_bus(fixture);
    let at = "2030-04-12T19:15:00.000Z".parse().unwrap();
    let origin = "taste:mining-fixture".parse::<wire::Origin>().unwrap();
    let mut write = JournalWrite::new(Actor::Spice, origin.clone(), at, Producer::Judge);
    write
        .fact(
            TasteConfirmed {
                taste: "unavailable-fixture".to_owned(),
                locus: "test-only rewrite source".to_owned(),
            },
            FactCause::Root,
        )
        .unwrap();
    let commit = match bus.commit(write).unwrap() {
        CommitOutcome::Committed(commit) => commit,
        outcome => panic!("unavailable-evidence seed did not commit: {outcome:?}"),
    };
    bus.shutdown().unwrap();
    commit.last()
}

pub(super) fn rewrite_unavailable_ack(fixture: &Fixture, sequence: u64) {
    let at = "2030-04-12T19:15:00.000Z".parse().unwrap();
    let origin = "taste:mining-fixture".parse::<wire::Origin>().unwrap();
    // Authoritative commits prohibit an answer without its publication. This
    // process test deliberately rewrites one otherwise valid fixture fact to
    // emulate captured authority whose referenced evidence became unavailable
    // after resident admission; the miner must fault before it can file.
    let fact = wire::Fact::new(SteerAcked {
        directive: directive("1kMiss01"),
        key: "1kMiss01".to_owned(),
        lane: uuid(0x20),
        epoch: uuid(0x2f),
        locus: mining_locus(0x60, 1),
    });
    let opaque = wire::decode::<wire::AnyFact>(&wire::encode(&fact).unwrap()).unwrap();
    let envelope = wire::Envelope {
        v: wire::WireVersion::CURRENT,
        seq: wire::Seq::new(sequence).unwrap(),
        at,
        actor: Actor::Spice,
        origin,
        cause: None,
        fact: opaque,
    };
    let bytes = wire::encode(&envelope).unwrap();
    let connection = rusqlite::Connection::open(&fixture.journal).unwrap();
    let sequence = i64::try_from(sequence).expect("fixture sequence fits SQLite");
    assert_eq!(
        connection
            .execute(
                "UPDATE fact_envelopes SET kind = ?1, envelope = ?2 WHERE seq = ?3",
                rusqlite::params![wire::FactKind::SteerAcked.as_str(), bytes, sequence],
            )
            .unwrap(),
        1
    );
}

pub(super) fn restore_unavailable_placeholder(fixture: &Fixture, sequence: u64) {
    let fact = wire::Fact::new(TasteConfirmed {
        taste: "unavailable-fixture".to_owned(),
        locus: "test-only rewrite source".to_owned(),
    });
    let opaque = wire::decode::<wire::AnyFact>(&wire::encode(&fact).unwrap()).unwrap();
    let envelope = wire::Envelope {
        v: wire::WireVersion::CURRENT,
        seq: wire::Seq::new(sequence).unwrap(),
        at: "2030-04-12T19:15:00.000Z".parse().unwrap(),
        actor: Actor::Spice,
        origin: "taste:mining-fixture".parse().unwrap(),
        cause: None,
        fact: opaque,
    };
    let bytes = wire::encode(&envelope).unwrap();
    let connection = rusqlite::Connection::open(&fixture.journal).unwrap();
    let sequence = i64::try_from(sequence).expect("fixture sequence fits SQLite");
    assert_eq!(
        connection
            .execute(
                "UPDATE fact_envelopes SET kind = ?1, envelope = ?2 WHERE seq = ?3",
                rusqlite::params![wire::FactKind::TasteConfirmed.as_str(), bytes, sequence],
            )
            .unwrap(),
        1
    );
}
