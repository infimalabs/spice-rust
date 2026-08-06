use wire::{
    Actor, CanonicalUuid, Envelope, Fact, Packed, StreamAdmissionError, StreamCursor, StreamFact,
    StreamFrame, StreamRequest, ViewSubject, WireVersion,
};

fn uuid(value: &str) -> CanonicalUuid {
    value.parse().expect("canonical fixture uuid")
}

fn seq(value: u64) -> wire::Seq {
    wire::Seq::new(value).expect("non-zero fixture cursor")
}

#[test]
fn stream_requests_are_strict_current_generation_documents() {
    let epoch = uuid("018f82d4-6f59-7d06-92e3-bce9131163c3");
    let request = StreamRequest::Subscribe {
        v: WireVersion::CURRENT,
        id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c4"),
        epoch,
        subjects: vec![ViewSubject::Facts, ViewSubject::Trust],
        after: Some(StreamCursor { epoch, seq: seq(7) }),
    };
    let bytes = wire::encode(&request).expect("request encodes");
    let decoded: StreamRequest = wire::decode(&bytes).expect("request decodes");

    assert_eq!(decoded, request);
    assert!(decoded.admit(epoch).is_ok());
    assert!(
        wire::decode::<StreamRequest>(
            br#"{
            "stream":"subscribe",
            "v":1,
            "id":"018f82d4-6f59-7d06-92e3-bce9131163c4",
            "epoch":"018f82d4-6f59-7d06-92e3-bce9131163c3",
            "subjects":["facts"],
            "extra":true
        }"#
        )
        .is_err()
    );
    assert!(
        wire::decode::<StreamRequest>(
            br#"{
            "stream":"thread",
            "v":1,
            "id":"018f82d4-6f59-7d06-92e3-bce9131163c4",
            "epoch":"018f82d4-6f59-7d06-92e3-bce9131163c3",
            "cursor":{
                "epoch":"018f82d4-6f59-7d06-92e3-bce9131163c3",
                "seq":0
            }
        }"#
        )
        .is_err()
    );
}

#[test]
fn wrong_wire_request_and_stale_cursor_are_distinct_before_application() {
    let epoch = uuid("018f82d4-6f59-7d06-92e3-bce9131163c3");
    let stale = uuid("018f82d4-6f59-7d06-92e3-bce9131163c9");
    let wrong_wire = StreamRequest::Subscribe {
        v: WireVersion(99),
        id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c4"),
        epoch,
        subjects: vec![ViewSubject::Facts],
        after: None,
    };
    let stale_cursor = StreamRequest::Continue {
        v: WireVersion::CURRENT,
        id: uuid("018f82d4-6f59-7d06-92e3-bce9131163c5"),
        epoch,
        subject: ViewSubject::Facts,
        cursor: StreamCursor {
            epoch: stale,
            seq: seq(7),
        },
    };

    assert!(matches!(
        wrong_wire.admit(epoch),
        Err(StreamAdmissionError::Wire(_))
    ));
    assert!(matches!(
        stale_cursor.admit(epoch),
        Err(StreamAdmissionError::Cursor(_))
    ));
}

#[test]
fn lane_seated_stream_fact_is_strict_and_credential_free() {
    let capability = uuid("018f82d4-6f59-7d06-92e3-bce9131163d9");
    let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163d8");
    let typed = Envelope {
        v: WireVersion::CURRENT,
        seq: seq(7),
        at: "2030-04-12T18:44:09.271Z".parse().expect("instant"),
        actor: Actor::Spice,
        origin: "resident:018f82d4-6f59-7d06-92e3-bce9131163c3"
            .parse()
            .expect("origin"),
        cause: None,
        fact: Fact::new(wire::body::LaneSeated {
            lane,
            team: lane,
            capability,
            worktree: "/tmp/lane".to_owned(),
            life: "drive".to_owned(),
            vendor: "claude".parse().expect("vendor"),
        }),
    };
    let durable = wire::encode(&typed).expect("durable fact encodes");
    assert!(String::from_utf8_lossy(&durable).contains(&capability.to_string()));
    let opaque: Envelope<wire::AnyFact> = wire::decode(&durable).expect("durable fact decodes");
    let projected = StreamFact::from(opaque);
    let frame = StreamFrame::Facts {
        v: WireVersion::CURRENT,
        id: uuid("018f82d4-6f59-7d06-92e3-bce9131163d7"),
        epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
        packed: Packed {
            rows: vec![projected],
            overflow: None,
            horizon: seq(7),
        },
    };

    let rendered = wire::encode(&frame).expect("stream frame encodes");
    assert!(!String::from_utf8_lossy(&rendered).contains(&capability.to_string()));
    let decoded: StreamFrame = wire::decode(&rendered).expect("stream frame decodes");
    assert_eq!(decoded, frame);
    assert!(
        wire::decode::<StreamFact>(&durable).is_err(),
        "the credential-bearing durable shape is not a stream DTO"
    );
    let StreamFrame::Facts { packed, .. } = decoded else {
        panic!("expected facts frame");
    };
    assert_eq!(packed.rows[0].kind(), wire::FactKind::LaneSeated);
    assert!(!packed.rows[0].body().contains_key("capability"));
}
