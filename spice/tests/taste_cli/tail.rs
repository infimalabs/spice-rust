fn complete_mined_proposal(fixture: &Fixture, task: &wire::TaskHandle) {
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

fn seed_later_mining_correction(fixture: &Fixture) {
    let bus = open_bus(fixture);
    let key = "1kMine09";
    let sent_at = "2030-04-12T19:13:00.000Z".parse().unwrap();
    let mining_origin = "taste:mining-fixture".parse::<wire::Origin>().unwrap();
    let mut sent = JournalWrite::new(
        Actor::Operator,
        mining_origin.clone(),
        sent_at,
        Producer::CommandDoor,
    );
    indexed_sent(
        &mut sent,
        SteerSent {
            directive: directive(key),
            key: key.to_owned(),
            lane: uuid(0x20),
            text: "Do not run grep for repository searches; choose rg.".to_owned(),
            control: None,
            attachments: Vec::new(),
            expires: "2031-01-01T00:00:00.000Z".parse().unwrap(),
        },
        FactCause::Root,
        mining_origin.clone(),
        sent_at,
    );
    assert!(matches!(
        bus.commit(sent).unwrap(),
        CommitOutcome::Committed(_)
    ));
    let answered_at = "2030-04-12T19:14:00.000Z".parse().unwrap();
    let mut acked = JournalWrite::new(
        Actor::Spice,
        mining_origin,
        answered_at,
        Producer::AckScanner,
    );
    indexed_acked(
        &mut acked,
        current_steering(&bus, directive(key)),
        SteerAcked {
            directive: directive(key),
            key: key.to_owned(),
            lane: uuid(0x20),
            epoch: uuid(0x2f),
            locus: mining_locus(9),
        },
        FactCause::Root,
        answered_at,
    );
    assert!(matches!(
        bus.commit(acked).unwrap(),
        CommitOutcome::Committed(_)
    ));
    bus.shutdown().unwrap();
}

/// Seed the live reminder evidence the report derives its fire rows from.
///
/// One lane is seated once with one vendor. `zeta` fires twice on the same
/// prose across two occupant generations and two compaction epochs; the first
/// directive escalates before it is acknowledged, and the second is declined.
/// `alpha` fires once and is left live.
fn seed_fires(fixture: &Fixture) {
    let bus = open_bus(fixture);
    commit(&bus, Producer::ResidentRecognition, "01", |write, at| {
        write
            .fact(seated("claude"), FactCause::Root)
            .expect("lane seating");
        indexed_reminder(
            write,
            reminded(FIRST_DIRECTIVE, "zeta", uuid(0x21), REPEATED_PROSE, 10),
            FactCause::Root,
            "taste:fixture".parse().expect("fixture origin"),
            at,
        );
    });
    escalate_and_acknowledge(&bus);
    commit(&bus, Producer::ResidentRecognition, "04", |write, at| {
        let origin = "taste:fixture"
            .parse::<wire::Origin>()
            .expect("fixture origin");
        indexed_reminder(
            write,
            reminded(RENEWED_DIRECTIVE, "zeta", uuid(0x22), REPEATED_PROSE, 20),
            FactCause::Root,
            origin.clone(),
            at,
        );
        indexed_reminder(
            write,
            reminded(ALPHA_DIRECTIVE, "alpha", uuid(0x22), "keep the alias", 20),
            FactCause::Root,
            origin,
            at,
        );
    });
    let renewed = current_steering(&bus, directive(RENEWED_DIRECTIVE));
    commit(&bus, Producer::AckScanner, "05", move |write, at| {
        indexed_declined(
            write,
            renewed,
            SteerDeclined {
                directive: directive(RENEWED_DIRECTIVE),
                key: RENEWED_DIRECTIVE.to_owned(),
                lane: uuid(0x20),
                epoch: uuid(0x2f),
                locus: locus(11),
                reason: "the wait is the observed completion event".to_owned(),
            },
            FactCause::Root,
            at,
        );
    });
    bus.shutdown().expect("fixture bus stops");
}

/// Escalate the first directive, then answer that same stable identity.
fn escalate_and_acknowledge(bus: &Bus) {
    let published = current_steering(bus, directive(FIRST_DIRECTIVE));
    commit(bus, Producer::BusTimer, "02", move |write, at| {
        indexed_escalated(
            write,
            published,
            SteerEscalated {
                directive: directive(FIRST_DIRECTIVE),
                key: FIRST_DIRECTIVE.to_owned(),
                lane: uuid(0x20),
                stage: wire::body::SteeringStage::Reminded,
            },
            FactCause::Root,
            at,
        );
    });
    let reminded = current_steering(bus, directive(FIRST_DIRECTIVE));
    commit(bus, Producer::AckScanner, "03", move |write, at| {
        indexed_acked(
            write,
            reminded,
            SteerAcked {
                directive: directive(FIRST_DIRECTIVE),
                key: FIRST_DIRECTIVE.to_owned(),
                lane: uuid(0x20),
                epoch: uuid(0x2f),
                locus: locus(9),
            },
            FactCause::Root,
            at,
        );
    });
}

/// Seed one reminder whose lane the durable stream never seated.
fn seed_unseated_reminder(fixture: &Fixture) {
    let bus = open_bus(fixture);
    commit(&bus, Producer::ResidentRecognition, "01", |write, at| {
        indexed_reminder(
            write,
            reminded(UNSEATED_DIRECTIVE, "zeta", uuid(0x21), REPEATED_PROSE, 10),
            FactCause::Root,
            "taste:fixture".parse().expect("fixture origin"),
            at,
        );
    });
    bus.shutdown().expect("fixture bus stops");
}

fn open_bus(fixture: &Fixture) -> Bus {
    let journal = journal::Journal::open(&fixture.journal).expect("fixture journal opens");
    Bus::start(journal).expect("fixture bus starts")
}

/// Commit one batch of facts, which is why the producer is a parameter: a
/// registered body admits exactly one producer class, and these span three.
fn commit(
    bus: &Bus,
    producer: Producer,
    minute: &str,
    facts: impl FnOnce(&mut JournalWrite, Instant),
) {
    let at = format!("2030-04-12T19:{minute}:00.000Z")
        .parse::<Instant>()
        .expect("fixture instant");
    let mut write = JournalWrite::new(
        Actor::Spice,
        "taste:fixture".parse().expect("fixture origin"),
        at,
        producer,
    );
    facts(&mut write, at);
    let outcome = bus.commit(write).expect("facts reach writer");
    assert!(
        matches!(outcome, CommitOutcome::Committed(_)),
        "fixture facts refused: {outcome:?}"
    );
}

fn current_steering(bus: &Bus, directive: CanonicalUuid) -> wire::SteeringHead {
    bus.read(move |read| read.steering_head(directive))
        .expect("fixture steering head reads")
        .expect("fixture steering head exists")
        .into_body()
}

fn indexed_sent(
    write: &mut JournalWrite,
    body: SteerSent,
    cause: FactCause,
    origin: wire::Origin,
    at: Instant,
) {
    let pending = write.fact(body.clone(), cause).expect("sent fact builds");
    write.steering_head(pending, move |created| {
        steer::Ledger::sent_head(&body, origin, at, created).expect("sent head builds")
    });
}

fn indexed_reminder(
    write: &mut JournalWrite,
    body: TasteReminded,
    cause: FactCause,
    origin: wire::Origin,
    at: Instant,
) {
    let pending = write
        .fact(body.clone(), cause)
        .expect("taste reminder fact builds");
    write.steering_head(pending, move |created| {
        steer::Ledger::reminded_head(&body, origin, at, created).expect("reminder head builds")
    });
}

fn indexed_escalated(
    write: &mut JournalWrite,
    current: wire::SteeringHead,
    body: SteerEscalated,
    cause: FactCause,
    at: Instant,
) {
    let next = steer::Ledger::transition_head(&current, steer::Steering::Escalated(&body), at)
        .expect("escalated head builds");
    let pending = write
        .fact(body, cause)
        .expect("steering escalation fact builds");
    write.steering_head(pending, move |_| next);
}

fn indexed_acked(
    write: &mut JournalWrite,
    current: wire::SteeringHead,
    body: SteerAcked,
    cause: FactCause,
    at: Instant,
) {
    let next = steer::Ledger::transition_head(&current, steer::Steering::Acked(&body), at)
        .expect("acknowledged head builds");
    let pending = write
        .fact(body, cause)
        .expect("steering acknowledgment fact builds");
    write.steering_head(pending, move |_| next);
}

fn indexed_declined(
    write: &mut JournalWrite,
    current: wire::SteeringHead,
    body: SteerDeclined,
    cause: FactCause,
    at: Instant,
) {
    let next = steer::Ledger::transition_head(&current, steer::Steering::Declined(&body), at)
        .expect("declined head builds");
    let pending = write
        .fact(body, cause)
        .expect("steering decline fact builds");
    write.steering_head(pending, move |_| next);
}

fn seated(vendor: &str) -> LaneSeated {
    LaneSeated {
        lane: uuid(0x20),
        lineage_id: uuid(0x20),
        capability: uuid(0x2e),
        name: Some("Ava".to_owned()),
        worktree: "/worktrees/lane-a".to_owned(),
        life: "drive".to_owned(),
        vendor: vendor.parse().expect("fixture vendor"),
    }
}

fn reminded(
    key: &str,
    taste: &str,
    occupant: CanonicalUuid,
    prose: &str,
    compaction: u64,
) -> TasteReminded {
    TasteReminded {
        directive: directive(key),
        key: key.to_owned(),
        lane: uuid(0x20),
        taste: taste.to_owned(),
        content: Digest::compute(Domain::TasteProse, prose.as_bytes()),
        occupant,
        compaction,
        locus: locus(compaction as u32),
        text: format!("[TASTE] {taste}"),
        expires: "2031-01-01T00:00:00.000Z".parse().expect("fixture expiry"),
    }
}

/// A distinct durable identity per key, so no two publications collide.
fn directive(key: &str) -> CanonicalUuid {
    let suffix = key.bytes().fold(0_u64, |value, byte| {
        value.wrapping_mul(131).wrapping_add(u64::from(byte))
    });
    format!("00000000-0000-4000-8000-{:012x}", suffix & 0xffff_ffff_ffff)
        .parse()
        .expect("fixture directive UUID")
}

fn uuid(tail: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{tail:02x}")
        .parse()
        .expect("fixture UUID")
}

fn locus(line: u32) -> TranscriptLocus {
    TranscriptLocus {
        member: 1,
        offset: 40,
        length: 80,
        event: 0,
        line,
        column: 1,
    }
}

fn mining_locus(index: u32) -> TranscriptLocus {
    TranscriptLocus {
        member: 5,
        offset: u64::from(index) * 100,
        length: 90,
        event: index,
        line: index,
        column: 1,
    }
}

fn text_reply<T: DeserializeOwned>(output: &Output) -> T {
    assert!(
        output.status.success(),
        "command failed: {}",
        spoken(output)
    );
    wire::decode(&output.stdout).expect("text reply body is typed")
}

fn json_reply<T: DeserializeOwned>(output: &Output) -> wire::Reply<T> {
    assert!(
        output.status.success(),
        "command failed: {}",
        spoken(output)
    );
    let Outcome::Reply(reply) =
        wire::decode::<Outcome<T>>(&output.stdout).expect("JSON outcome is typed")
    else {
        panic!("JSON command did not reply")
    };
    reply
}

fn kind_count(snapshot: &journal::FactSnapshot, kind: FactKind) -> usize {
    snapshot
        .facts()
        .iter()
        .filter(|fact| fact.fact.kind() == kind)
        .count()
}

fn task_add_requests(fixture: &Fixture) -> Vec<TaskAdd> {
    let connection = rusqlite::Connection::open(&fixture.journal).expect("receipt store opens");
    let mut statement = connection
        .prepare(concat!(
            "SELECT request FROM command_receipts ",
            "WHERE command_kind = 'task::add' ORDER BY first_seq"
        ))
        .expect("receipt query prepares");
    let rows = statement
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .expect("receipt query runs");
    rows.map(|row| {
        let request = row.expect("receipt row reads");
        wire::decode::<wire::Command<TaskAdd>>(&request)
            .expect("task-add receipt decodes")
            .body
    })
    .collect()
}

fn spoken(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
