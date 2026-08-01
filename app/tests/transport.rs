//! What the browser is allowed to believe.
//!
//! Every fixture here is an encoded document rather than a constructed value,
//! because the socket carries bytes and the app's only honest input is bytes.
//! A test that built a DTO in memory would prove the app agrees with itself;
//! these prove it agrees with the wire.

use app::{Applied, BrowserState, Session};
use serde::{Deserialize, Serialize};
use wire::{CanonicalUuid, Envelope, FaultClass, Overflow, Packed, Seq, WireVersion, body};

/// Capability this session opens with.
const CAPABILITY: &str = "018f82d4-6f59-7d06-92e3-bce9131163c1";
/// Generation this session addresses.
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c3";
/// A generation that replaced the one above.
const NEXT_EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c9";
/// A capability a later discovery read presents.
const NEXT_CAPABILITY: &str = "018f82d4-6f59-7d06-92e3-bce9131163ca";

/// Parse a canonical uuid fixture, which is spelled correctly by construction.
fn uuid(text: &str) -> CanonicalUuid {
    text.parse().expect("fixture uuid is canonical")
}

/// A session opened against [`CAPABILITY`] and [`EPOCH`].
fn session() -> Session {
    Session::open(uuid(CAPABILITY), uuid(EPOCH))
}

/// The body of the reply fixture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Status {
    task: String,
    status: String,
}

/// A ranked row, standing in for whatever a pane lists.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row(&'static str);

/// One envelope document at `seq`, decoded the way a frame arrives.
///
/// `at` is written at the one admitted resolution rather than built from a
/// clock, so this fixture is stable and carries no ambient time.
fn envelope(seq: u64) -> Envelope<body::TaskPhased> {
    let document = format!(
        r#"{{
  "v": 1,
  "seq": {seq},
  "at": "2030-04-12T18:44:09.271Z",
  "actor": "lane:018f82d4-6f59-7d06-92e3-bce9131163c2",
  "origin": "task:SPICE-aBcD",
  "cause": 48190,
  "fact": {{ "kind": "task::phased", "task": "SPICE-aBcD", "phase": "verify" }}
}}"#
    );
    wire::decode(document.as_bytes()).expect("fixture envelope decodes")
}

/// One committed reply document at `version`, answering `epoch`.
fn response(version: u32, epoch: &str) -> Vec<u8> {
    format!(
        r#"{{
  "v": {version},
  "id": "018f82d4-6f59-7d06-92e3-bce9131163c4",
  "epoch": "{epoch}",
  "outcome": {{
    "kind": "reply",
    "seq": {{ "first": 48213, "last": 48214 }},
    "body": {{ "task": "SPICE-aBcD", "status": "done" }}
  }}
}}"#
    )
    .into_bytes()
}

/// A position, which every fixture spells above zero.
fn seq(value: u64) -> Seq {
    Seq::new(value).expect("fixture position is non-zero")
}

#[test]
fn hello_opens_at_the_version_this_build_speaks() {
    let hello = session().hello();

    assert_eq!(hello.wire, WireVersion::CURRENT);
    assert_eq!(hello.capability, uuid(CAPABILITY));
    assert_eq!(hello.epoch, uuid(EPOCH));
}

#[test]
fn the_opening_a_session_offers_is_the_one_a_resident_admits() {
    let bytes = wire::encode(&session().hello()).expect("hello encodes");
    let crossed: wire::Hello = wire::decode(&bytes).expect("hello decodes");

    assert!(crossed.admit(uuid(EPOCH)).is_ok());
}

#[test]
fn an_opening_against_a_replaced_generation_is_rejected_by_the_resident() {
    let hello = session().hello();

    let rejected = hello
        .admit(uuid(NEXT_EPOCH))
        .expect_err("a moved epoch rejects");

    assert!(matches!(rejected, wire::Rejected::Epoch(_)));
}

#[test]
fn the_same_capability_is_admitted_on_a_later_discovery_read() {
    assert!(session().admit_capability(uuid(CAPABILITY)).is_ok());
}

