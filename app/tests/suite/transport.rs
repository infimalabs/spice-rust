//! What the browser is allowed to believe.
//!
//! Every fixture reaches the app as an encoded document because the socket
//! carries bytes and the app's only honest input is bytes. Valid response
//! documents are first constructed from their wire DTOs, so schema drift is a
//! compile error; deliberately malformed decoder inputs remain raw bytes.
//!
//! The streamed fixtures name [`wire::body::TaskPhased`] rather than a shape
//! declared here, so the fact they carry is one the vocabulary registers. A
//! locally declared body would have let this suite pass against a kind no
//! producer mints.

use app::{Admitted, Applied, BrowserState, ChainEnd, Session};
use serde::{Deserialize, Serialize};
use wire::{
    Accepted, CanonicalUuid, Envelope, Fact, FaultClass, Outcome, Overflow, Packed, Reply,
    ReplyPosition, Response, Seq, SeqRange, WireVersion, body,
};

use super::support;

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

fn next_session() -> Session {
    Session::open(uuid(NEXT_CAPABILITY), uuid(NEXT_EPOCH))
}

/// The principal receipt that scopes browser state and every admitted frame.
fn accepted(session: &Session, epoch: &str) -> Admitted<Accepted> {
    let bytes = format!(
        r#"{{ "wire": 1, "actor": "lane:018f82d4-6f59-7d06-92e3-bce9131163c2", "epoch": "{epoch}", "release": "1.2.3" }}"#
    );
    session
        .admit_accepted(bytes.as_bytes())
        .expect("the fixture acceptance is admitted")
}

#[test]
fn an_accepted_frame_cannot_echo_the_session_capability() {
    let bytes = format!(
        concat!(
            r#"{{ "wire": 1, "actor": "lane:018f82d4-6f59-7d06-92e3-bce9131163c2", "#,
            r#""epoch": "{EPOCH}", "release": "1.2.3", "capability": "{CAPABILITY}" }}"#,
        ),
        EPOCH = EPOCH,
        CAPABILITY = CAPABILITY,
    );

    let fault = session()
        .admit_accepted(bytes.as_bytes())
        .expect_err("the resident return frame cannot echo a live credential");

    assert_eq!(fault.class, FaultClass::Protocol);
    assert_eq!(fault.reason, "frame::undecodable");
    assert!(
        !fault.detail.contains(CAPABILITY),
        "the decoder diagnostic echoed the credential: {}",
        fault.detail
    );
}

fn browser_state<T>() -> BrowserState<T> {
    BrowserState::new(&accepted(&session(), EPOCH))
}

/// The body of the reply fixture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Status {
    task: String,
    status: String,
}

/// A ranked row, standing in for whatever a pane lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Row(String);

/// One envelope document at `seq` and `version`, as bytes off the stream.
///
/// `at` is written at the one admitted resolution rather than built from a
/// clock, so this fixture is stable and carries no ambient time.
fn envelope_bytes_with_cause(version: u32, seq: u64, cause: Option<u64>) -> Vec<u8> {
    let cause = cause.map_or_else(String::new, |cause| format!("  \"cause\": {cause},\n"));
    format!(
        r#"{{
  "v": {version},
  "seq": {seq},
  "at": "2030-04-12T18:44:09.271Z",
  "actor": "lane:018f82d4-6f59-7d06-92e3-bce9131163c2",
  "origin": "task:1kKRRsDP",
{cause}  "fact": {{ "kind": "task::phased", "task": "1kKRRsDP", "phase": "verify" }}
}}"#
    )
    .into_bytes()
}

/// One non-root envelope document, preserving the original fixture shape.
fn envelope_bytes(version: u32, seq: u64) -> Vec<u8> {
    envelope_bytes_with_cause(version, seq, Some(48190))
}

