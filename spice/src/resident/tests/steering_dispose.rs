//! The ack command judged against the prose the lane tape holds.

use std::cell::Cell;

use wire::Disposition;

use super::*;

/// The prose a lane speaks when it declines and then explains itself by
/// citing the header it wrote, once mid-sentence and once leading a line.
fn declined_then_cited(key: &str) -> String {
    format!(
        "NACK {key}: Verified on the tape; the direction names a lane this worktree does not hold.\n\n\
         I emitted `ACK {key}: Carrying it forward now` as prose before checking, and\n\
         `ACK {key}: Carrying it forward now` is the header the operator saw."
    )
}

/// The placed header decides the key even when the same prose goes on to
/// cite that header, and a witness the index no longer bears still moves.
#[test]
fn a_placed_header_retires_its_key_when_the_same_prose_later_cites_it() {
    let direction = Direction::new("ack-placed-over-cited");
    direction.speak(&declined_then_cited(&direction.key));
    let before = direction.fact_kinds().len();

    let (outcome, effects) = direction.ack(Disposition::Declined, direction.live_witness());

    let Outcome::Reply(Reply {
        body: CommandAnswer::Directive(answer),
        ..
    }) = outcome
    else {
        panic!("the placed NACK header retires the key: {outcome:?}");
    };
    assert_eq!(answer.directive, direction.directive);
    assert_eq!(answer.disposition, Disposition::Declined);
    assert_eq!(
        direction.fact_kinds()[before..],
        [wire::FactKind::SteerDeclined],
        "the citations after the header record no miss"
    );
    assert_eq!(direction.head().disposition, Disposition::Declined);
    let declined = direction
        .last_fact::<wire::body::SteerDeclined>()
        .expect("the decline is journaled");
    assert_eq!(
        (declined.locus.line, declined.locus.column),
        (1, 6),
        "the journaled locus cites the header, not the citation"
    );
    assert_eq!(effects.inbox_projection, Some(direction.lane));

    direction.speak(&format!("ACK {}: again, after the fact.", direction.key));
    let (stale, _) = direction.ack(Disposition::Acked, direction.live_witness());
    let Outcome::Refusal(refusal) = stale else {
        panic!("a witness naming a retired directive as live has moved: {stale:?}");
    };
    assert_eq!(refusal.reason(), "steer::witness-moved");
    direction.finish();
}

/// A mention that only quotes or buries a live key names its own miss.
///
/// The lane's shell cites the live directive its inbox shows, and that
/// witness holds: nothing about the directive moved, the prose missed. The
/// refusal therefore names the miss the lane can repair by speaking a header,
/// and journals it, instead of sending the lane to reread a snapshot that
/// would show it the same live key.
#[test]
fn a_quoted_or_buried_mention_against_a_live_witness_names_its_miss() {
    let direction = Direction::new("ack-mention-miss");
    let key = direction.key.clone();
    let before = direction.fact_kinds().len();

    for (spoken, miss, column) in [
        (
            format!(
                "`ACK {key}: Carrying it forward now` is the header I will emit once the check lands."
            ),
            wire::body::AcknowledgmentMiss::Quoted,
            6,
        ),
        (
            format!("I emitted ACK {key}: Carrying it forward now, but only as narration."),
            wire::body::AcknowledgmentMiss::Buried,
            15,
        ),
    ] {
        direction.speak(&spoken);
        let (outcome, effects) = direction.ack(Disposition::Acked, direction.live_witness());
        let Outcome::Refusal(refusal) = outcome else {
            panic!("a {miss:?} mention retires nothing: {outcome:?}");
        };
        assert_eq!(refusal.reason(), "steer::ack-missed", "{spoken:?}");
        let diagnostic = refusal.diagnostic().expect("a miss carries its diagnostic");
        assert_eq!(
            diagnostic.observed(),
            Some(format!("{key}:{}", format!("{miss:?}").to_lowercase()).as_str())
        );
        assert_eq!(
            diagnostic.detail(),
            Some(
                "lead a new assistant prose line with the ACK or NACK header; \
                 a quoted or buried mention of the key acknowledges nothing"
            )
        );
        assert!(effects.inbox_projection.is_none());
        let missed = direction
            .last_fact::<wire::body::SteerMissed>()
            .expect("the miss is journaled");
        assert_eq!(missed.key, key);
        assert_eq!(missed.lane, direction.lane);
        assert_eq!(missed.epoch, direction.epoch);
        assert_eq!(missed.miss, miss);
        assert_eq!((missed.locus.line, missed.locus.column), (1, column));
        assert_eq!(direction.head().disposition, Disposition::Live);
    }
    assert_eq!(
        direction.fact_kinds()[before..],
        [wire::FactKind::SteerMissed, wire::FactKind::SteerMissed]
    );
    direction.finish();
}

