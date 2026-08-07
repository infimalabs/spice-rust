use std::cell::Cell;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use wire::{Disposition, ResidentState};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: std::path::PathBuf,
    paths: RuntimePaths,
    bus: Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    anchor: Seq,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-taste-runtime-{name}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("fixture root");
        let paths = paths(&root);
        let bus =
            Bus::start(journal::Journal::open(&paths.journal).expect("persistent journal opens"))
                .expect("bus starts");
        let epoch = uuid(1);
        let lane = uuid(2);
        let occupant = uuid(3);

        let mut startup =
            write(Actor::Spice, epoch, Producer::CommandDoor).expect("startup proposal begins");
        let seated = startup
            .fact(wire::body::UpSeated { epoch }, FactCause::Root)
            .expect("resident seats");
        startup.residency_head(seated, move |_| ResidencyHead {
            epoch,
            state: ResidentState::Seated,
            lanes: Vec::new(),
            subscriptions: Vec::new(),
        });
        commit(&bus, startup).expect("startup commits");

        let observed = current_residency(&bus, epoch).expect("startup residency");
        let capability = uuid(4);
        let mut lane_start = write(Actor::Spice, epoch, Producer::ResidentRecognition)
            .expect("lane proposal begins");
        let lane_seated = lane_start
            .fact(
                wire::body::LaneSeated {
                    lane,
                    lineage_id: lane,
                    capability,
                    name: None,
                    worktree: "/fixture".to_owned(),
                    life: "drive".to_owned(),
                    vendor: "claude".parse().expect("vendor name"),
                },
                FactCause::Existing(observed.position()),
            )
            .expect("lane seats");
        let started = lane_start
            .fact(
                wire::body::LaneStarted {
                    lane,
                    occupant,
                    epoch,
                    group: 123,
                },
                FactCause::Pending(lane_seated),
            )
            .expect("occupant starts");
        lane_start.residency_head(started, move |_| ResidencyHead {
            epoch,
            state: ResidentState::Seated,
            lanes: vec![wire::LaneResidency {
                lane,
                lineage_id: lane,
                name: None,
                worktree: "/fixture".to_owned(),
                life: "drive".to_owned(),
                vendor: "claude".parse().expect("vendor name"),
                occupant: Some(occupant),
                lineage: vec![occupant],
                capabilities: vec![capability],
                session: None,
                disposition: "occupied".to_owned(),
            }],
            subscriptions: vec![wire::LineageSubscription {
                lineage_id: lane,
                policy: wire::intent::LineageLife::Drive,
                manual_projects: Vec::new(),
                auto_projects: Vec::new(),
            }],
        });
        let committed = commit(&bus, lane_start).expect("lane commits");
        let anchor = Seq::new(committed.last()).expect("nonzero anchor");

        Self {
            root,
            paths,
            bus,
            epoch,
            lane,
            occupant,
            anchor,
        }
    }

    fn recognize(&self, bag: &taste::Bag, record: &lane::capture::CaptureRecord) {
        super::recognize(
            &self.bus,
            &self.paths,
            self.epoch,
            bag,
            &steer::Config::default(),
            record,
        )
        .expect("capture recognizes");
    }

    fn reminders(&self) -> Vec<wire::body::TasteReminded> {
        reminders(&self.bus.fact_snapshot().expect("snapshot reads"))
    }

    fn compact(&mut self) -> Seq {
        let locus = listen::Locus::new(9, 400, 32);
        let mut proposal = write(Actor::Spice, self.epoch, Producer::ResidentRecognition)
            .expect("compaction proposal");
        proposal
            .fact(
                wire::body::LaneCompacted {
                    lane: self.lane,
                    occupant: self.occupant,
                    stream: "stdout".to_owned(),
                    member: locus.member,
                    offset: locus.offset,
                    length: locus.length,
                },
                FactCause::Existing(self.anchor),
            )
            .expect("compaction fact");
        let committed = commit(&self.bus, proposal).expect("compaction commits");
        self.anchor = Seq::new(committed.last()).expect("nonzero anchor");
        self.anchor
    }

    fn renew(&mut self, successor: CanonicalUuid) -> Seq {
        let observed = current_residency(&self.bus, self.epoch).expect("current residency");
        let mut next = observed.body().clone();
        let row = next.lanes.first_mut().expect("fixture lane");
        row.occupant = Some(successor);
        row.lineage.push(successor);
        let ancestor = self.occupant;
        let mut proposal = write(Actor::Spice, self.epoch, Producer::ResidentRecognition)
            .expect("renewal proposal");
        let renewed = proposal
            .fact(
                wire::body::LaneRenewed {
                    lane: self.lane,
                    occupant: successor,
                    ancestor,
                    epoch: self.epoch,
                    group: 124,
                },
                FactCause::Existing(observed.position()),
            )
            .expect("renewal fact");
        proposal.residency_head(renewed, move |_| next);
        let committed = commit(&self.bus, proposal).expect("renewal commits");
        self.occupant = successor;
        self.anchor = Seq::new(committed.last()).expect("nonzero anchor");
        self.anchor
    }

    fn restart(self) -> Self {
        let Self {
            root,
            paths,
            bus,
            epoch,
            lane,
            occupant,
            anchor,
        } = self;
        bus.shutdown().expect("predecessor bus stops");
        let bus =
            Bus::start(journal::Journal::open(&paths.journal).expect("persistent journal reopens"))
                .expect("successor bus starts");
        Self {
            root,
            paths,
            bus,
            epoch,
            lane,
            occupant,
            anchor,
        }
    }

    fn finish(self) {
        self.bus.shutdown().expect("bus stops");
        fs::remove_dir_all(self.root).expect("fixture cleanup");
    }
}

