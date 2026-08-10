use std::cell::Cell;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::resident::write;
use steer::Standing as _;
use wire::intent::DeliveryStanding;
use wire::{Disposition, ResidentState};

#[path = "tests/fold.rs"]
mod fold;

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
        fs::create_dir_all(root.join(".git")).expect("fixture git directory");
        let paths = RuntimePaths::discover(&root).expect("runtime paths resolve");
        fs::create_dir_all(&paths.data).expect("fixture data directory");
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
            topology_revision: 0,
            lanes: Vec::new(),
            subscriptions: Vec::new(),
        });
        commit(&bus, startup).expect("startup commits");

        let observed = current_residency(&bus, epoch).expect("startup residency");
        let capability = uuid(4);
        let worktree = paths.worktree.to_string_lossy().into_owned();
        let mut lane_start = write(Actor::Spice, epoch, Producer::ResidentRecognition)
            .expect("lane proposal begins");
        let lane_seated = lane_start
            .fact(
                wire::body::LaneSeated {
                    lane,
                    lineage_id: lane,
                    capability,
                    name: None,
                    worktree: worktree.clone(),
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
            topology_revision: 1,
            lanes: vec![wire::LaneResidency {
                lane,
                lineage_id: lane,
                name: None,
                worktree,
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
            None,
            record,
        )
        .expect("capture recognizes");
    }

    fn adjudicate(
        &self,
        bag: &taste::Bag,
        judge: &mut dyn Adjudicator,
        record: &lane::capture::CaptureRecord,
    ) {
        super::recognize(
            &self.bus,
            &self.paths,
            self.epoch,
            bag,
            &steer::Config::default(),
            Some(judge),
            record,
        )
        .expect("capture adjudicates");
    }

    fn reminders(&self) -> Vec<wire::body::TasteReminded> {
        reminders(&self.bus.fact_snapshot().expect("snapshot reads"))
    }

    fn judgments(&self) -> Vec<wire::body::TasteJudged> {
        judgments(&self.bus.fact_snapshot().expect("snapshot reads"))
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
    let calls = Cell::new(0_u8);
    let mut judge = FunctionAdjudicator(|_: &Candidate| {
        calls.set(calls.get() + 1);
        Ok(judgment(
            true,
            vec![attempt(0, "YES", wire::body::TasteJudgeReply::Yes)],
        ))
    });
    fixture.adjudicate(&bag, &mut judge, &duplicate);
    assert_eq!(
        calls.get(),
        0,
        "the direct evidence suppresses later judging"
    );
    assert!(fixture.judgments().is_empty());
    assert_eq!(fixture.reminders().len(), 1);
    fixture.finish();
}

#[test]
fn enabled_confirmation_records_complete_evidence_and_opens_one_restart_safe_directive() {
    let fixture = Fixture::new("judge-confirmed");
    let bag = bag(&[("polling", &["poll*"], "Wait for the terminal event.")]);
    let record = prose_record(fixture.lane, fixture.occupant, "Polling is not completion.");
    let calls = Cell::new(0_u8);
    let mut judge = FunctionAdjudicator(|candidate: &Candidate| {
        calls.set(calls.get() + 1);
        assert_eq!(candidate.taste, "polling");
        assert_eq!(candidate.trigger, "poll*");
        assert_eq!(candidate.driver, "Wait for the terminal event.");
        assert_eq!(candidate.source, "Polling is not completion.");
        Ok(judgment(
            true,
            vec![
                attempt(2, "perhaps", wire::body::TasteJudgeReply::Ambiguous),
                attempt(0, "YES", wire::body::TasteJudgeReply::Yes),
            ],
        ))
    });

    fixture.adjudicate(&bag, &mut judge, &record);

    assert_eq!(calls.get(), 1);
    assert!(fixture.reminders().is_empty(), "enabled mode uses one fact");
    let judgments = fixture.judgments();
    assert_eq!(judgments.len(), 1);
    let evidence = &judgments[0];
    assert_eq!(evidence.bag, taste::bag_identity(&bag));
    assert_eq!(evidence.trigger, "poll*");
    assert_eq!(evidence.driver, "Wait for the terminal event.");
    assert_eq!(evidence.source, "Polling is not completion.");
    assert_eq!(evidence.compaction, fixture.anchor.get());
    assert_eq!(evidence.attempts.len(), 2);
    let wire::body::TasteDecision::Confirmed { key, text, .. } = &evidence.decision else {
        panic!("positive judgment was not confirmed");
    };
    assert_eq!(text, "[TASTE] polling: Wait for the terminal event.");
    let snapshot = fixture.bus.fact_snapshot().expect("snapshot");
    let ledger = steer::Ledger::from_snapshot(&snapshot).expect("judgment replays");
    assert_eq!(ledger.pending(fixture.lane).len(), 1);
    assert_eq!(ledger.pending(fixture.lane)[0].key().to_string(), *key);
    assert_eq!(wire::FactKind::TasteJudged.producer(), Producer::Judge);

    let fixture = fixture.restart();
    fixture.adjudicate(&bag, &mut judge, &record);
    fixture.recognize(&bag, &record);
    assert_eq!(calls.get(), 1, "restart replay did not re-judge evidence");
    assert_eq!(fixture.judgments().len(), 1);
    assert!(
        fixture.reminders().is_empty(),
        "a disabled restart respects the durable judgment"
    );
    assert_eq!(
        steer::Ledger::from_snapshot(&fixture.bus.fact_snapshot().expect("snapshot"))
            .expect("restart judgment replays")
            .pending(fixture.lane)
            .len(),
        1,
    );
    fixture.finish();
}

#[test]
fn enabled_clearance_is_durable_evidence_and_never_opens_steering() {
    let fixture = Fixture::new("judge-cleared");
    let bag = bag(&[("fallbacks", &["fallback*"], "Keep one path.")]);
    let record = prose_record(
        fixture.lane,
        fixture.occupant,
        "This paragraph merely mentions fallback terminology.",
    );
    let calls = Cell::new(0_u8);
    let mut judge = FunctionAdjudicator(|_: &Candidate| {
        calls.set(calls.get() + 1);
        Ok(judgment(
            false,
            vec![attempt(1, "NO", wire::body::TasteJudgeReply::No)],
        ))
    });

    fixture.adjudicate(&bag, &mut judge, &record);
    fixture.adjudicate(&bag, &mut judge, &record);

    assert_eq!(calls.get(), 1, "durable clearance suppresses replay");
    let judgments = fixture.judgments();
    assert_eq!(judgments.len(), 1);
    assert!(matches!(
        judgments[0].decision,
        wire::body::TasteDecision::Cleared
    ));
    assert!(fixture.reminders().is_empty());
    let ledger = steer::Ledger::from_snapshot(&fixture.bus.fact_snapshot().expect("snapshot"))
        .expect("clearance replays");
    assert!(ledger.pending(fixture.lane).is_empty());
    let fixture = fixture.restart();
    fixture.recognize(&bag, &record);
    assert!(
        fixture.reminders().is_empty(),
        "a disabled restart respects the durable clearance"
    );
    assert!(
        steer::Ledger::from_snapshot(&fixture.bus.fact_snapshot().expect("snapshot"))
            .expect("clearance still replays")
            .pending(fixture.lane)
            .is_empty()
    );
    fixture.finish();
}

#[test]
fn unavailable_judge_abandons_candidate_without_fact_or_reminder() {
    let fixture = Fixture::new("judge-unavailable");
    let bag = bag(&[("polling", &["poll*"], "Wait.")]);
    let record = prose_record(fixture.lane, fixture.occupant, "polling now");
    let mut judge = FunctionAdjudicator(|_: &Candidate| {
        Err(crate::judge::failure_error(
            "unavailable",
            "fixture judge is absent",
        ))
    });

    fixture.adjudicate(&bag, &mut judge, &record);

    assert!(fixture.judgments().is_empty());
    assert!(fixture.reminders().is_empty());
    assert!(
        steer::Ledger::from_snapshot(&fixture.bus.fact_snapshot().expect("snapshot"))
            .expect("empty judgment replay")
            .pending(fixture.lane)
            .is_empty()
    );
    fixture.finish();
}

#[test]
fn taste_framings_are_complete_and_deterministically_shuffled() {
    let bag = bag(&[("polling", &["poll*"], "Wait.")]);
    let candidate = (0..128)
        .find_map(|index| {
            let record = prose_record(uuid(2), uuid(3), &format!("polling candidate {index}"));
            let candidate = candidates(&bag, &record).expect("candidate scan").remove(0);
            let order = judge_prompts(&candidate)
                .iter()
                .map(crate::judge::Prompt::framing)
                .collect::<Vec<_>>();
            (order != [0, 1, 2]).then_some(candidate)
        })
        .expect("at least one digest shuffles the framing order");

    let first = judge_prompts(&candidate);
    let second = judge_prompts(&candidate);
    assert_eq!(first, second, "same evidence has one framing order");
    let mut identities = first
        .iter()
        .map(crate::judge::Prompt::framing)
        .collect::<Vec<_>>();
    identities.sort_unstable();
    assert_eq!(identities, [0, 1, 2]);
    assert!(first.iter().all(|prompt| {
        prompt.text().contains(&candidate.source)
            && prompt.text().contains(&candidate.trigger)
            && prompt.text().contains(&candidate.driver)
    }));
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
        listen::Decoded::Events {
            context: Default::default(),
            events: vec![
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
                    turns: 1,
                    input_tokens: 1,
                    cached_input_tokens: 0,
                    cache_write_input_tokens: 0,
                    output_tokens: 1,
                    reasoning_output_tokens: None,
                    locus,
                }),
                listen::Event::Compaction(listen::Compaction {
                    summary: None,
                    locus,
                }),
                listen::Event::Failure(listen::Failure {
                    message: "polling failed".to_owned(),
                    locus,
                }),
            ],
        },
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
fn generated_patch_and_diff_bodies_never_become_taste_candidates() {
    let bag = bag(&[("polling", &["poll*"], "Wait.")]);
    let lane = uuid(90);
    let occupant = uuid(91);
    let patch = prose_record(
        lane,
        occupant,
        "*** Begin Patch\n+polling forever\n*** End Patch",
    );
    let mixed = prose_record(
        lane,
        occupant,
        "Authored delivery.\ndiff --git a/a b/a\n@@ -1 +1 @@\n-pause\n+polling",
    );
    let authored = prose_record(
        lane,
        occupant,
        "Polling is discussed here.\ndiff --git a/a b/a\n+polling",
    );
    let tail = prose_record(
        lane,
        occupant,
        "*** Begin Patch\n+polling forever\n*** End Patch\nPolling after the patch is authored.",
    );

    assert!(candidates(&bag, &patch).expect("patch scans").is_empty());
    assert!(
        candidates(&bag, &mixed)
            .expect("mixed diff scans")
            .is_empty()
    );
    assert_eq!(
        candidates(&bag, &authored)
            .expect("authored prefix scans")
            .len(),
        1,
        "only the authored prefix remains eligible"
    );
    assert_eq!(
        candidates(&bag, &tail)
            .expect("post-patch authored tail scans")
            .len(),
        1,
        "authored text after a complete generated block remains eligible"
    );
}

#[test]
fn emitted_reminders_ack_decline_escalate_and_expire_as_ordinary_steering() {
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
    lifecycle
        .absorb(steer::Steering::Escalated(&wire::body::SteerEscalated {
            directive: reminders[0].directive,
            key: reminders[0].key.clone(),
            lane: fixture.lane,
            stage: wire::body::SteeringStage::Reminded,
        }))
        .expect("reminder escalates");
    lifecycle
        .absorb(steer::Steering::Lapsed(&wire::body::SteerLapsed {
            key: reminders[0].key.clone(),
            lane: fixture.lane,
            expires: reminders[0].expires,
        }))
        .expect("stable reminder expires");
    assert_eq!(
        lifecycle.disposition(&reminders[0].key.parse().expect("key")),
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
        listen::Decoded::Events {
            context: Default::default(),
            events: vec![listen::Event::Prose(listen::Prose {
                role: listen::ProseRole::Assistant,
                text: text.to_owned(),
                phase: Some("final".to_owned()),
                media: Vec::new(),
                locus,
            })],
        },
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
        .filter_map(|envelope| envelope.fact.read())
        .collect()
}

fn judgments(snapshot: &journal::FactSnapshot) -> Vec<wire::body::TasteJudged> {
    snapshot
        .facts()
        .iter()
        .filter_map(|envelope| envelope.fact.read())
        .collect()
}

fn judgment(confirmed: bool, attempts: Vec<wire::body::TasteJudgeAttempt>) -> Judgment {
    Judgment {
        confirmed,
        attempts,
    }
}

fn attempt(
    framing: u8,
    reply: &str,
    verdict: wire::body::TasteJudgeReply,
) -> wire::body::TasteJudgeAttempt {
    wire::body::TasteJudgeAttempt {
        framing,
        reply: reply.to_owned(),
        verdict,
    }
}

fn append_send(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    directive: CanonicalUuid,
    key: &str,
    cause: Seq,
) {
    let at = current_instant().expect("command instant");
    let origin: wire::Origin = format!("resident:{epoch}").parse().expect("command origin");
    let mut proposal =
        JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    let body = wire::body::SteerSent {
        directive,
        key: key.to_owned(),
        lane,
        text: "concurrent command".to_owned(),
        control: None,
        attachments: Vec::new(),
        expires: "2030-01-01T00:00:00.000Z".parse().expect("expiry"),
    };
    let pending = proposal
        .fact(body.clone(), FactCause::Existing(cause))
        .expect("command fact");
    proposal.steering_head(pending, move |seq| {
        steer::Ledger::sent_head(&body, origin, at, seq).expect("command head")
    });
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
    let at = current_instant().expect("reminder instant");
    let origin: wire::Origin = format!("resident:{epoch}")
        .parse()
        .expect("reminder origin");
    let mut proposal = JournalWrite::new(
        Actor::Spice,
        origin.clone(),
        at,
        Producer::ResidentRecognition,
    );
    let body = wire::body::TasteReminded {
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
    };
    let pending = proposal
        .fact(body.clone(), FactCause::Existing(anchor))
        .expect("reminder fact");
    proposal.steering_head(pending, move |seq| {
        steer::Ledger::reminded_head(&body, origin, at, seq).expect("reminder head")
    });
    commit(bus, proposal).expect("concurrent reminder commits");
}

fn append_judgment(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    anchor: Seq,
    opening: &Opening,
) {
    let at = current_instant().expect("judgment instant");
    let origin: wire::Origin = format!("resident:{epoch}")
        .parse()
        .expect("judgment origin");
    let mut proposal = JournalWrite::new(Actor::Spice, origin.clone(), at, Producer::Judge);
    let body = wire::body::TasteJudged {
        lane,
        occupant,
        compaction: anchor.get(),
        taste: opening.candidate.taste.clone(),
        content: opening.candidate.content.clone(),
        bag: opening.candidate.bag.clone(),
        trigger: opening.candidate.trigger.clone(),
        driver: opening.candidate.driver.clone(),
        source: opening.candidate.source.clone(),
        locus: opening.candidate.locus,
        attempts: vec![attempt(0, "YES", wire::body::TasteJudgeReply::Yes)],
        decision: wire::body::TasteDecision::Confirmed {
            directive: opening.directive,
            key: opening.key.to_string(),
            text: opening.candidate.text.clone(),
            expires: "2030-01-01T00:00:00.000Z".parse().expect("expiry"),
        },
    };
    let pending = proposal
        .fact(body.clone(), FactCause::Existing(anchor))
        .expect("judgment fact");
    proposal.steering_head(pending, move |seq| {
        steer::Ledger::judged_head(&body, origin, at, seq).expect("judgment head")
    });
    commit(bus, proposal).expect("concurrent judgment commits");
}

fn uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
        .parse()
        .expect("fixture UUID")
}