/// One streamed fact at `seq`, admitted the way state must receive it.
fn envelope(seq: u64) -> Admitted<Envelope<Fact<body::TaskPhased>>> {
    session()
        .admit_envelope(&envelope_bytes(1, seq))
        .expect("fixture envelope is admitted")
}

/// One streamed fact whose immediate cause is explicit or omitted at a root.
fn causal_envelope(seq: u64, cause: Option<u64>) -> Admitted<Envelope<Fact<body::TaskPhased>>> {
    session()
        .admit_envelope(&envelope_bytes_with_cause(1, seq, cause))
        .expect("causal fixture envelope is admitted")
}

/// One committed reply document at `version`, answering `epoch`.
fn response(version: u32, epoch: &str) -> Vec<u8> {
    support::response(
        WireVersion(version),
        uuid("018f82d4-6f59-7d06-92e3-bce9131163c4"),
        uuid(epoch),
        Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(48213),
                last: seq(48214),
            }),
            body: Status {
                task: "SPICE-1kKRRsDP".to_owned(),
                status: "done".to_owned(),
            },
        }),
    )
}

/// One packed reply admitted through the same response door as every answer.
fn packed_answer(
    rows: &[&str],
    overflow: Option<(u64, u64)>,
    horizon: u64,
) -> Admitted<Response<Packed<Row>>> {
    let body = Packed {
        rows: rows.iter().map(|row| Row((*row).to_owned())).collect(),
        overflow: overflow.map(|(omitted, continuation)| Overflow {
            omitted,
            continuation: seq(continuation),
        }),
        horizon: seq(horizon),
    };
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid("018f82d4-6f59-7d06-92e3-bce9131163c4"),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(horizon)),
            body,
        }),
    );
    session()
        .admit_frame(&bytes)
        .expect("the packed fixture answer is admitted")
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
fn a_requirement_unaware_app_generation_is_stopped_at_discovery() {
    let fault = Session::admit_app_protocol(Some(wire::AppProtocolVersion(3)))
        .expect_err("a requirement-unaware browser does not cross current app DTOs");
    assert_eq!(fault.reason, "bootstrap::app-protocol-mismatch");
}

#[test]
fn the_current_app_protocol_is_admitted_at_discovery() {
    Session::admit_app_protocol(Some(wire::AppProtocolVersion::CURRENT))
        .expect("the current app reaches bootstrap decoding");
}

#[test]
fn an_absent_app_protocol_replaces_the_current_document() {
    let fault = Session::admit_app_protocol(None)
        .expect_err("a response without the pre-socket clock does not open a socket");
    assert_eq!(fault.reason, "bootstrap::app-protocol-mismatch");
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
    let admitted = session()
        .admit_frame::<Status>(&response(1, EPOCH))
        .expect("a current frame is admitted");
    let frame = admitted.get();

    assert_eq!(frame.v, WireVersion::CURRENT);
    assert_eq!(frame.epoch, uuid(EPOCH));
}

#[test]
fn a_current_streamed_fact_is_admitted() {
    let admitted = session()
        .admit_envelope::<body::TaskPhased>(&envelope_bytes(1, 48213))
        .expect("a current envelope is admitted");
    let frame = admitted.get();

    assert_eq!(frame.v, WireVersion::CURRENT);
    assert_eq!(frame.seq, seq(48213));
    assert_eq!(frame.fact.body.phase.as_str(), "verify");
}

#[test]
fn a_streamed_fact_at_another_wire_version_faults_and_never_reaches_state() {
    let fault = session()
        .admit_envelope::<body::TaskPhased>(&envelope_bytes(99, 48213))
        .expect_err("a foreign version faults");

    assert_eq!(fault.class, FaultClass::Protocol);
    assert_eq!(fault.reason, "frame::wire-mismatch");
    assert!(fault.detail.contains("99"), "{}", fault.detail);

    // The door is the only route in, so a state that saw this stream is
    // indistinguishable from one that saw nothing at all.
    let state = browser_state::<Row>();
    assert_eq!(state.cursor(), None);
}