#[test]
fn two_tastes_in_one_prose_record_commit_two_ordinary_directives() {
    let fixture = Fixture::new("two-tastes");
    let bag = bag(&[
        ("polling", &["poll*"], "Wait for the terminal event."),
        ("fallbacks", &["fallback*"], "Keep one current path."),
    ]);
    let record = prose_record(
        fixture.lane,
        fixture.occupant,
        "Polling with a fallback is not completion.",
    );

    fixture.recognize(&bag, &record);

    let reminders = fixture.reminders();
    assert_eq!(reminders.len(), 2);
    assert_eq!(
        reminders
            .iter()
            .map(|body| body.taste.as_str())
            .collect::<Vec<_>>(),
        ["polling", "fallbacks"]
    );
    assert!(
        reminders
            .iter()
            .all(|body| body.compaction == fixture.anchor.get())
    );
    assert!(reminders.iter().all(|body| body.locus.event == 0));
    let snapshot = fixture.bus.fact_snapshot().expect("snapshot");
    let ledger = steer::Ledger::from_snapshot(&snapshot).expect("reminders replay");
    assert_eq!(ledger.pending(fixture.lane).len(), 2);
    let rendered = fs::read_to_string(steer::inbox::path(&fixture.paths.inbox, fixture.lane))
        .expect("post-commit inbox renders");
    assert!(rendered.contains("[TASTE] polling: Wait for the terminal event."));
    assert!(rendered.contains("[TASTE] fallbacks: Keep one current path."));
    fixture.finish();
}

#[test]
fn distinct_contents_in_separate_records_both_commit_in_one_suppression_epoch() {
    let fixture = Fixture::new("distinct-records");
    let bag = bag(&[
        ("polling", &["poll*"], "Wait."),
        ("fallbacks", &["fallback*"], "Keep one path."),
    ]);

    fixture.recognize(
        &bag,
        &prose_record(fixture.lane, fixture.occupant, "polling now"),
    );
    fixture.recognize(
        &bag,
        &prose_record(fixture.lane, fixture.occupant, "fallback now"),
    );

    let reminders = fixture.reminders();
    assert_eq!(reminders.len(), 2);
    assert_ne!(reminders[0].content, reminders[1].content);
    assert!(
        reminders
            .iter()
            .all(|body| body.compaction == fixture.anchor.get())
    );
    fixture.finish();
}

