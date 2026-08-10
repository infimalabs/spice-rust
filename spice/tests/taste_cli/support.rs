fn assert_report(fixture: &Fixture, report: &serde_json::Value) {
    assert_eq!(report["provenance"]["source"], "tracked");
    assert_eq!(report["provenance"]["present"], true);
    assert_eq!(
        report["provenance"]["location"],
        fixture.root.join("spice-rust.toml").display().to_string()
    );
    assert_eq!(report["provenance"]["digest"], report["digest"]);
    assert_eq!(
        report["tastes"]
            .as_array()
            .expect("taste rows")
            .iter()
            .map(|row| row["name"].as_str().expect("taste name"))
            .collect::<Vec<_>>(),
        ["zeta", "alpha"]
    );
    assert_eq!(
        report["tastes"][0]["words"],
        serde_json::json!(["wait*", "sleep*"])
    );
    assert_eq!(report["tastes"][0]["judgments"][0]["verdict"], "confirmed");
    assert_eq!(report["tastes"][0]["judgments"][0]["locus"], "tape:zeta");
    assert_eq!(report["tastes"][1]["judgments"][0]["verdict"], "cleared");
    assert_eq!(report["tastes"][1]["judgments"][0]["locus"], "tape:alpha");
    assert_eq!(
        report["tastes"][0]["judgments"][0]["at"],
        "2030-04-12T18:44:09.271Z"
    );
    assert!(
        report["tastes"]
            .as_array()
            .expect("taste rows")
            .iter()
            .all(|row| row["name"] != "retired")
    );
}

fn assert_review_plan(
    board: &BoardAnswer,
    answer: &TaskAnswer,
    title: &str,
    origin: &wire::Origin,
) {
    let task = board
        .tasks
        .iter()
        .find(|task| task.statement.body.handle == answer.task)
        .expect("planned task is on the current board");
    assert_eq!(task.presentation_title(), title);
    assert_eq!(task.statement.body.project, "taste.policy");
    assert_eq!(&task.statement.body.provenance, origin);
    assert!(!task.visibility.is_hidden());
    assert_eq!(
        task.workflow
            .body
            .flow
            .iter()
            .map(wire::Phase::as_str)
            .collect::<Vec<_>>(),
        ["todo", "review"]
    );
    assert_eq!(task.workflow.body.phase.as_str(), "todo");
    assert_eq!(task.workflow.body.status, "pending");
}

fn board(fixture: &Fixture) -> BoardAnswer {
    let output = fixture.invoke(&["board"]);
    assert!(output.status.success(), "board failed: {}", spoken(&output));
    let answer = wire::intent::AnswerShape::Board
        .decode(&output.stdout)
        .expect("board text body is typed");
    let CommandAnswer::Board(board) = answer else {
        panic!("board command returned another answer shape")
    };
    *board
}