#[test]
fn a_streamed_fact_that_is_not_a_frame_faults_with_the_shared_reason() {
    let fault = session()
        .admit_envelope::<body::TaskPhased>(b"{\"v\": 1, \"seq\":")
        .expect_err("truncated bytes fault");

    assert_eq!(fault.class, FaultClass::Protocol);
    assert_eq!(fault.reason, "frame::undecodable");
}

#[test]
fn both_doors_name_the_same_reason_for_the_same_failure() {
    let session = session();

    let response_fault = session
        .admit_frame::<Status>(&response(99, EPOCH))
        .expect_err("a foreign version faults");
    let envelope_fault = session
        .admit_envelope::<body::TaskPhased>(&envelope_bytes(99, 48213))
        .expect_err("a foreign version faults");

    assert_eq!(response_fault.reason, envelope_fault.reason);
    assert_eq!(response_fault.detail, envelope_fault.detail);
    assert_eq!(response_fault.class, envelope_fault.class);
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
    let state = browser_state::<Row>();

    assert_eq!(state.cursor(), None);
    assert_eq!(state.horizon(), None);
    assert_eq!(state.overflow(), None);
    assert!(state.rows().is_empty());
    assert!(!state.is_overflowing());
}

#[test]
fn monotone_frames_advance_the_cursor() {
    let mut state = browser_state::<Row>();

    assert_eq!(state.apply(&envelope(48213)), Applied::Advanced);
    assert_eq!(state.cursor(), Some(seq(48213)));
    assert_eq!(state.apply(&envelope(48214)), Applied::Advanced);
    assert_eq!(state.cursor(), Some(seq(48214)));
}

#[test]
fn a_same_epoch_frame_from_another_capability_never_reaches_state() {
    let foreign = Session::open(uuid(NEXT_CAPABILITY), uuid(EPOCH))
        .admit_envelope::<body::TaskPhased>(&envelope_bytes(1, 48213))
        .expect("the other session admits its own frame");
    let mut state = browser_state::<Row>();

    assert_eq!(state.apply(&foreign), Applied::ForeignScope);
    assert_eq!(state.cursor(), None);
    assert!(state.walk(seq(48213)).links().is_empty());
}

#[test]
fn a_repeated_delivery_is_named_a_duplicate_and_moves_nothing() {
    let mut state = browser_state::<Row>();
    state.apply(&envelope(48213));

    let applied = state.apply(&envelope(48213));

    assert_eq!(applied, Applied::Duplicate);
    assert!(!applied.advanced());
    assert_eq!(state.cursor(), Some(seq(48213)));
}

#[test]
fn a_reordered_delivery_is_named_stale_and_never_lowers_the_cursor() {
    let mut state = browser_state::<Row>();
    state.apply(&envelope(48214));

    let applied = state.apply(&envelope(48190));

    assert_eq!(applied, Applied::Stale);
    assert_eq!(state.cursor(), Some(seq(48214)));
}

#[test]
fn a_gap_advances_without_being_repaired() {
    let mut state = browser_state::<Row>();
    state.apply(&envelope(48213));

    assert_eq!(state.apply(&envelope(48999)), Applied::Advanced);
    assert_eq!(state.cursor(), Some(seq(48999)));
}

#[test]
fn a_chain_walk_ends_at_the_received_envelope_that_omits_cause() {
    let mut state = browser_state::<Row>();
    state.apply(&causal_envelope(48190, None));
    state.apply(&causal_envelope(48213, Some(48190)));
    state.apply(&causal_envelope(48214, Some(48213)));

    let walk = state.walk(seq(48214));

    assert_eq!(
        walk.links()
            .iter()
            .map(app::CausalLink::seq)
            .collect::<Vec<_>>(),
        vec![seq(48214), seq(48213), seq(48190)]
    );
    assert_eq!(walk.end(), ChainEnd::Root);
    assert_eq!(walk.links().last().and_then(app::CausalLink::cause), None);
    assert!(
        walk.links()
            .iter()
            .all(|link| link.origin().to_string() == "task:1kKRRsDP")
    );
}