#[test]
fn duplicate_hits_records_and_restart_replay_create_one_reminder_in_the_epoch() {
    let fixture = Fixture::new("deduplicate");
    let bag = bag(&[(
        "polling",
        &["poll*", "polling"],
        "Wait for the terminal event.",
    )]);
    let first = prose_record(fixture.lane, fixture.occupant, "  POLLING now ");
    let duplicate = prose_record(fixture.lane, fixture.occupant, "polling   NOW");

    fixture.recognize(&bag, &first);
    fixture.recognize(&bag, &first);
    let fixture = fixture.restart();
    fixture.recognize(&bag.clone(), &duplicate);

    assert_eq!(fixture.reminders().len(), 1);
    fixture.finish();
}

#[test]
fn compaction_and_occupant_renewal_each_open_one_later_suppression_epoch() {
    let mut fixture = Fixture::new("epochs");
    let bag = bag(&[("polling", &["poll*"], "Wait.")]);
    let started = fixture.anchor;
    let original = prose_record(fixture.lane, fixture.occupant, "polling now");
    fixture.recognize(&bag, &original);
    let content = fixture.reminders()[0].content.clone();

    let compacted = fixture.compact();
    fixture.recognize(&bag, &original);
    let successor = uuid(5);
    let renewed = fixture.renew(successor);
    let successor_record = prose_record(fixture.lane, successor, "POLLING   now");
    fixture.recognize(&bag, &successor_record);

    let reminders = fixture.reminders();
    assert_eq!(reminders.len(), 3);
    assert!(reminders.iter().all(|body| body.content == content));
    assert_eq!(
        reminders
            .iter()
            .map(|body| body.compaction)
            .collect::<Vec<_>>(),
        [started.get(), compacted.get(), renewed.get()]
    );
    assert_eq!(reminders[2].occupant, successor);
    fixture.finish();
}

#[test]
fn quoted_echo_and_every_non_prose_surface_are_inert() {
    let fixture = Fixture::new("inert-surfaces");
    let bag = bag(&[("polling", &["poll*"], "Wait.")]);
    let echo = prose_record(
        fixture.lane,
        fixture.occupant,
        "> [TASTE] polling: polling is not completion",
    );
    fixture.recognize(&bag, &echo);

    let locus = listen::Locus::new(2, 80, 32);
    let typed = record(
        fixture.lane,
        fixture.occupant,
        listen::Decoded::Events(vec![
            listen::Event::Reasoning(listen::Reasoning {
                content: listen::ReasoningContent::Plain(vec!["polling".to_owned()]),
                locus,
            }),
            listen::Event::Tool(listen::Tool {
                call_id: "patch".to_owned(),
                moment: listen::ToolMoment::Call {
                    name: "file_change".to_owned(),
                    arguments: r#"{"patch":"polling"}"#.to_owned(),
                },
                locus,
            }),
            listen::Event::Usage(listen::Usage {
                input_tokens: 1,
                cached_input_tokens: 0,
                cache_write_input_tokens: 0,
                output_tokens: 1,
                reasoning_output_tokens: None,
                locus,
            }),
            listen::Event::Compaction(listen::Compaction { locus }),
            listen::Event::Failure(listen::Failure {
                message: "polling failed".to_owned(),
                locus,
            }),
        ]),
    );
    assert!(candidates(&bag, &typed).expect("typed scan").is_empty());
    let residue = record(
        fixture.lane,
        fixture.occupant,
        listen::Decoded::Residue(listen::Residue {
            dialect: listen::Dialect::Claude,
            locus,
            kind: listen::ResidueKind::Unknown {
                discriminator: "polling".to_owned(),
            },
        }),
    );
    assert!(candidates(&bag, &residue).expect("residue scan").is_empty());
    assert!(fixture.reminders().is_empty());
    fixture.finish();
}

