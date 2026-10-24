//! A seat that closes presents what its occupant left on the tape.

use super::*;

const EARLIER: &[u8] = br#"{"timestamp":"2030-04-12T18:44:09.271Z","type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"earlier answer whose notice was refused"}]}}
"#;
const LAST: &[u8] = br#"{"timestamp":"2030-04-12T18:44:10.271Z","type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"final answer written moments before exit"}]}}
"#;

/// The exit is accounted before the last records' notices are read, so the
/// closing sweep presents them itself, and a notice that arrives afterwards
/// is refused by name rather than dropped.
#[test]
fn a_closing_seat_presents_every_record_its_occupant_left_behind() {
    let (root, paths) = resident_fixture("closed-transcript");
    let bus = Bus::start(journal::Journal::in_memory().expect("closing authority opens"))
        .expect("closing bus starts");
    let lane = test_uuid(5);
    let occupant = test_uuid(6);
    let epoch = test_uuid(4);
    app_announcement::seat_ingestion_residency(&bus, lane, epoch);
    let tape = tape::Tape::open(paths.tape.join(lane.to_string()).join("omp"))
        .expect("the lane tape opens");
    let earlier = tape.tee(EARLIER).expect("the earlier record is durable");
    let last = tape.tee(LAST).expect("the last record is durable");
    let mut seats = Seats::new();
    seats
        .seat_detached(
            lane,
            occupant,
            Box::new(DetachedFixture {
                dropped: Arc::new(AtomicU64::new(0)),
            }),
        )
        .expect("the closing fixture occupant is seated");
    let source = message_source(bus.reader(), epoch);
    let publisher = source.message_publisher();
    let mut updates = source.subscribe_messages();
    let mut diagnostics = Vec::new();
    let message_state = std::sync::Mutex::new(MessageState::default());
    let tastes = taste::Bag::new(Vec::new()).expect("empty taste bag");
    let steering = steer::Config::default();
    let generation = publisher.batch().expect("closing generation begins");
    let ambient = spice::ambient::capture();
    let surfaces = CaptureSurfaces::new(&paths, &tastes, &steering, &generation, &ambient);

    drop(seats.vacate(lane));
    for (closed_lane, closed_occupant) in seats.take_vacated() {
        message_surface::present_closed_transcript(
            &bus,
            epoch,
            &surfaces,
            message_surface::TasteRecognition::Direct,
            closed_lane,
            closed_occupant,
            CaptureViews::new(&message_state, &mut diagnostics),
        )
        .expect("the closing sweep presents");
    }
    assert!(generation.commit().expect("closing generation commits"));

    let generation = updates
        .try_recv()
        .expect("one closing generation publishes");
    assert!(
        updates.try_recv().is_err(),
        "the closing sweep published a partial second generation",
    );
    let presented = generation
        .messages
        .into_iter()
        .map(|message| ((message.locus.member, message.locus.offset), message.text))
        .collect::<Vec<_>>();
    assert_eq!(
        presented,
        vec![
            (
                (earlier.member, earlier.offset),
                "earlier answer whose notice was refused".to_owned(),
            ),
            (
                (last.member, last.offset),
                "final answer written moments before exit".to_owned(),
            ),
        ],
        "the closing sweep presented something other than the two records left behind: {}",
        String::from_utf8_lossy(&diagnostics),
    );
    assert!(
        diagnostics.is_empty(),
        "the closing sweep raised diagnostics: {}",
        String::from_utf8_lossy(&diagnostics),
    );

    let late = supervisor::Notice::Record {
        lane,
        occupant,
        stream: "stdout".to_owned(),
        boundary: "complete".to_owned(),
        locus: wire::TapeLocus {
            member: last.member,
            offset: last.offset,
            length: last.length,
        },
    };
    let residency = current_residency(&bus, epoch).expect("the residency head reads");
    let materialized = capture::materialize(&paths, residency.body(), late, &mut diagnostics)
        .expect("a late notice is refused rather than faulted");
    assert!(
        materialized.is_none(),
        "a late notice materialized a record"
    );
    let spoken = String::from_utf8_lossy(&diagnostics);
    assert!(
        spoken.contains(&format!(
            "lane {lane} record notice from occupant {occupant} after its seat closed"
        )),
        "the late notice was refused silently: {spoken:?}",
    );

    seats.release_all();
    bus.shutdown().expect("closing bus stops");
    fs::remove_dir_all(root).expect("closing fixture cleanup");
}
