use super::*;

#[derive(Clone, Copy)]
struct SeededCorrection {
    key: &'static str,
    text: &'static str,
    lane: u8,
    source: u8,
    coordinate: u32,
    decline_reason: Option<&'static str>,
}

pub(super) fn seed_mining_corrections(fixture: &Fixture) {
    let bus = open_bus(fixture);
    let rows = [
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
        // This is the same durable transcript source and exact locus as the
        // first correction, copied to another lane by fanout.
        seeded(
            "1kMine0A",
            "Do not use grep for repository searches; use rg instead.",
            0x22,
            0x40,
            1,
        ),
        seeded("1kMine0B", "Use alpha beta one.", 0x20, 0x50, 10),
        seeded("1kMine0C", "Use alpha beta gamma delta.", 0x20, 0x51, 11),
        seeded("1kMine0D", "Use gamma delta three.", 0x20, 0x52, 12),
    ];
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
    let acked_at = "2030-04-12T19:11:00.000Z".parse().unwrap();
    let mut acked = JournalWrite::new(
        Actor::Spice,
        "taste:mining-fixture".parse().unwrap(),
        acked_at,
        Producer::AckScanner,
    );
    for row in rows {
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
    bus.shutdown().unwrap();
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