#[test]
fn transaction_guard_retries_after_both_key_and_directive_command_races() {
    let fixture = Fixture::new("guard-races");
    let bag = bag(&[("polling", &["poll*"], "Wait.")]);
    let record = prose_record(fixture.lane, fixture.occupant, "polling now");
    let attempts = Cell::new(0_u8);

    recognize_with(
        &fixture.bus,
        &fixture.paths,
        fixture.epoch,
        &bag,
        &steer::Config::default(),
        &record,
        |bus, openings, proposal| {
            let attempt = attempts.get();
            attempts.set(attempt + 1);
            if attempt < 2 {
                let opening = &openings[0];
                let (directive, key) = if attempt == 0 {
                    (uuid(80), opening.key.to_string())
                } else {
                    (opening.directive, "00000000".to_owned())
                };
                append_send(
                    bus,
                    fixture.epoch,
                    fixture.lane,
                    directive,
                    &key,
                    fixture.anchor,
                );
            }
            commit(bus, proposal)
        },
        steering::publish_inbox,
    )
    .expect("guard retries from current authority");

    assert_eq!(attempts.get(), 3, "both collision guards forced a rebuild");
    assert_eq!(fixture.reminders().len(), 1);
    let snapshot = fixture.bus.fact_snapshot().expect("snapshot");
    assert_eq!(
        snapshot
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::SteerSent)
            .count(),
        2
    );
    steer::Ledger::from_snapshot(&snapshot).expect("all three directives replay distinctly");
    fixture.finish();
}

#[test]
fn transaction_guard_rebuilds_when_compaction_moves_after_recognition() {
    let fixture = Fixture::new("epoch-guard");
    let bag = bag(&[("polling", &["poll*"], "Wait.")]);
    let record = prose_record(fixture.lane, fixture.occupant, "polling now");
    let attempts = Cell::new(0_u8);
    let moved = Cell::new(0_u64);

    recognize_with(
        &fixture.bus,
        &fixture.paths,
        fixture.epoch,
        &bag,
        &steer::Config::default(),
        &record,
        |bus, _, proposal| {
            let attempt = attempts.get();
            attempts.set(attempt + 1);
            if attempt == 0 {
                moved.set(
                    append_compaction(
                        bus,
                        fixture.epoch,
                        fixture.lane,
                        fixture.occupant,
                        fixture.anchor,
                    )
                    .get(),
                );
            }
            commit(bus, proposal)
        },
        steering::publish_inbox,
    )
    .expect("epoch movement rebuilds");

    assert_eq!(attempts.get(), 2, "the stale epoch proposal was refused");
    assert_eq!(fixture.reminders()[0].compaction, moved.get());
    fixture.finish();
}

#[test]
fn transaction_guard_treats_a_concurrent_same_content_winner_as_success() {
    let fixture = Fixture::new("content-guard");
    let bag = bag(&[("polling", &["poll*"], "Wait.")]);
    let record = prose_record(fixture.lane, fixture.occupant, "polling now");
    let attempts = Cell::new(0_u8);

    recognize_with(
        &fixture.bus,
        &fixture.paths,
        fixture.epoch,
        &bag,
        &steer::Config::default(),
        &record,
        |bus, openings, proposal| {
            attempts.set(attempts.get() + 1);
            if attempts.get() == 1 {
                append_reminder(
                    bus,
                    fixture.epoch,
                    fixture.lane,
                    fixture.occupant,
                    fixture.anchor,
                    &openings[0].candidate,
                );
            }
            commit(bus, proposal)
        },
        steering::publish_inbox,
    )
    .expect("the concurrent winner suppresses retry work");

    assert_eq!(attempts.get(), 1);
    assert_eq!(
        fixture.reminders().len(),
        1,
        "same content and epoch has one durable winner"
    );
    fixture.finish();
}

