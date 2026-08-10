use super::*;

pub(super) fn seed_mining_corrections(fixture: &Fixture) {
    let bus = open_bus(fixture);
    let rows = [
        (
            "1kMine01",
            "Do not use grep for repository searches; use rg instead.",
        ),
        (
            "1kMine02",
            "Prefer rg rather than grep for repository searches.",
        ),
        (
            "1kMine03",
            "Never reach for grep in repository searches; always use rg.",
        ),
        (
            "1kMine04",
            "Always validate configuration with cargo metadata.",
        ),
        (
            "1kMine05",
            "Status update: do not use grep while I continue working.",
        ),
        ("1kMine06", "```diff\n-do not use grep\n+use rg\n```"),
        (
            "1kMine07",
            "Do not print -----BEGIN PRIVATE KEY----- abcdefghijklmnop.",
        ),
        ("1kMine08", "🌶️ last maxim: do not use grep; use rg."),
    ];
    let sent_origin = "taste:mining-fixture".parse::<wire::Origin>().unwrap();
    let sent_at = "2030-04-12T19:10:00.000Z".parse().unwrap();
    let mut sent = JournalWrite::new(
        Actor::Operator,
        sent_origin.clone(),
        sent_at,
        Producer::CommandDoor,
    );
    for (key, text) in rows {
        let body = SteerSent {
            directive: directive(key),
            key: key.to_owned(),
            lane: uuid(0x20),
            text: text.to_owned(),
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
    for (index, (key, _)) in rows.into_iter().enumerate() {
        let published = bus
            .read(move |read| read.steering_head(directive(key)))
            .unwrap()
            .unwrap();
        if matches!(index, 2 | 3) {
            let body = SteerDeclined {
                directive: directive(key),
                key: key.to_owned(),
                lane: uuid(0x20),
                epoch: uuid(0x2f),
                locus: mining_locus(index as u32 + 1),
                reason: if index == 2 {
                    "repository search should use the indexed path".to_owned()
                } else {
                    "credential: -----BEGIN PRIVATE KEY----- abcdefghijklmnop".to_owned()
                },
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
                directive: directive(key),
                key: key.to_owned(),
                lane: uuid(0x20),
                epoch: uuid(0x2f),
                locus: mining_locus(index as u32 + 1),
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

pub(super) fn complete_mined_proposal(fixture: &Fixture, task: &wire::TaskHandle) {
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
                task: task.born().unwrap(),
                validation: "operator reviewed the first mined recurrence".to_owned(),
                git: None,
                source: wire::body::SourceBoundary::unrecorded(),
            },
            FactCause::Root,
        )
        .unwrap();
    write.workflow_head(task.born().unwrap(), done, move |_| wire::WorkflowHead {
        flow: vec!["todo".parse().unwrap(), "review".parse().unwrap()],
        phase: "review".parse().unwrap(),
        status: "completed".to_owned(),
        review: "clean".to_owned(),
        assignment: None,
        deferral: None,
        completion: Some(completed_at),
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
        locus: mining_locus(9),
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
