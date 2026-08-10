//! Canvas message publication-unavailable regression.

use super::*;

struct UnavailableMessagePublisher;

impl AgentMessagePublisher for UnavailableMessagePublisher {
    fn publish(
        &self,
        _message: wire::AgentMessage,
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        Err(canvas::stream::MessagePublishError::Unavailable)
    }
}

#[test]
fn unavailable_canvas_messages_are_diagnostic_and_capture_remains_live() {
    let (root, paths) = resident_fixture("message-unavailable");
    let bus = Bus::start(journal::Journal::in_memory().expect("capture authority opens"))
        .expect("capture bus starts");
    let epoch = test_uuid(4);
    let lane = test_uuid(5);
    let occupant = test_uuid(6);
    let mut seats = Seats::new();
    seats
        .seat(
            lane,
            occupant,
            process_spec("sleep 0.05", Duration::from_secs(5)),
        )
        .expect("capture fixture group starts");
    let mut diagnostics = Vec::new();
    let mut previews = CallPreviews::default();
    let tastes = taste::Bag::new(Vec::new()).expect("empty taste bag");
    let steering = steer::Config::default();
    let publisher = UnavailableMessagePublisher;
    let surfaces = CaptureSurfaces::new(&paths, &tastes, &steering, &publisher);

    for (offset, text) in [(120, "first answer"), (180, "later answer")] {
        let locus = listen::Locus::new(4, offset, 48);
        let record = lane::capture::CaptureRecord {
            lane,
            occupant,
            stream: vendor::RecordStream::Stdout,
            boundary: sill::RecordBoundary::Complete,
            locus,
            declaration: None,
            decoded: listen::Decoded::Events {
                context: Default::default(),
                events: vec![listen::Event::Prose(listen::Prose {
                    role: listen::ProseRole::Assistant,
                    text: text.to_owned(),
                    phase: None,
                    locus,
                })],
            },
        };

        accept_capture_record(
            &bus,
            epoch,
            &mut seats,
            &surfaces,
            &record,
            &mut previews,
            &mut diagnostics,
        )
        .expect("a failed Canvas view leaves the capture handler live");
    }

    assert_eq!(
        String::from_utf8(diagnostics).expect("diagnostic is text"),
        concat!(
            "canvas message unavailable: canvas::message-publish-failed: ",
            "canvas message retention is unavailable\n",
            "canvas message unavailable: canvas::message-publish-failed: ",
            "canvas message retention is unavailable\n",
        )
    );
    seats
        .vacate(lane)
        .expect("capture fixture remains supervised")
        .wait()
        .expect("capture fixture stops");
    bus.shutdown().expect("capture bus stops");
    fs::remove_dir_all(root).expect("fixture cleanup");
}