fn seed_judgments(fixture: &Fixture) {
    fs::create_dir_all(fixture.journal.parent().expect("journal parent"))
        .expect("journal directory");
    let journal = journal::Journal::open(&fixture.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let mut write = JournalWrite::new(
        Actor::Spice,
        "taste:fixture".parse().expect("fixture origin"),
        "2030-04-12T18:44:09.271Z"
            .parse::<Instant>()
            .expect("fixture instant"),
        Producer::Judge,
    );
    write
        .fact(
            TasteConfirmed {
                taste: "zeta".to_owned(),
                locus: "tape:zeta".to_owned(),
            },
            FactCause::Root,
        )
        .expect("confirmed judgment");
    write
        .fact(
            TasteCleared {
                taste: "alpha".to_owned(),
                locus: "tape:alpha".to_owned(),
            },
            FactCause::Root,
        )
        .expect("cleared judgment");
    write
        .fact(
            TasteConfirmed {
                taste: "retired".to_owned(),
                locus: "tape:retired".to_owned(),
            },
            FactCause::Root,
        )
        .expect("retired judgment");
    assert!(matches!(
        bus.commit(write).expect("judgments reach writer"),
        CommitOutcome::Committed(_)
    ));
    bus.shutdown().expect("fixture bus stops");
}

fn seed_mining_corrections(fixture: &Fixture) {
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
        ("1kMine07", "API key secret: do not use grep."),
        ("1kMine08", "🌶️ last maxim: do not use grep; use rg."),
    ];
    let origin: wire::Origin = "taste:mining-fixture".parse().unwrap();
    let at = "2030-04-12T19:10:00.000Z".parse().unwrap();
    let mut sent = JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    for (key, text) in rows {
        append_sent(
            &mut sent,
            SteerSent {
                directive: directive(key),
                key: key.to_owned(),
                lane: uuid(0x20),
                text: text.to_owned(),
                control: None,
                attachments: Vec::new(),
                expires: "2031-01-01T00:00:00.000Z".parse().unwrap(),
            },
            origin.clone(),
            at,
        );
    }
    assert!(matches!(
        bus.commit(sent).unwrap(),
        CommitOutcome::Committed(_)
    ));
    let at = "2030-04-12T19:11:00.000Z".parse().unwrap();
    let mut acked = JournalWrite::new(Actor::Spice, origin, at, Producer::AckScanner);
    for (index, (key, _)) in rows.into_iter().enumerate() {
        if index == 2 {
            append_declined(
                &bus,
                &mut acked,
                SteerDeclined {
                    directive: directive(key),
                    key: key.to_owned(),
                    lane: uuid(0x20),
                    epoch: uuid(0x2f),
                    locus: mining_locus(index as u32 + 1),
                    reason: "repository search should use the indexed path".to_owned(),
                },
                at,
            );
        } else {
            append_acked(
                &bus,
                &mut acked,
                SteerAcked {
                    directive: directive(key),
                    key: key.to_owned(),
                    lane: uuid(0x20),
                    epoch: uuid(0x2f),
                    locus: mining_locus(index as u32 + 1),
                },
                at,
            );
        }
    }
    assert!(matches!(
        bus.commit(acked).unwrap(),
        CommitOutcome::Committed(_)
    ));
    bus.shutdown().unwrap();
}

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
    let origin: wire::Origin = "taste:mining-fixture".parse().unwrap();
    let at = "2030-04-12T19:13:00.000Z".parse().unwrap();
    let mut sent = JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    append_sent(
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
        origin.clone(),
        at,
    );
    assert!(matches!(
        bus.commit(sent).unwrap(),
        CommitOutcome::Committed(_)
    ));
    let at = "2030-04-12T19:14:00.000Z".parse().unwrap();
    let mut acked = JournalWrite::new(Actor::Spice, origin, at, Producer::AckScanner);
    append_acked(
        &bus,
        &mut acked,
        SteerAcked {
            directive: directive(key),
            key: key.to_owned(),
            lane: uuid(0x20),
            epoch: uuid(0x2f),
            locus: mining_locus(9),
        },
        at,
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
    commit(
        &bus,
        Producer::ResidentRecognition,
        "01",
        |write, origin, at| {
            write
                .fact(seated("claude"), FactCause::Root)
                .expect("lane seating");
            append_reminded(
                write,
                reminded(FIRST_DIRECTIVE, "zeta", uuid(0x21), REPEATED_PROSE, 10),
                origin,
                at,
            );
        },
    );
    escalate_and_acknowledge(&bus);
    commit(
        &bus,
        Producer::ResidentRecognition,
        "04",
        |write, origin, at| {
            append_reminded(
                write,
                reminded(RENEWED_DIRECTIVE, "zeta", uuid(0x22), REPEATED_PROSE, 20),
                origin.clone(),
                at,
            );
            append_reminded(
                write,
                reminded(ALPHA_DIRECTIVE, "alpha", uuid(0x22), "keep the alias", 20),
                origin,
                at,
            );
        },
    );
    commit(&bus, Producer::AckScanner, "05", |write, _, at| {
        append_declined(
            &bus,
            write,
            SteerDeclined {
                directive: directive(RENEWED_DIRECTIVE),
                key: RENEWED_DIRECTIVE.to_owned(),
                lane: uuid(0x20),
                epoch: uuid(0x2f),
                locus: locus(11),
                reason: "the wait is the observed completion event".to_owned(),
            },
            at,
        );
    });
    bus.shutdown().expect("fixture bus stops");
}