#[test]
fn a_commit_failure_delivers_nothing_while_a_rendering_failure_keeps_authority() {
    let fixture = Fixture::new("failure-order");
    let bag = bag(&[("polling", &["poll*"], "Wait.")]);
    let record = prose_record(fixture.lane, fixture.occupant, "polling now");
    let published = Cell::new(false);

    let failed = recognize_with(
        &fixture.bus,
        &fixture.paths,
        fixture.epoch,
        &bag,
        &steer::Config::default(),
        &record,
        |_, _, _| Err(store_fault("taste::forced-commit", "fixture refusal")),
        |_, _, _, _| {
            published.set(true);
            Ok(())
        },
    );
    assert!(failed.is_err());
    assert!(!published.get(), "delivery ran before durable commit");
    assert!(fixture.reminders().is_empty());

    let durable_before_delivery = Cell::new(false);
    recognize_with(
        &fixture.bus,
        &fixture.paths,
        fixture.epoch,
        &bag,
        &steer::Config::default(),
        &record,
        |bus, _, proposal| commit(bus, proposal),
        |bus, _, _, _| {
            durable_before_delivery
                .set(!reminders(&bus.fact_snapshot().expect("delivery snapshot")).is_empty());
            Err(environment_fault(
                "taste::forced-rendering",
                "fixture unavailable",
            ))
        },
    )
    .expect("rendering is disposable");
    assert!(durable_before_delivery.get());
    assert_eq!(fixture.reminders().len(), 1);
    fixture.recognize(&bag, &record);
    assert_eq!(
        fixture.reminders().len(),
        1,
        "replaying after a failed rendering does not duplicate authority"
    );
    fixture.finish();
}

#[test]
fn emitted_reminders_ack_decline_resend_and_expire_as_ordinary_steering() {
    let fixture = Fixture::new("ordinary-steering");
    let bag = bag(&[
        ("polling", &["poll*"], "Wait."),
        ("fallbacks", &["fallback*"], "Keep one path."),
    ]);
    fixture.recognize(
        &bag,
        &prose_record(fixture.lane, fixture.occupant, "polling fallback"),
    );
    let snapshot = fixture.bus.fact_snapshot().expect("snapshot");
    let reminders = reminders(&snapshot);
    let mut disposed = steer::Ledger::from_snapshot(&snapshot).expect("ledger");
    disposed
        .absorb(steer::Steering::Acked(&wire::body::SteerAcked {
            directive: reminders[0].directive,
            key: reminders[0].key.clone(),
            lane: fixture.lane,
            epoch: fixture.epoch,
            locus: reminders[0].locus,
        }))
        .expect("semantic ACK retires reminder");
    disposed
        .absorb(steer::Steering::Declined(&wire::body::SteerDeclined {
            directive: reminders[1].directive,
            key: reminders[1].key.clone(),
            lane: fixture.lane,
            epoch: fixture.epoch,
            locus: reminders[1].locus,
            reason: "not applicable".to_owned(),
        }))
        .expect("semantic NACK retires reminder");
    assert_eq!(
        disposed.disposition(&reminders[0].key.parse().expect("key")),
        Some(Disposition::Acked)
    );
    assert_eq!(
        disposed.disposition(&reminders[1].key.parse().expect("key")),
        Some(Disposition::Declined)
    );

    let mut lifecycle = steer::Ledger::from_snapshot(&snapshot).expect("ledger");
    let climbed = uuid(90);
    lifecycle
        .absorb(steer::Steering::Resent(&wire::body::SteerResent {
            directive: climbed,
            key: "1kJK6M0V".to_owned(),
            ancestor: reminders[0].key.clone(),
            lane: fixture.lane,
            escalation: 1,
        }))
        .expect("reminder resends");
    lifecycle
        .absorb(steer::Steering::Lapsed(&wire::body::SteerLapsed {
            key: "1kJK6M0V".to_owned(),
            lane: fixture.lane,
            expires: reminders[0].expires,
        }))
        .expect("reminder lineage expires");
    assert_eq!(
        lifecycle.disposition(&"1kJK6M0V".parse().expect("key")),
        Some(Disposition::Lapsed)
    );
    fixture.finish();
}

