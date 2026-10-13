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

fn ask(respondent: serde_json::Value) -> RoomPublication {
    serde_json::from_value(serde_json::json!({
        "publication": "1kKRRsDR1",
        "author": {"kind": "lane", "lane": "018f82d4-6f59-7d06-92e3-bce913116301"},
        "source": {"kind": "journal", "fact": "1"},
        "content_bytes": 10,
        "kind": {"verb": "ask", "result": "1kKRRsDR3", "deadline": "2030-01-02T00:00:00.000Z"},
        "routes": [{"destination": respondent, "context": "1kKRRsDR2"}],
    }))
    .expect("ask")
}

fn question(requester: serde_json::Value, respondent: serde_json::Value) -> QuestionResultRecord {
    serde_json::from_value(serde_json::json!({
        "result": "1kKRRsDR3",
        "question": "1kKRRsDR1",
        "requester": requester,
        "respondent": respondent,
        "context": "1kKRRsDR2",
        "source": {"kind": "journal", "fact": "1"},
        "deadline": "2030-01-02T00:00:00.000Z",
    }))
    .expect("question")
}

fn control(disposition: &str, key: &RoomReceiptRef) -> RoomReceiptControl {
    serde_json::from_value(serde_json::json!({
        "disposition": disposition,
        "keys": [key],
        "source": {"kind": "journal", "fact": "2"},
    }))
    .expect("control")
}

fn answer(author: serde_json::Value) -> RoomPublication {
    serde_json::from_value(serde_json::json!({
        "publication": "1kKRRsDR4",
        "author": author,
        "source": {"kind": "journal", "fact": "3"},
        "content_bytes": 20,
        "kind": {"verb": "ans", "result": "1kKRRsDR3", "context": "1kKRRsDR2"},
        "routes": [],
    }))
    .expect("answer")
}

/// The result handle is the ASK's only delivery key: the steering key of
/// a lane respondent's directive, or the key of the operator's item, whose
/// NAK alone declines the result.
#[test]
fn the_result_handle_is_the_ask_delivery_key_on_either_side() {
    let operator = serde_json::json!({"kind": "operator"});
    let respondent =
        serde_json::json!({"kind": "lane", "lane": "018f82d4-6f59-7d06-92e3-bce913116302"});
    let to_lane = ask(respondent.clone());
    assert_eq!(to_lane.operator_item(), None);
    let record = question(operator.clone(), respondent.clone());
    assert_eq!(record.item(), None);
    assert!(crate::quote::steering_key(record.result().as_str()));
    let result_key = RoomReceiptRef::new(record.result().authority().clone());
    assert_eq!(record.control_result(&control("nak", &result_key)), None);

    let to_operator = ask(operator.clone());
    let record = question(respondent, operator);
    let item = record.item().expect("the operator respondent has an item");
    assert_eq!(item, result_key);
    assert_eq!(
        to_operator.operator_item(),
        Some((item.clone(), Some(&task("1kKRRsDR2"))))
    );
    assert_eq!(record.control_result(&control("ack", &item)), None);
    assert_eq!(
        record.control_result(&control("nak", &item)),
        Some(QuestionResultState::Declined { source: source(2) })
    );
    let publication_key = RoomReceiptRef::new(authority("1kKRRsDR1"));
    assert_eq!(
        record.control_result(&control("nak", &publication_key)),
        None
    );

    let mut conflated = serde_json::to_value(&record).unwrap();
    conflated["result"] = serde_json::json!("1kKRRsDR1");
    assert!(serde_json::from_value::<QuestionResultRecord>(conflated).is_err());
    let mut legacy = serde_json::to_value(&record).unwrap();
    legacy["receipt"] = serde_json::json!({"publication": "1kKRRsDR1", "route": "0"});
    assert!(serde_json::from_value::<QuestionResultRecord>(legacy).is_err());
    let mut self_handle =
        serde_json::to_value(ask(serde_json::json!({"kind": "operator"}))).expect("ask encodes");
    self_handle["kind"]["result"] = serde_json::json!("1kKRRsDR1");
    assert!(serde_json::from_value::<RoomPublication>(self_handle).is_err());
}

