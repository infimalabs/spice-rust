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

/// One continuing-session declaration for `lane` under `occupant`.
fn continuing_session(
    lane: CanonicalUuid,
    epoch: CanonicalUuid,
    occupant: CanonicalUuid,
) -> wire::body::LaneSessionDeclared {
    wire::body::LaneSessionDeclared {
        lane,
        occupant,
        worktree: "/worktree".to_owned(),
        vendor: "omp".parse().expect("fixture vendor"),
        launch: wire::body::LaneLaunchIdentity::new("model", "effort"),
        epoch,
        stream: "session".to_owned(),
        source: test_uuid(80),
        identity: "continuing-session".to_owned(),
        member: 1,
        offset: 2,
        length: 3,
    }
}

/// Declare `occupant`'s session holding one conversation card, and confirm
/// that card alone is the matchers' background while the occupant is seated.
fn seat_conversation(
    state: &mut MessageState,
    lane: CanonicalUuid,
    epoch: CanonicalUuid,
    occupant: CanonicalUuid,
) {
    use board::similarity::Background;
    use spice::rooms::evidence::{Conversation, Evidence};
    use spice::rooms::profile::EvidenceKey;

    let conversation = Conversation::read(
        "The closing sweep vacates an occupant binding while retaining the profile evidence \
         the departed session gathered for its successor.",
    )
    .expect("enough distinct terms for one conversation card");
    let key = state
        .profiles
        .declare(&continuing_session(lane, epoch, occupant));
    state.profiles.insert(
        lane,
        occupant,
        Evidence::conversation(
            EvidenceKey::Conversation {
                source: test_uuid(80),
                member: 1,
                offset: 10,
                length: 20,
            },
            &conversation,
        ),
    );
    assert_eq!(
        state.profiles.conversation_background(),
        Background::of(
            state
                .profiles
                .profile(&key)
                .expect("declared profile")
                .evidence()
                .conversation_messages()
        ),
        "the seated occupant's conversation is the background before it closes",
    );
    assert_ne!(
        state.profiles.conversation_background(),
        Background::default()
    );
}

/// The closing sweep is where a departed occupant leaves the matchers'
/// background: its exact binding goes, its continuing profile and retained
/// conversation stay, and a successor that declares the same session is the
/// one seated holder of that conversation, counted once.
#[test]
fn a_closing_seat_leaves_the_conversation_background_but_keeps_its_profile() {
    use board::similarity::Background;

    let (root, paths) = resident_fixture("closed-transcript-vacancy");
    let bus = Bus::start(journal::Journal::in_memory().expect("closing authority opens"))
        .expect("closing bus starts");
    let lane = test_uuid(5);
    let occupant = test_uuid(6);
    let successor = test_uuid(7);
    let epoch = test_uuid(4);
    app_announcement::seat_ingestion_residency(&bus, lane, epoch);
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
    let message_state = std::sync::Mutex::new(MessageState::default());
    seat_conversation(
        &mut message_state.lock().expect("message state locks"),
        lane,
        epoch,
        occupant,
    );
    let tastes = taste::Bag::new(Vec::new()).expect("empty taste bag");
    let steering = steer::Config::default();
    let source = message_source(bus.reader(), epoch);
    let publisher = source.message_publisher();
    let generation = publisher.batch().expect("closing generation begins");
    let ambient = spice::ambient::capture();
    let surfaces = CaptureSurfaces::new(&paths, &tastes, &steering, &generation, &ambient);
    let mut diagnostics = Vec::new();

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
    drop(generation);

    let mut state = message_state.lock().expect("message state locks");
    assert_eq!(
        state.profiles.conversation_background(),
        Background::default(),
        "a vacated occupant still counts in the background: {}",
        String::from_utf8_lossy(&diagnostics),
    );
    assert!(
        state.profiles.session(lane, occupant).is_none(),
        "the vacated binding survived the closing sweep",
    );
    let key = state
        .profiles
        .declare(&continuing_session(lane, epoch, successor));
    let retained = state
        .profiles
        .profile(&key)
        .expect("the continuing profile outlives its occupant")
        .evidence()
        .conversation_messages()
        .count();
    assert_eq!(
        retained, 1,
        "the continuing profile lost its retained conversation"
    );
    assert_eq!(
        state.profiles.conversation_background(),
        Background::of(
            state
                .profiles
                .profile(&key)
                .expect("declared profile")
                .evidence()
                .conversation_messages()
        ),
        "a successor resuming the exact session counts that conversation once",
    );
    drop(state);

    seats.release_all();
    bus.shutdown().expect("closing bus stops");
    fs::remove_dir_all(root).expect("closing fixture cleanup");
}