/// Escalate the first directive, then answer that same stable identity.
fn escalate_and_acknowledge(bus: &Bus) {
    commit(bus, Producer::BusTimer, "02", |write, _, at| {
        append_escalated(
            bus,
            write,
            SteerEscalated {
                directive: directive(FIRST_DIRECTIVE),
                key: FIRST_DIRECTIVE.to_owned(),
                lane: uuid(0x20),
                stage: wire::body::SteeringStage::Reminded,
            },
            at,
        );
    });
    commit(bus, Producer::AckScanner, "03", |write, _, at| {
        append_acked(
            bus,
            write,
            SteerAcked {
                directive: directive(FIRST_DIRECTIVE),
                key: FIRST_DIRECTIVE.to_owned(),
                lane: uuid(0x20),
                epoch: uuid(0x2f),
                locus: locus(9),
            },
            at,
        );
    });
}

/// Seed one reminder whose lane the durable stream never seated.
fn seed_unseated_reminder(fixture: &Fixture) {
    let bus = open_bus(fixture);
    commit(
        &bus,
        Producer::ResidentRecognition,
        "01",
        |write, origin, at| {
            append_reminded(
                write,
                reminded(UNSEATED_DIRECTIVE, "zeta", uuid(0x21), REPEATED_PROSE, 10),
                origin,
                at,
            );
        },
    );
    bus.shutdown().expect("fixture bus stops");
}

fn open_bus(fixture: &Fixture) -> Bus {
    let journal = journal::Journal::open(&fixture.journal).expect("fixture journal opens");
    Bus::start(journal).expect("fixture bus starts")
}

fn append_sent(write: &mut JournalWrite, body: SteerSent, origin: wire::Origin, at: Instant) {
    let pending = write
        .fact(body.clone(), FactCause::Root)
        .expect("sent steering fact builds");
    write.steering_head(pending, move |seq| {
        steer::Ledger::sent_head(&body, origin, at, seq).expect("sent steering head builds")
    });
}

fn append_reminded(
    write: &mut JournalWrite,
    body: TasteReminded,
    origin: wire::Origin,
    at: Instant,
) {
    let pending = write
        .fact(body.clone(), FactCause::Root)
        .expect("taste reminder fact builds");
    write.steering_head(pending, move |seq| {
        steer::Ledger::reminded_head(&body, origin, at, seq).expect("taste reminder head builds")
    });
}

fn append_acked(bus: &Bus, write: &mut JournalWrite, body: SteerAcked, at: Instant) {
    let current = steering_head(bus, &body.key);
    let next = steer::Ledger::transition_head(current.body(), steer::Steering::Acked(&body), at)
        .expect("acknowledgment steering head builds");
    let pending = write
        .fact(body, FactCause::Root)
        .expect("acknowledgment fact builds");
    write.steering_head(pending, move |_| next);
}

fn append_declined(bus: &Bus, write: &mut JournalWrite, body: SteerDeclined, at: Instant) {
    let current = steering_head(bus, &body.key);
    let next = steer::Ledger::transition_head(current.body(), steer::Steering::Declined(&body), at)
        .expect("decline steering head builds");
    let pending = write
        .fact(body, FactCause::Root)
        .expect("decline fact builds");
    write.steering_head(pending, move |_| next);
}

fn append_escalated(bus: &Bus, write: &mut JournalWrite, body: SteerEscalated, at: Instant) {
    let current = steering_head(bus, &body.key);
    let next =
        steer::Ledger::transition_head(current.body(), steer::Steering::Escalated(&body), at)
            .expect("escalation steering head builds");
    let pending = write
        .fact(body, FactCause::Root)
        .expect("escalation fact builds");
    write.steering_head(pending, move |_| next);
}

fn steering_head(bus: &Bus, key: &str) -> journal::JournalHead<wire::SteeringHead> {
    let key = key.to_owned();
    bus.read(move |read| read.steering_key(&key))
        .expect("steering index reads")
        .expect("fixture steering head exists")
}

/// Commit one batch of facts, which is why the producer is a parameter: a
/// registered body admits exactly one producer class, and these span three.
fn commit(
    bus: &Bus,
    producer: Producer,
    minute: &str,
    facts: impl FnOnce(&mut JournalWrite, wire::Origin, Instant),
) {
    let origin: wire::Origin = "taste:fixture".parse().expect("fixture origin");
    let at = format!("2030-04-12T19:{minute}:00.000Z")
        .parse::<Instant>()
        .expect("fixture instant");
    let mut write = JournalWrite::new(Actor::Spice, origin.clone(), at, producer);
    facts(&mut write, origin, at);
    assert!(matches!(
        bus.commit(write).expect("facts reach writer"),
        CommitOutcome::Committed(_)
    ));
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