/// An answer returns to a lane requester through exactly one directive
/// and to the operator through the item keyed by the answer, never both.
#[test]
fn answer_return_binds_one_delivery_for_its_stored_requester() {
    let operator = serde_json::json!({"kind": "operator"});
    let respondent =
        serde_json::json!({"kind": "lane", "lane": "018f82d4-6f59-7d06-92e3-bce913116302"});
    let requester =
        serde_json::json!({"kind": "lane", "lane": "018f82d4-6f59-7d06-92e3-bce913116303"});
    let directive = lane("018f82d4-6f59-7d06-92e3-bce913116399");
    let before = instant("2030-01-01T00:00:00.000Z");

    let record = question(operator.clone(), respondent.clone());
    let QuestionResultState::Answered {
        return_delivery, ..
    } = record
        .answer_state(&answer(respondent.clone()), before, &[])
        .expect("the respondent answers the operator")
    else {
        panic!("answer is terminal");
    };
    assert_eq!(return_delivery.destination(), &RoomDestination::Operator {});
    assert_eq!(
        return_delivery.delivery(),
        &RoomDelivery::Item {
            item: RoomReceiptRef::new(authority("1kKRRsDR4"))
        }
    );
    assert_eq!(
        record.answer_state(&answer(respondent.clone()), before, &[directive]),
        Err(QuestionAnswerError::ReturnDelivery)
    );

    let record = question(requester, respondent.clone());
    let QuestionResultState::Answered {
        return_delivery, ..
    } = record
        .answer_state(&answer(respondent.clone()), before, &[directive])
        .expect("the respondent answers a lane")
    else {
        panic!("answer is terminal");
    };
    assert_eq!(
        return_delivery.delivery(),
        &RoomDelivery::Directive { directive }
    );
    for directed in [&[][..], &[directive, directive][..]] {
        assert_eq!(
            record.answer_state(&answer(respondent.clone()), before, directed),
            Err(QuestionAnswerError::ReturnDelivery)
        );
    }
    assert_eq!(
        record.answer_state(&answer(operator), before, &[directive]),
        Err(QuestionAnswerError::Respondent)
    );
    assert_eq!(
        record.answer_state(
            &answer(respondent),
            instant("2030-01-02T00:00:00.000Z"),
            &[directive]
        ),
        Err(QuestionAnswerError::Deadline)
    );
}

#[test]
fn lapsed_item_recovers_under_its_key_without_touching_the_result() {
    let record = question(
        serde_json::json!({"kind": "lane", "lane": "018f82d4-6f59-7d06-92e3-bce913116302"}),
        serde_json::json!({"kind": "operator"}),
    );
    let item = record.item().expect("operator item");
    let first = instant("2030-01-01T00:00:00.000Z");
    let mut receipt = RoomReceiptHead::pending(item.clone(), first);
    let mut result = QuestionResultHead::pending(record.result().clone());
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

    result
        .settle(QuestionResultState::TimedOut {})
        .expect("deadline wins");
    assert_eq!(record.deadline(), instant("2030-01-02T00:00:00.000Z"));
    assert_eq!(
        result.settle(QuestionResultState::Pending {}),
        Err(TerminalConflict)
    );
}

#[test]
fn immutable_question_deadline_and_one_terminal_winner_survive_replay() {
    let result = QuestionResultRef::new(authority("1kKRRsDR3"));
    let winner = QuestionResultState::TimedOut {};
    let mut head = QuestionResultHead {
        result,
        state: QuestionResultState::Pending {},
    };
    head.settle(winner.clone()).expect("first terminal wins");
    head.settle(winner).expect("exact replay is idempotent");
    assert_eq!(
        head.settle(QuestionResultState::Cancelled { source: source(4) }),
        Err(TerminalConflict)
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
    let charge = publication.admission_charge(0).expect("boundary admits");
    assert_eq!(charge.aggregate_bytes as usize, ROOM_ADMISSION_BYTES_LIMIT);
    assert_eq!(
        message(routes, boundary_content as u32 + 1).admission_charge(0),
        Err(RoomAdmissionError::AggregateBytes)
    );

    let control: RoomReceiptControl = serde_json::from_value(serde_json::json!({
        "disposition": "ack",
        "keys": ["1kKRRsDR1"],
        "source": {"kind": "journal", "fact": "5"},
    }))
    .expect("reserved ACK remains admissible independently of content saturation");
    assert_eq!(control.disposition, RoomReceiptDisposition::Ack);
}

#[test]
fn pending_result_limit_and_answer_context_are_exact() {
    let respondent =
        serde_json::json!({"kind": "lane", "lane": "018f82d4-6f59-7d06-92e3-bce913116302"});
    assert_eq!(
        ask(respondent.clone()).admission_charge(ROOM_PENDING_RESULT_LIMIT),
        Err(RoomAdmissionError::PendingResults)
    );

    let mut answer = serde_json::to_value(answer(respondent.clone())).expect("answer encodes");
    answer["kind"]["context"] = serde_json::json!("1kKRRsDR5");
    let answer: RoomPublication = serde_json::from_value(answer).expect("answer shape");
    let record = question(serde_json::json!({"kind": "operator"}), respondent);
    assert_eq!(
        record.answer_state(&answer, instant("2030-01-01T00:00:00.000Z"), &[]),
        Err(QuestionAnswerError::Context)
    );
}