fn bag(rows: &[(&str, &[&str], &str)]) -> taste::Bag {
    taste::Bag::new(
        rows.iter()
            .map(|(name, words, message)| {
                taste::Taste::new(
                    name.parse().expect("taste name"),
                    words.iter().copied(),
                    *message,
                )
                .expect("taste row")
            })
            .collect(),
    )
    .expect("taste bag")
}

fn prose_record(
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    text: &str,
) -> lane::capture::CaptureRecord {
    let locus = listen::Locus::new(7, 120, text.len() as u64);
    record(
        lane,
        occupant,
        listen::Decoded::Events(vec![listen::Event::Prose(listen::Prose {
            text: text.to_owned(),
            phase: Some("final".to_owned()),
            locus,
        })]),
    )
}

fn record(
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    decoded: listen::Decoded,
) -> lane::capture::CaptureRecord {
    lane::capture::CaptureRecord {
        lane,
        occupant,
        stream: vendor::RecordStream::Stdout,
        boundary: sill::RecordBoundary::Complete,
        locus: listen::Locus::new(7, 120, 64),
        declaration: None,
        decoded,
    }
}

fn reminders(snapshot: &journal::FactSnapshot) -> Vec<wire::body::TasteReminded> {
    snapshot
        .facts()
        .iter()
        .filter(|envelope| envelope.fact.kind() == wire::FactKind::TasteReminded)
        .map(|envelope| body(envelope).expect("reminder body"))
        .collect()
}

fn append_send(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    directive: CanonicalUuid,
    key: &str,
    cause: Seq,
) {
    let mut proposal =
        write(Actor::Operator, epoch, Producer::CommandDoor).expect("command proposal");
    proposal
        .fact(
            wire::body::SteerSent {
                directive,
                key: key.to_owned(),
                lane,
                text: "concurrent command".to_owned(),
                control: None,
                attachments: Vec::new(),
                expires: "2030-01-01T00:00:00.000Z".parse().expect("expiry"),
            },
            FactCause::Existing(cause),
        )
        .expect("command fact");
    commit(bus, proposal).expect("concurrent command commits");
}

fn append_compaction(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    cause: Seq,
) -> Seq {
    let mut proposal =
        write(Actor::Spice, epoch, Producer::ResidentRecognition).expect("compaction proposal");
    proposal
        .fact(
            wire::body::LaneCompacted {
                lane,
                occupant,
                stream: "stdout".to_owned(),
                member: 12,
                offset: 500,
                length: 32,
            },
            FactCause::Existing(cause),
        )
        .expect("compaction fact");
    Seq::new(commit(bus, proposal).expect("compaction commits").last()).expect("nonzero compaction")
}

fn append_reminder(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    anchor: Seq,
    candidate: &Candidate,
) {
    let mut proposal =
        write(Actor::Spice, epoch, Producer::ResidentRecognition).expect("reminder proposal");
    proposal
        .fact(
            wire::body::TasteReminded {
                directive: uuid(70),
                key: "00000000".to_owned(),
                lane,
                taste: candidate.taste.clone(),
                content: candidate.content.clone(),
                occupant,
                compaction: anchor.get(),
                locus: candidate.locus,
                text: candidate.text.clone(),
                expires: "2030-01-01T00:00:00.000Z".parse().expect("expiry"),
            },
            FactCause::Existing(anchor),
        )
        .expect("reminder fact");
    commit(bus, proposal).expect("concurrent reminder commits");
}

fn paths(root: &std::path::Path) -> RuntimePaths {
    RuntimePaths {
        worktree: root.to_path_buf(),
        common: root.to_path_buf(),
        runtime: root.join("runtime"),
        data: root.join("data"),
        journal: root.join("journal"),
        resident: root.join("resident"),
        canvas: root.join("canvas"),
        lane_canvas: root.join("lanes"),
        inbox: root.join("inbox"),
        degraded: root.join("degraded"),
        tape: root.join("tape"),
        stage: root.join("stage"),
    }
}

fn uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
        .parse()
        .expect("fixture UUID")
}
