use super::*;

fn authority(text: &str) -> RoomAuthorityStamp {
    text.parse().expect("authority")
}

fn task(text: &str) -> TaskStamp {
    text.parse().expect("task")
}

fn lane(text: &str) -> CanonicalUuid {
    text.parse().expect("lane")
}

fn instant(text: &str) -> Instant {
    text.parse().expect("instant")
}

fn source(sequence: u64) -> RoomSource {
    RoomSource::Journal {
        fact: SequenceCode::new(sequence),
    }
}

fn message(routes: Vec<RoomRoute>, bytes: u32) -> RoomPublication {
    serde_json::from_value(serde_json::json!({
        "publication": "1kKRRsDR1",
        "author": {"kind": "lane", "lane": "018f82d4-6f59-7d06-92e3-bce913116301"},
        "source": {"kind": "journal", "fact": "1"},
        "content_bytes": bytes,
        "kind": {"verb": "msg"},
        "routes": routes,
    }))
    .expect("message")
}

#[test]
fn selectors_keep_independent_qualifiers_and_reject_duplicate_or_unknown_routes() {
    let first = RoomRoute {
        destination: RoomDestination::Lane {
            lane: lane("018f82d4-6f59-7d06-92e3-bce913116302"),
        },
        context: Some(task("1kKRRsDR2")),
        direction: None,
    };
    let second = RoomRoute {
        destination: RoomDestination::Operator {},
        context: None,
        direction: None,
    };
    let publication = message(vec![first.clone(), second.clone()], 40);
    assert_eq!(publication.routes, [first.clone(), second]);
    assert_eq!(
        crate::decode::<RoomPublication>(&crate::encode(&publication).unwrap()).unwrap(),
        publication
    );
    assert_eq!(
        publication.lane_routes().collect::<Vec<_>>(),
        [(
            lane("018f82d4-6f59-7d06-92e3-bce913116302"),
            Some(&task("1kKRRsDR2"))
        )]
    );
    assert_eq!(
        publication.operator_item(),
        Some((RoomReceiptRef::new(authority("1kKRRsDR1")), None))
    );

    for routes in [
        serde_json::json!([first.clone(), first]),
        serde_json::json!([{"destination": {"kind": "group", "group": "1kKRRsDR2"}}]),
        serde_json::json!([
            {"destination": {"kind": "operator"}},
            {"destination": {"kind": "operator"}, "context": "1kKRRsDR2"},
        ]),
        serde_json::json!([{"destination": {"kind": "operator"}, "direction": "dVwg6Zqlf"}]),
    ] {
        let refused = serde_json::json!({
            "publication": "1kKRRsDR1",
            "author": {"kind": "operator"},
            "source": {"kind": "journal", "fact": "1"},
            "content_bytes": 1,
            "kind": {"verb": "msg"},
            "routes": routes,
        });
        assert!(serde_json::from_value::<RoomPublication>(refused).is_err());
    }
    let about = |destination: serde_json::Value| {
        serde_json::from_value::<RoomPublication>(serde_json::json!({
            "publication": "1kKRRsDR1",
            "author": {"kind": "lane", "lane": "018f82d4-6f59-7d06-92e3-bce913116301"},
            "source": {"kind": "journal", "fact": "1"},
            "content_bytes": 1,
            "kind": {"verb": "msg"},
            "routes": [{"destination": destination, "direction": "dVwg6Zqlf"}],
        }))
    };
    assert!(about(serde_json::json!({"kind": "operator"})).is_ok());
    assert!(
        about(serde_json::json!({"kind": "lane", "lane": "018f82d4-6f59-7d06-92e3-bce913116302"}))
            .is_err()
    );
}

fn control(disposition: &str, key: &RoomReceiptRef) -> RoomReceiptControl {
    serde_json::from_value(serde_json::json!({
        "disposition": disposition,
        "keys": [key],
        "source": {"kind": "journal", "fact": "2"},
    }))
    .expect("control")
}

#[test]
fn lapsed_item_recovers_under_its_stable_key() {
    let item = RoomReceiptRef::new(authority("1kKRRsDR1"));
    let first = instant("2030-01-01T00:00:00.000Z");
    let mut receipt = RoomReceiptHead::pending(item.clone(), first);
    let lapse = RoomReceiptState::Expired { at: first };
    receipt.settle(lapse.clone()).expect("pending item lapses");
    receipt.settle(lapse).expect("exact lapse replays");
    assert_eq!(
        receipt.settle(RoomReceiptState::Acked { source: source(2) }),
        Err(RoomReceiptTransitionError::NotPending)
    );
    assert_eq!(
        receipt.recover(first),
        Err(RoomReceiptTransitionError::Expiry)
    );
    let fresh = instant("2030-01-01T01:00:00.000Z");
    receipt.recover(fresh).expect("lapse recovers");
    assert_eq!(receipt.receipt(), &item);
    assert_eq!(
        receipt.state(),
        &RoomReceiptState::Pending { expires: fresh }
    );
    assert_eq!(
        receipt.recover(instant("2030-01-01T02:00:00.000Z")),
        Err(RoomReceiptTransitionError::NotLapsed)
    );
    receipt
        .settle(RoomReceiptState::Acked { source: source(2) })
        .expect("ACK settles");
    assert_eq!(
        receipt.settle(RoomReceiptState::Expired { at: fresh }),
        Err(RoomReceiptTransitionError::NotPending)
    );
    assert_eq!(
        control("ack", &item).disposition,
        RoomReceiptDisposition::Ack
    );
}

#[test]
fn content_admission_reserves_control_returns_before_saturation() {
    let routes = (0..ROOM_ROUTE_FANOUT_LIMIT)
        .map(|index| RoomRoute {
            destination: RoomDestination::Lane {
                lane: CanonicalUuid::new(uuid::Uuid::from_u128(index as u128 + 1)),
            },
            context: None,
            direction: None,
        })
        .collect::<Vec<_>>();
    let boundary_content =
        ROOM_ADMISSION_BYTES_LIMIT - ROOM_ROUTE_FANOUT_LIMIT * ROOM_CONTROL_RETURN_RESERVE_BYTES;
    let publication = message(routes.clone(), boundary_content as u32);
    let charge = publication.admission_charge().expect("boundary admits");
    assert_eq!(charge.aggregate_bytes as usize, ROOM_ADMISSION_BYTES_LIMIT);
    assert_eq!(
        message(routes, boundary_content as u32 + 1).admission_charge(),
        Err(RoomAdmissionError::AggregateBytes)
    );
}
