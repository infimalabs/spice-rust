//! A seated lane that keeps running tools without visible prose is prodded,
//! and the prod stops once it speaks.

use super::*;
use sweep_publish::noticed;
use wire::body::{NarrationCleared, NarrationProdded, NarrationRelief};

/// Tool moments the fixture lane may run without visible prose before the prod.
const BOUND: u32 = 3;

const PROSE: &[u8] = br#"{"timestamp":"2030-04-12T18:44:20.271Z","type":"item.completed","item":{"id":"item-prose","type":"agent_message","text":"three listings in, nothing renamed yet"}}
"#;

/// One completed codex command: a tool call and its result on one record.
fn command(second: u8) -> Vec<u8> {
    let mut bytes = format!(
        r#"{{"timestamp":"2030-04-12T18:44:{second:02}.271Z","type":"item.completed","item":{{"id":"call-{second}","type":"command_execution","command":"ls","status":"completed","aggregated_output":"","exit_code":0}}}}"#
    )
    .into_bytes();
    bytes.push(b'\n');
    bytes
}

/// One seated codex lane whose supervisor notices reach the resident sweep.
struct Occupied<'a> {
    bus: &'a Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    tape: tape::Tape,
    seats: Seats,
    state: std::sync::Mutex<MessageState>,
    diagnostics: Vec<u8>,
}

impl Occupied<'_> {
    /// Tee one record onto the lane tape and sweep it as its own notice.
    fn accept<P: AgentMessagePublisher>(
        &mut self,
        surfaces: &CaptureSurfaces<'_, P>,
        bytes: &[u8],
    ) {
        let locus = self.tape.tee(bytes).expect("the record is durable");
        accept_capture_record(
            self.bus,
            self.epoch,
            &mut self.seats,
            surfaces,
            None,
            &noticed(self.lane, self.occupant, bytes, locus),
            CaptureViews::new(&self.state, &mut self.diagnostics),
        )
        .expect("the notice is accepted");
    }
}

/// Every prod the resident recorded, in journal order.
fn prods(bus: &Bus) -> Vec<NarrationProdded> {
    bus.fact_snapshot()
        .expect("the journal reads")
        .facts()
        .iter()
        .filter_map(|envelope| envelope.fact.read::<NarrationProdded>())
        .collect()
}

/// Every clearance the resident recorded, in journal order.
fn clearances(bus: &Bus) -> Vec<NarrationCleared> {
    bus.fact_snapshot()
        .expect("the journal reads")
        .facts()
        .iter()
        .filter_map(|envelope| envelope.fact.read::<NarrationCleared>())
        .collect()
}

/// The working state the PostToolUse hook reads for the lane, and its line.
fn hook_status(
    root: &Path,
    lane: CanonicalUuid,
) -> (Option<spice::session_meter::NarrationState>, Option<String>) {
    let state = spice::session_meter::LiveMeter::working_state_for_lane(root, lane)
        .expect("the lane's working state reads");
    let status = spice::session_meter::ordinary_status(&state, false);
    (state.narration, status)
}

/// The resident measures the lane from its tape, records the prod as a fact the
/// hook's working state reads back for the lane, and clears it on the first
/// visible prose; further tool calls under the bound stay silent.
#[test]
fn a_silent_lane_is_prodded_through_the_hook_and_falls_silent_once_it_speaks() {
    let (root, paths) = resident_fixture("narration-prod");
    let bus = Bus::start(journal::Journal::open(&paths.journal).expect("narration journal opens"))
        .expect("narration bus starts");
    let lane = test_uuid(5);
    let occupant = test_uuid(6);
    let epoch = test_uuid(4);
    canvas_announcement::seat_occupied_ingestion_residency(&bus, lane, occupant, epoch);
    let mut seats = Seats::new();
    seats
        .seat_detached(
            lane,
            occupant,
            Box::new(DetachedFixture {
                dropped: Arc::new(AtomicU64::new(0)),
            }),
        )
        .expect("the narration fixture occupant is seated");
    let mut occupied = Occupied {
        bus: &bus,
        epoch,
        lane,
        occupant,
        tape: tape::Tape::open(paths.tape.join(lane.to_string()).join("codex"))
            .expect("the lane tape opens"),
        seats,
        state: std::sync::Mutex::new(MessageState::default()),
        diagnostics: Vec::new(),
    };
    let source = canvas::stream::Source::new(bus.reader(), epoch, 8);
    let publisher = source.message_publisher();
    let tastes = taste::Bag::new(Vec::new()).expect("empty taste bag");
    let steering = steer::Config::default();
    let surfaces = CaptureSurfaces::new(&paths, &tastes, &steering, &publisher).narration(BOUND);

    occupied.accept(&surfaces, &command(1));
    occupied.accept(&surfaces, &command(2));
    assert!(prods(&bus).is_empty(), "a lane under the bound was prodded");
    assert_eq!(hook_status(&root, lane), (None, None));

    occupied.accept(&surfaces, &command(3));
    assert_eq!(
        prods(&bus),
        vec![NarrationProdded {
            lane,
            occupant,
            moments: BOUND,
            count: 1,
        }],
    );
    let (narration, _) = hook_status(&root, lane);
    assert_eq!(
        narration,
        Some(spice::session_meter::NarrationState {
            moments: BOUND,
            count: 1,
        }),
    );

    occupied.accept(&surfaces, PROSE);
    assert_eq!(
        clearances(&bus),
        vec![NarrationCleared {
            lane,
            occupant,
            moments: BOUND,
            prods: 1,
            relief: NarrationRelief::Prose,
        }],
    );
    assert_eq!(hook_status(&root, lane), (None, None));

    occupied.accept(&surfaces, &command(4));
    occupied.accept(&surfaces, &command(5));
    assert_eq!(
        prods(&bus).len(),
        1,
        "a lane that spoke was prodded again under the bound"
    );
    assert_eq!(hook_status(&root, lane), (None, None));
    assert!(
        occupied.diagnostics.is_empty(),
        "the sweep raised diagnostics: {}",
        String::from_utf8_lossy(&occupied.diagnostics),
    );

    occupied.seats.release_all();
    drop(occupied);
    bus.shutdown().expect("narration bus stops");
    fs::remove_dir_all(root).expect("narration fixture cleanup");
}