#[test]
fn a_dangling_cause_is_reported_without_inventing_its_position() {
    let mut state = browser_state::<Row>();
    state.apply(&causal_envelope(48214, Some(48213)));

    let walk = state.walk(seq(48214));
    let unseen = state.walk(seq(48999));

    assert_eq!(
        walk.links()
            .iter()
            .map(app::CausalLink::seq)
            .collect::<Vec<_>>(),
        vec![seq(48214)]
    );
    assert_eq!(walk.end(), ChainEnd::Dangling(seq(48213)));
    assert!(unseen.links().is_empty());
    assert_eq!(unseen.end(), ChainEnd::Dangling(seq(48999)));
}

#[test]
fn a_cause_cycle_is_reported_after_each_received_link_once() {
    let mut state = browser_state::<Row>();
    state.apply(&causal_envelope(48213, Some(48214)));
    state.apply(&causal_envelope(48214, Some(48213)));

    let walk = state.walk(seq(48214));

    assert_eq!(
        walk.links()
            .iter()
            .map(app::CausalLink::seq)
            .collect::<Vec<_>>(),
        vec![seq(48214), seq(48213)]
    );
    assert_eq!(walk.end(), ChainEnd::Cycle(seq(48214)));
}

#[test]
fn a_received_stale_frame_can_complete_a_previously_dangling_walk() {
    let mut state = browser_state::<Row>();
    state.apply(&causal_envelope(48214, Some(48213)));
    assert_eq!(state.walk(seq(48214)).end(), ChainEnd::Dangling(seq(48213)));

    assert_eq!(state.apply(&causal_envelope(48213, None)), Applied::Stale);

    let completed = state.walk(seq(48214));
    assert_eq!(completed.end(), ChainEnd::Root);
    assert_eq!(state.cursor(), Some(seq(48214)));
}

#[test]
fn a_received_stale_frame_below_a_full_window_leaves_the_walk_dangling() {
    let window = u64::try_from(app::CAUSAL_HEADERS).expect("the window fits a position");
    let root = 1;
    let last = root + window;
    let mut state = browser_state::<Row>();
    for position in (root + 1)..=last {
        state.apply(&causal_envelope(position, Some(position - 1)));
    }
    assert_eq!(state.walk(seq(last)).end(), ChainEnd::Dangling(seq(root)));

    assert_eq!(state.apply(&causal_envelope(root, None)), Applied::Stale);

    let still_dangling = state.walk(seq(last));
    assert_eq!(still_dangling.end(), ChainEnd::Dangling(seq(root)));
    assert_eq!(still_dangling.links().len(), app::CAUSAL_HEADERS);
    assert!(state.walk(seq(root)).links().is_empty());
    assert_eq!(state.cursor(), Some(seq(last)));
}

#[test]
fn retention_stops_at_the_stated_window_and_dangles_where_it_evicted() {
    let window = u64::try_from(app::CAUSAL_HEADERS).expect("the window fits a position");
    let root = 1;
    let last = root + window - 1;
    let mut state = browser_state::<Row>();

    state.apply(&causal_envelope(root, None));
    for position in (root + 1)..=last {
        state.apply(&causal_envelope(position, Some(position - 1)));
    }

    // A window exactly full still holds its root, so the eviction below is what
    // changes this walk rather than the chain having been short all along.
    let whole = state.walk(seq(last));
    assert_eq!(whole.end(), ChainEnd::Root);
    assert_eq!(whole.links().len(), app::CAUSAL_HEADERS);

    state.apply(&causal_envelope(last + 1, Some(last)));

    let walk = state.walk(seq(last + 1));
    assert_eq!(walk.end(), ChainEnd::Dangling(seq(root)));
    assert_eq!(walk.links().len(), app::CAUSAL_HEADERS);
    assert_eq!(
        walk.links().last().and_then(app::CausalLink::cause),
        Some(seq(root))
    );
    assert!(state.walk(seq(root)).links().is_empty());
    assert_eq!(state.walk(seq(root)).end(), ChainEnd::Dangling(seq(root)));
    assert_eq!(state.cursor(), Some(seq(last + 1)));
}