#[test]
fn a_replaced_capability_faults_rather_than_refreshing_the_session() {
    let fault = session()
        .admit_capability(uuid(NEXT_CAPABILITY))
        .expect_err("a replaced capability faults");

    assert_eq!(fault.class, FaultClass::Protocol);
    assert_eq!(fault.reason, "session::capability-replaced");
}

#[test]
fn a_current_frame_for_this_generation_is_admitted() {
    let frame = session()
        .admit_frame::<Status>(&response(1, EPOCH))
        .expect("a current frame is admitted");

    assert_eq!(frame.v, WireVersion::CURRENT);
    assert_eq!(frame.epoch, uuid(EPOCH));
}

#[test]
fn a_frame_at_another_wire_version_faults_and_names_both_versions() {
    let fault = session()
        .admit_frame::<Status>(&response(2, EPOCH))
        .expect_err("a foreign version faults");

    assert_eq!(fault.class, FaultClass::Protocol);
    assert_eq!(fault.reason, "frame::wire-mismatch");
    assert!(fault.detail.contains('2'), "{}", fault.detail);
    assert!(
        fault.detail.contains(&WireVersion::CURRENT.to_string()),
        "{}",
        fault.detail
    );
}

#[test]
fn a_frame_answering_another_generation_faults_and_names_both_epochs() {
    let fault = session()
        .admit_frame::<Status>(&response(1, NEXT_EPOCH))
        .expect_err("a foreign epoch faults");

    assert_eq!(fault.class, FaultClass::Protocol);
    assert_eq!(fault.reason, "frame::epoch-mismatch");
    assert!(fault.detail.contains(NEXT_EPOCH), "{}", fault.detail);
    assert!(fault.detail.contains(EPOCH), "{}", fault.detail);
}

#[test]
fn bytes_that_are_not_a_frame_fault_rather_than_being_repaired() {
    let fault = session()
        .admit_frame::<Status>(b"{\"v\": 1, \"id\":")
        .expect_err("truncated bytes fault");

    assert_eq!(fault.class, FaultClass::Protocol);
    assert_eq!(fault.reason, "frame::undecodable");
    assert!(!fault.detail.is_empty());
}

#[test]
fn a_frame_missing_a_required_field_faults_as_undecodable() {
    let missing_epoch = br#"{"v": 1, "id": "018f82d4-6f59-7d06-92e3-bce9131163c4"}"#;

    let fault = session()
        .admit_frame::<Status>(missing_epoch)
        .expect_err("an incomplete frame faults");

    assert_eq!(fault.reason, "frame::undecodable");
}

#[test]
fn an_untouched_state_invents_no_cursor_horizon_or_rows() {
    let state = BrowserState::<Row>::new(uuid(EPOCH));

    assert_eq!(state.cursor(), None);
    assert_eq!(state.horizon(), None);
    assert_eq!(state.overflow(), None);
    assert!(state.rows().is_empty());
    assert!(!state.is_overflowing());
}

#[test]
fn monotone_frames_advance_the_cursor() {
    let mut state = BrowserState::<Row>::new(uuid(EPOCH));

    assert_eq!(state.apply(&envelope(48213)), Applied::Advanced);
    assert_eq!(state.cursor(), Some(seq(48213)));
    assert_eq!(state.apply(&envelope(48214)), Applied::Advanced);
    assert_eq!(state.cursor(), Some(seq(48214)));
}

#[test]
fn a_repeated_delivery_is_named_a_duplicate_and_moves_nothing() {
    let mut state = BrowserState::<Row>::new(uuid(EPOCH));
    state.apply(&envelope(48213));

    let applied = state.apply(&envelope(48213));

    assert_eq!(applied, Applied::Duplicate);
    assert!(!applied.advanced());
    assert_eq!(state.cursor(), Some(seq(48213)));
}

#[test]
fn a_reordered_delivery_is_named_stale_and_never_lowers_the_cursor() {
    let mut state = BrowserState::<Row>::new(uuid(EPOCH));
    state.apply(&envelope(48214));

    let applied = state.apply(&envelope(48190));

    assert_eq!(applied, Applied::Stale);
    assert_eq!(state.cursor(), Some(seq(48214)));
}