/// One occupied lane holding a live operator directive, with the transcript
/// tape the ack command reads.
struct Direction {
    root: PathBuf,
    paths: RuntimePaths,
    bus: Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    directive: CanonicalUuid,
    key: String,
    /// The last fixture receipt id handed to an ack command.
    receipt: Cell<u8>,
}

impl Direction {
    fn new(tag: &str) -> Self {
        let (root, paths) = resident_fixture(tag);
        let repository = Repository::discover(&root).expect("fixture repository resolves");
        let worktree = repository.worktree_root().to_string_lossy().into_owned();
        let surface =
            crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
        let bus =
            Bus::start(journal::Journal::open(&paths.journal).expect("fixture journal opens"))
                .expect("fixture bus starts");
        let epoch = test_uuid(1);
        let lane = test_uuid(2);
        let occupant = test_uuid(3);
        commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
        seed_lane_in_worktree(&bus, epoch, lane, occupant, &worktree);
        let send = wire::intent::SteerSend {
            fanout: test_uuid(31),
            lane,
            message: "answer before the turn ends".to_owned(),
            attachments: Vec::new(),
        };
        let command = Command {
            v: wire::WireVersion::CURRENT,
            id: test_uuid(30),
            epoch,
            kind: CommandKind::SteerSend,
            expected: vec![Witness::Residency {
                lane,
                occupant: Some(occupant),
                epoch,
            }],
            body: serde_json::to_value(&send).expect("steer request encodes"),
        }
        .admit(&wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor: Actor::Operator,
            epoch,
        })
        .expect("operator send admits");
        let mut effects = CommandEffects::default();
        let outcome = steering::send(
            &bus,
            epoch,
            &steer::Config::default(),
            &command,
            &send,
            &mut effects,
        );
        let Outcome::Reply(Reply {
            body: CommandAnswer::Directive(sent),
            ..
        }) = outcome
        else {
            panic!("the operator direction is durable: {outcome:?}");
        };
        Self {
            root,
            paths,
            bus,
            epoch,
            lane,
            directive: sent.directive,
            key: sent.key,
            receipt: Cell::new(40),
        }
    }

    /// Persist one assistant text record on the lane tape.
    fn speak(&self, text: &str) {
        let record = serde_json::json!({
            "type": "assistant",
            "message": {
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "text", "text": text }]
            }
        });
        let mut bytes = wire::encode(&record).expect("prose record encodes");
        bytes.push(b'\n');
        tape::Tape::open(self.paths.tape.join(self.lane.to_string()).join("claude"))
            .expect("lane tape opens")
            .tee(&bytes)
            .expect("prose reaches the tape");
    }

    /// The witness the lane's shell cites while its inbox carries the key.
    fn live_witness(&self) -> Vec<Witness> {
        vec![Witness::Directive {
            steer: self.directive,
            disposition: Disposition::Live,
        }]
    }

    /// Run the ack command from the lane with the given witness.
    fn ack(
        &self,
        disposition: Disposition,
        expected: Vec<Witness>,
    ) -> (Outcome<CommandAnswer>, CommandEffects) {
        let dispose = wire::intent::SteerDispose {
            token: self.key.clone(),
            disposition,
        };
        self.receipt.set(self.receipt.get() + 1);
        let command = Command {
            v: wire::WireVersion::CURRENT,
            id: test_uuid(self.receipt.get()),
            epoch: self.epoch,
            kind: CommandKind::SteerDispose,
            expected,
            body: serde_json::to_value(&dispose).expect("ack request encodes"),
        }
        .admit(&wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor: Actor::Lane(self.lane),
            epoch: self.epoch,
        })
        .expect("lane ack admits");
        let mut effects = CommandEffects::default();
        let outcome = steering::dispose(
            &self.bus,
            &self.paths,
            self.epoch,
            &command,
            &dispose,
            &mut effects,
        );
        (outcome, effects)
    }

    fn head(&self) -> wire::SteeringHead {
        let directive = self.directive;
        self.bus
            .read(move |read| read.steering_head(directive))
            .expect("steering index reads")
            .expect("the operator direction is indexed")
            .body()
            .clone()
    }

    fn fact_kinds(&self) -> Vec<wire::FactKind> {
        self.bus
            .fact_snapshot()
            .expect("facts read")
            .facts()
            .iter()
            .map(|fact| fact.fact.kind())
            .collect()
    }

    fn last_fact<B: wire::Body>(&self) -> Option<B> {
        self.bus
            .fact_snapshot()
            .expect("facts read")
            .facts()
            .iter()
            .rev()
            .find_map(|fact| fact.fact.read::<B>())
    }

    fn finish(self) {
        self.bus.verify().expect("the steering authority verifies");
        self.bus.shutdown().expect("fixture bus stops");
        fs::remove_dir_all(self.root).expect("fixture cleanup");
    }
}