#[test]
fn a_later_answer_replaces_the_earlier_one_whole() {
    let mut state = browser_state();
    assert!(state.replace(&packed_answer(&["first", "second"], None, 48213)));

    assert!(state.replace(&packed_answer(&["third"], None, 48214)));

    assert_eq!(state.rows(), [Row("third".to_owned())].as_slice());
    assert_eq!(state.horizon(), Some(seq(48214)));
}

#[test]
fn overflow_is_carried_from_the_answer_rather_than_inferred_from_length() {
    let mut state = browser_state();
    assert!(state.replace(&packed_answer(&["only"], Some((41, 48250)), 48213)));

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
    let mut state = browser_state();
    assert!(state.replace(&packed_answer(&["only"], Some((41, 48250)), 48213)));

    assert!(state.replace(&packed_answer(&["only", "rest"], None, 48214)));

    assert!(!state.is_overflowing());
    assert_eq!(state.overflow(), None);
}

#[test]
fn reconnecting_to_a_new_generation_starts_empty() {
    let mut state = browser_state();
    state.apply(&envelope(48213));
    assert!(state.replace(&packed_answer(&["stale"], None, 48213)));

    state.reconnect(&accepted(&next_session(), NEXT_EPOCH));

    assert_eq!(state.epoch(), uuid(NEXT_EPOCH));
    assert_eq!(state.cursor(), None);
    assert_eq!(state.horizon(), None);
    assert!(state.rows().is_empty());
    assert!(state.walk(seq(48213)).links().is_empty());
    assert_eq!(state.walk(seq(48213)).end(), ChainEnd::Dangling(seq(48213)));
}

#[test]
fn reconnecting_to_another_capability_in_the_same_epoch_starts_empty() {
    let mut state = browser_state();
    state.apply(&envelope(48213));
    assert!(state.replace(&packed_answer(&["private"], None, 48213)));
    let replacement = Session::open(uuid(NEXT_CAPABILITY), uuid(EPOCH));

    state.reconnect(&accepted(&replacement, EPOCH));

    assert_eq!(state.epoch(), uuid(EPOCH));
    assert_eq!(state.cursor(), None);
    assert_eq!(state.horizon(), None);
    assert!(state.rows().is_empty());
}

#[test]
fn reconnecting_to_the_same_generation_resumes_where_it_left_off() {
    let mut state = browser_state();
    state.apply(&envelope(48213));
    assert!(state.replace(&packed_answer(&["live"], None, 48213)));

    state.reconnect(&accepted(&session(), EPOCH));

    assert_eq!(state.epoch(), uuid(EPOCH));
    assert_eq!(state.cursor(), Some(seq(48213)));
    assert_eq!(state.rows(), [Row("live".to_owned())].as_slice());
}

#[test]
fn a_resumed_state_still_refuses_positions_it_already_applied() {
    let mut state = browser_state::<Row>();
    state.apply(&envelope(48213));

    state.reconnect(&accepted(&session(), EPOCH));

    assert_eq!(state.apply(&envelope(48213)), Applied::Duplicate);
}

#[test]
fn a_frame_admitted_by_the_session_is_the_frame_state_applies() {
    let session = session();
    let mut state = BrowserState::<Row>::new(&accepted(&session, EPOCH));

    session
        .admit_frame::<Status>(&response(1, EPOCH))
        .expect("the fixture frame is admitted");
    state.apply(&envelope(48213));

    assert_eq!(state.epoch(), session.epoch());
    assert_eq!(state.cursor(), Some(seq(48213)));
}