#[test]
fn a_gap_advances_without_being_repaired() {
    let mut state = BrowserState::<Row>::new(uuid(EPOCH));
    state.apply(&envelope(48213));

    assert_eq!(state.apply(&envelope(48999)), Applied::Advanced);
    assert_eq!(state.cursor(), Some(seq(48999)));
}

#[test]
fn a_later_answer_replaces_the_earlier_one_whole() {
    let mut state = BrowserState::new(uuid(EPOCH));
    state.replace(Packed {
        rows: vec![Row("first"), Row("second")],
        overflow: None,
        horizon: seq(48213),
    });

    state.replace(Packed {
        rows: vec![Row("third")],
        overflow: None,
        horizon: seq(48214),
    });

    assert_eq!(state.rows(), [Row("third")].as_slice());
    assert_eq!(state.horizon(), Some(seq(48214)));
}

#[test]
fn overflow_is_carried_from_the_answer_rather_than_inferred_from_length() {
    let mut state = BrowserState::new(uuid(EPOCH));
    state.replace(Packed {
        rows: vec![Row("only")],
        overflow: Some(Overflow {
            omitted: 41,
            continuation: seq(48250),
        }),
        horizon: seq(48213),
    });

    assert!(state.is_overflowing());
    assert_eq!(
        state.overflow(),
        Some(Overflow {
            omitted: 41,
            continuation: seq(48250)
        })
    );
    assert_eq!(state.rows().len(), 1);
}

#[test]
fn a_complete_answer_clears_the_overflow_an_earlier_one_declared() {
    let mut state = BrowserState::new(uuid(EPOCH));
    state.replace(Packed {
        rows: vec![Row("only")],
        overflow: Some(Overflow {
            omitted: 41,
            continuation: seq(48250),
        }),
        horizon: seq(48213),
    });

    state.replace(Packed {
        rows: vec![Row("only"), Row("rest")],
        overflow: None,
        horizon: seq(48214),
    });

    assert!(!state.is_overflowing());
    assert_eq!(state.overflow(), None);
}

#[test]
fn reconnecting_to_a_new_generation_starts_empty() {
    let mut state = BrowserState::new(uuid(EPOCH));
    state.apply(&envelope(48213));
    state.replace(Packed {
        rows: vec![Row("stale")],
        overflow: None,
        horizon: seq(48213),
    });

    state.reconnect(uuid(NEXT_EPOCH));

    assert_eq!(state.epoch(), uuid(NEXT_EPOCH));
    assert_eq!(state.cursor(), None);
    assert_eq!(state.horizon(), None);
    assert!(state.rows().is_empty());
}

#[test]
fn reconnecting_to_the_same_generation_resumes_where_it_left_off() {
    let mut state = BrowserState::new(uuid(EPOCH));
    state.apply(&envelope(48213));
    state.replace(Packed {
        rows: vec![Row("live")],
        overflow: None,
        horizon: seq(48213),
    });

    state.reconnect(uuid(EPOCH));

    assert_eq!(state.epoch(), uuid(EPOCH));
    assert_eq!(state.cursor(), Some(seq(48213)));
    assert_eq!(state.rows(), [Row("live")].as_slice());
}

#[test]
fn a_resumed_state_still_refuses_positions_it_already_applied() {
    let mut state = BrowserState::<Row>::new(uuid(EPOCH));
    state.apply(&envelope(48213));

    state.reconnect(uuid(EPOCH));

    assert_eq!(state.apply(&envelope(48213)), Applied::Duplicate);
}

#[test]
fn a_frame_admitted_by_the_session_is_the_frame_state_applies() {
    let session = session();
    let mut state = BrowserState::<Row>::new(session.epoch());

    session
        .admit_frame::<Status>(&response(1, EPOCH))
        .expect("the fixture frame is admitted");
    state.apply(&envelope(48213));

    assert_eq!(state.epoch(), session.epoch());
    assert_eq!(state.cursor(), Some(seq(48213)));
}
