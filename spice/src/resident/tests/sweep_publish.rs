//! One surviving supervisor notice presents every record its sweep accounted.

use super::*;

const CODEX_FIRST: &[u8] = br#"{"timestamp":"2030-04-12T18:44:09.271Z","type":"item.completed","item":{"id":"item-1","type":"agent_message","text":"first answer without a notice"}}
"#;
const CODEX_SECOND: &[u8] = br#"{"timestamp":"2030-04-12T18:44:10.271Z","type":"item.completed","item":{"id":"item-2","type":"agent_message","text":"second answer without a notice"}}
"#;
const CODEX_THIRD: &[u8] = br#"{"timestamp":"2030-04-12T18:44:11.271Z","type":"item.completed","item":{"id":"item-3","type":"agent_message","text":"third answer with the notice"}}
"#;

/// A notice is a wake-up: the records whose own notices the kernel refused are
/// on the tape, the sweep accounts them, and the Canvas sees all of them in
/// tape order rather than only the one record the surviving notice named.
#[test]
fn a_surviving_notice_presents_every_record_its_sweep_accounted() {
    let (root, paths) = resident_fixture("sweep-publish");
    let bus = Bus::start(journal::Journal::in_memory().expect("sweep authority opens"))
        .expect("sweep bus starts");
    let lane = test_uuid(5);
    let occupant = test_uuid(6);
    let epoch = test_uuid(4);
    canvas_announcement::seat_ingestion_residency(&bus, lane, epoch);
    let tape = tape::Tape::open(paths.tape.join(lane.to_string()).join("codex"))
        .expect("the lane tape opens");
    let first = tape.tee(CODEX_FIRST).expect("the first record is durable");
    let second = tape
        .tee(CODEX_SECOND)
        .expect("the second record is durable");
    let third = tape.tee(CODEX_THIRD).expect("the third record is durable");
    let mut seats = Seats::new();
    seats
        .seat_detached(
            lane,
            occupant,
            Box::new(DetachedFixture {
                dropped: Arc::new(AtomicU64::new(0)),
            }),
        )
        .expect("the sweep fixture occupant is seated");
    let source = canvas::stream::Source::new(bus.reader(), epoch, 8);
    let publisher = source.message_publisher();
    let mut updates = source.subscribe_messages();
    let mut diagnostics = Vec::new();
    let message_state = std::sync::Mutex::new(MessageState::default());
    let tastes = taste::Bag::new(Vec::new()).expect("empty taste bag");
    let steering = steer::Config::default();
    let surfaces = CaptureSurfaces::new(&paths, &tastes, &steering, &publisher);

    accept_capture_record(
        &bus,
        epoch,
        &mut seats,
        &surfaces,
        None,
        &noticed(lane, occupant, CODEX_THIRD, third),
        CaptureViews::new(&message_state, &mut diagnostics),
    )
    .expect("the surviving notice is accepted");

    let presented = std::iter::from_fn(|| updates.try_recv().ok())
        .map(|message| {
            (
                (message.locus.member, message.locus.offset),
                message.activity,
                message.text,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        presented,
        vec![
            (
                (first.member, first.offset),
                wire::AgentActivityKind::Prose,
                "first answer without a notice".to_owned(),
            ),
            (
                (second.member, second.offset),
                wire::AgentActivityKind::Prose,
                "second answer without a notice".to_owned(),
            ),
            (
                (third.member, third.offset),
                wire::AgentActivityKind::Prose,
                "third answer with the notice".to_owned(),
            ),
        ],
        "the sweep presented something other than its three accounted records: {}",
        String::from_utf8_lossy(&diagnostics),
    );

    // A notice that arrives after its bytes were accounted (the startup overlap)
    // still crosses, and the retained locus refuses the repeat presentation.
    accept_capture_record(
        &bus,
        epoch,
        &mut seats,
        &surfaces,
        None,
        &noticed(lane, occupant, CODEX_FIRST, first),
        CaptureViews::new(&message_state, &mut diagnostics),
    )
    .expect("a late notice for an accounted record is accepted");
    assert!(
        updates.try_recv().is_err(),
        "an already presented record was presented again",
    );
    assert!(
        diagnostics.is_empty(),
        "the sweep raised diagnostics: {}",
        String::from_utf8_lossy(&diagnostics),
    );
    seats.release_all();
    bus.shutdown().expect("sweep bus stops");
    fs::remove_dir_all(root).expect("sweep fixture cleanup");
}

/// The capture record one supervisor notice materializes for a tape record.
pub(super) fn noticed(
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    bytes: &[u8],
    locus: tape::Locus,
) -> lane::capture::CaptureRecord {
    lane::capture::CaptureRecord {
        lane,
        occupant,
        stream: vendor::RecordStream::Stdout,
        boundary: sill::RecordBoundary::Complete,
        locus,
        declaration: None,
        decoded: listen::decode(listen::Dialect::Codex, bytes, locus)
            .expect("the fixture record decodes under its own locus"),
    }
}
