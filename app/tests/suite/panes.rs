//! What each capability is shown, and what it is offered.
//!
//! Every principal here is decoded from an acceptance document rather than
//! constructed, because that is the only way the app can obtain one: the
//! resident derives actor and rights when it accepts the connection, and a
//! test that built a `Principal` directly would be proving the app agrees with
//! itself about authority it is not allowed to assert.
//!
//! `app` no longer holds a classification to restate: each command's authority
//! is a field on its `wire` law row, and the panes read it there. So the tests
//! below assert the behaviour that field buys, and the one set written out by
//! hand is the expected operator set, which is written out precisely because an
//! oracle derived the way the pane derives would agree with any classification.

use std::collections::BTreeSet;

use app::{
    Admitted, BrowserState, Declined, GrantPane, Principal, ReadPane, Session, SteerPane, settled,
};
use serde::{Deserialize, Serialize};
use wire::{
    Accepted, Answer, Authority, CanonicalUuid, Command, CommandKind, Derived, Digest, FaultClass,
    FoldName, Name, Outcome, Overflow, Packed, Refusal, Reply, ReplyPosition, Response, Seq,
    TrustState, WireVersion, Witness, WitnessKind,
};

use super::support;

/// Capability the operator session opens with.
const OPERATOR_CAPABILITY: &str = "018f82d4-6f59-7d06-92e3-bce9131163c1";
/// Capability the lane session opens with.
const LANE_CAPABILITY: &str = "018f82d4-6f59-7d06-92e3-bce9131163c6";
/// Generation this session addresses.
const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c3";
/// A generation that replaced the one above.
const NEXT_EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c9";
/// Capability from the replacement generation.
const NEXT_CAPABILITY: &str = "018f82d4-6f59-7d06-92e3-bce9131163ca";
/// The lane a lane-scoped capability is bound to.
const LANE: &str = "018f82d4-6f59-7d06-92e3-bce9131163c2";
/// Identity this fixture mints for a submitted command.
const COMMAND: &str = "018f82d4-6f59-7d06-92e3-bce9131163c4";
/// Identity of some other command in flight at the same moment.
const OTHER_COMMAND: &str = "018f82d4-6f59-7d06-92e3-bce9131163c5";
/// Confidential tape bytes admitted only for the operator fixture.
const RAW_TAPE_SPAN: &str = "raw transcript span: authorization bearer alpha";
/// Credential-bearing consent payload admitted only for the operator fixture.
const CONSENT_CREDENTIAL: &str = "consent request payload: credential beta";

/// Parse a canonical uuid fixture, which is spelled correctly by construction.
fn uuid(text: &str) -> CanonicalUuid {
    text.parse().expect("fixture uuid is canonical")
}

/// A position, which every fixture spells above zero.
fn seq(value: u64) -> Seq {
    Seq::new(value).expect("fixture position is non-zero")
}

fn operator_session() -> Session {
    Session::open(uuid(OPERATOR_CAPABILITY), uuid(EPOCH))
}

fn lane_session() -> Session {
    Session::open(uuid(LANE_CAPABILITY), uuid(EPOCH))
}

fn next_session() -> Session {
    Session::open(uuid(NEXT_CAPABILITY), uuid(NEXT_EPOCH))
}

/// A ranked row, standing in for whatever a pane lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Row(String);

/// The body of a command and of the reply that answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Note {
    task: String,
    note: String,
}

/// The resident's acceptance naming `actor`, as bytes off the socket.
fn accepted_bytes(actor: &str, epoch: &str) -> Vec<u8> {
    format!(r#"{{ "wire": 1, "actor": "{actor}", "epoch": "{epoch}", "release": "1.2.3" }}"#)
        .into_bytes()
}

/// The resident's admitted principal receipt for one capability-scoped session.
fn accepted(session: &Session, actor: &str, epoch: &str) -> Admitted<Accepted> {
    session
        .admit_accepted(&accepted_bytes(actor, epoch))
        .expect("the fixture acceptance is admitted")
}

/// The principal the resident named, admitted the way panes must receive it.
fn principal(session: &Session, actor: &str) -> Principal {
    Principal::admitted(&accepted(session, actor, EPOCH))
}

/// A session the resident accepted as the operator.
fn operator() -> Principal {
    principal(&operator_session(), "operator")
}

/// A session the resident accepted as one seated lane.
fn lane() -> Principal {
    principal(&lane_session(), &format!("lane:{LANE}"))
}

/// State holding one complete answer from this generation.
fn answered() -> BrowserState<Row> {
    let session = lane_session();
    let mut state = BrowserState::new(&accepted(&session, &format!("lane:{LANE}"), EPOCH));
    assert!(state.replace(&packed_answer(
        &session,
        &["SPICE-1kKRRsDP", "SPICE-1kKRRsDQ"],
        Some((41, 48250)),
        48213,
    )));
    state
}

/// The one witness `task::note` requires.
fn existence() -> Vec<Witness> {
    vec![Witness::Existence {
        task: "1kKRRsDP".parse().expect("fixture task stamp"),
    }]
}

/// The two witnesses `consent::decide` requires.
fn consent_witnesses() -> Vec<Witness> {
    vec![
        Witness::Consent {
            request: uuid(OTHER_COMMAND),
            answer: Answer::Asked,
        },
        Witness::Rule {
            name: "review".parse::<Name>().expect("fixture rule name"),
        },
    ]
}

/// The one witness `trust::grant` requires.
fn trust() -> Vec<Witness> {
    vec![Witness::Trust {
        digest: "a".repeat(64).parse::<Digest>().expect("fixture digest"),
        standing: TrustState::Pended,
    }]
}

/// A note body, as one command's registered DTO.
fn note() -> Note {
    Note {
        task: "SPICE-1kKRRsDP".to_owned(),
        note: "observed in review".to_owned(),
    }
}

/// One typed answer to `id`, encoded before it crosses the session door.
fn response(id: &str, outcome: Outcome<Note>) -> Admitted<Response<Note>> {
    let bytes = support::response(WireVersion::CURRENT, uuid(id), uuid(EPOCH), outcome);
    lane_session()
        .admit_frame::<Note>(&bytes)
        .expect("the fixture answer is admitted")
}

/// One packed reply admitted through the session before browser state sees it.
fn packed_answer(
    session: &Session,
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
        uuid(COMMAND),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(horizon)),
            body,
        }),
    );
    session
        .admit_frame(&bytes)
        .expect("the packed fixture answer is admitted")
}

/// A command a lane is offered, built through the pane that offers it.
fn noted(principal: &Principal) -> Command<Note> {
    SteerPane::over(principal)
        .command(uuid(COMMAND), CommandKind::TaskNote, existence(), note())
        .expect("a lane is offered task::note")
}

#[test]
fn a_read_pane_shows_the_answer_it_was_given() {
    let lane = lane();
    let state = answered();

    let pane = ReadPane::over(&lane, &state).expect("state is from this generation");

    assert_eq!(
        pane.rows(),
        [
            Row("SPICE-1kKRRsDP".to_owned()),
            Row("SPICE-1kKRRsDQ".to_owned())
        ]
        .as_slice()
    );
    assert_eq!(pane.horizon(), Some(seq(48213)));
    assert_eq!(
        pane.overflow(),
        Some(Overflow {
            omitted: 41,
            continuation: seq(48250)
        })
    );
    assert!(pane.is_overflowing());
}

#[test]
fn a_read_pane_does_not_open_over_another_generations_state() {
    let operator = operator();
    let session = next_session();
    let state = BrowserState::<Row>::new(&accepted(&session, "operator", NEXT_EPOCH));

    assert!(ReadPane::over(&operator, &state).is_none());
}

#[test]
fn operator_only_packed_state_never_opens_for_a_same_epoch_lane() {
    let session = operator_session();
    let acceptance = accepted(&session, "operator", EPOCH);
    let operator = Principal::admitted(&acceptance);
    let lane = lane();
    let mut state = BrowserState::new(&acceptance);
    assert!(state.replace(&packed_answer(
        &session,
        &[OPERATOR_CAPABILITY, RAW_TAPE_SPAN, CONSENT_CREDENTIAL],
        None,
        48213,
    )));

    assert_eq!(
        operator.epoch(),
        lane.epoch(),
        "the fixture shares an epoch"
    );
    assert_eq!(
        ReadPane::over(&operator, &state)
            .expect("operator owns this answer")
            .rows()
            .len(),
        3
    );
    assert!(ReadPane::over(&lane, &state).is_none());

    let lane_answer = packed_answer(&lane_session(), &["lane-visible"], None, 48214);
    assert!(!state.replace(&lane_answer));
    let operator_read = ReadPane::over(&operator, &state).expect("operator answer remains whole");
    assert_eq!(operator_read.rows()[0].0, OPERATOR_CAPABILITY);
    assert_eq!(operator_read.rows()[1].0, RAW_TAPE_SPAN);
    assert_eq!(operator_read.rows()[2].0, CONSENT_CREDENTIAL);
}

#[test]
fn no_rendered_row_promotes_a_lane_principal() {
    let session = lane_session();
    let lane = lane();
    let mut state = BrowserState::new(&accepted(&session, &format!("lane:{LANE}"), EPOCH));
    // Unprivileged text may mention authority without carrying it.
    assert!(state.replace(&packed_answer(
        &session,
        &[
            "operator",
            "actor: operator",
            "the word capability is ordinary rendered text",
        ],
        None,
        48213,
    )));

    let read = ReadPane::over(&lane, &state).expect("state is from this generation");
    assert_eq!(read.rows().len(), 3, "the rows are rendered, not hidden");

    // Reading them changes nothing about what this principal may be offered.
    let principal = read.principal();
    assert!(!principal.is_human());
    assert!(GrantPane::over(principal).is_none());

    let steer = SteerPane::over(principal);
    for kind in GrantPane::decisions() {
        assert!(
            !steer.offers(kind),
            "{kind} was offered to a lane that had rendered operator text"
        );
    }
}

#[test]
fn an_operator_steer_pane_offers_every_command() {
    let operator = operator();
    let steer = SteerPane::over(&operator);

    let withheld = CommandKind::ALL
        .iter()
        .copied()
        .filter(|kind| !steer.offers(*kind))
        .collect::<Vec<_>>();
    assert_eq!(withheld, []);
}

#[test]
fn the_grant_pane_presents_exactly_the_commands_a_lane_is_denied() {
    let lane = lane();
    let steer = SteerPane::over(&lane);

    let presented = GrantPane::decisions().collect::<BTreeSet<_>>();
    let withheld = CommandKind::ALL
        .iter()
        .copied()
        .filter(|kind| !steer.offers(*kind))
        .collect::<BTreeSet<_>>();

    assert_eq!(presented, withheld);
}

#[test]
fn a_lane_builds_the_commands_it_is_offered() {
    let lane = lane();

    let command = noted(&lane);

    assert_eq!(command.kind, CommandKind::TaskNote);
    assert_eq!(command.id, uuid(COMMAND));
    assert_eq!(command.expected, existence());
    assert!(command.witnessed().is_ok());
}

#[test]
fn a_command_that_does_not_satisfy_its_law_is_declined_before_it_is_built() {
    let lane = lane();

    let error = SteerPane::over(&lane)
        .command(uuid(COMMAND), CommandKind::TaskNote, trust(), note())
        .expect_err("task::note does not witness trust");

    assert!(matches!(error, Declined::Law(_)), "{error:?}");
}

#[test]
fn an_answer_to_another_command_faults_rather_than_being_shown() {
    let lane = lane();
    let command = noted(&lane);
    let elsewhere = response(
        OTHER_COMMAND,
        Outcome::Refusal(Refusal::bare(wire::RefusalDiagnostic::new(
            "claim::held".to_owned(),
            None,
            None,
            None,
        ))),
    );

    let fault = settled(&command, &elsewhere).expect_err("an answer to another command faults");

    assert_eq!(fault.class, FaultClass::Protocol);
    assert_eq!(fault.reason, "pane::answer-mismatch");
    assert!(fault.detail.contains(OTHER_COMMAND), "{}", fault.detail);
    assert!(fault.detail.contains(COMMAND), "{}", fault.detail);
}

#[test]
fn no_lane_surface_renders_the_capability_its_session_presented() {
    let session = lane_session();
    let acceptance = accepted(&session, &format!("lane:{LANE}"), EPOCH);
    let lane = Principal::admitted(&acceptance);
    let answer = packed_answer(
        &session,
        &["SPICE-1kKRRsDP", "SPICE-1kKRRsDQ"],
        Some((41, 48250)),
        48213,
    );
    let mut state = BrowserState::new(&acceptance);
    assert!(state.replace(&answer));
    let read = ReadPane::over(&lane, &state).expect("state is from this generation");
    let steer = SteerPane::over(&lane);
    let command = noted(&lane);

    let rendered = format!(
        "{session:?} {acceptance:?} {answer:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}",
        read.principal(),
        read.rows(),
        read.overflow(),
        read.horizon(),
        read.cursor(),
        steer.principal(),
        command,
    );

    assert!(
        !rendered.contains(LANE_CAPABILITY),
        "a lane-scoped surface rendered the session capability: {rendered}"
    );
    assert!(
        rendered.contains(LANE),
        "the surface should still name the lane it belongs to"
    );
}

#[test]
fn transcript_disposition_admits_a_conditional_directive_witness() {
    let dispose = CommandKind::SteerDispose.law();
    let cancel = CommandKind::SteerCancel.law();

    // A scanner miss can name durable transcript evidence without a live
    // directive predicate. Operator cancellation cannot.
    assert!(dispose.required.is_empty());
    assert_eq!(dispose.optional, &[WitnessKind::Directive]);
    assert_eq!(dispose.authority, Authority::Connected);
    assert_eq!(cancel.required, &[WitnessKind::Directive]);
    assert!(cancel.optional.is_empty());
    assert_eq!(cancel.authority, Authority::Operator);
}

#[test]
fn a_lane_is_refused_the_consent_decision_an_operator_is_offered() {
    let lane = lane();
    let operator = operator();

    // The consent law names predicates and no capability; the registry row
    // states the capability, and this is the behaviour that statement buys.
    assert!(!SteerPane::over(&lane).offers(CommandKind::ConsentDecide));
    assert!(SteerPane::over(&operator).offers(CommandKind::ConsentDecide));
    assert!(GrantPane::over(&lane).is_none());
    assert!(GrantPane::over(&operator).is_some());

    let declined = SteerPane::over(&lane)
        .command(
            uuid(COMMAND),
            CommandKind::ConsentDecide,
            consent_witnesses(),
            note(),
        )
        .expect_err("a lane holds no consent decision");

    // The decline names the command and the principal, so the refusal is
    // legible without re-deriving either from the call site.
    let Declined::Authority { kind, actor } = declined else {
        panic!("a lane is declined for authority rather than for witnesses: {declined}");
    };
    assert_eq!(kind, CommandKind::ConsentDecide);
    assert_eq!(actor.to_string(), format!("lane:{LANE}"));

    // The refusal is about the capability rather than the witnesses: the same
    // witnesses satisfy the same row when the operator submits them.
    SteerPane::over(&operator)
        .command(
            uuid(COMMAND),
            CommandKind::ConsentDecide,
            consent_witnesses(),
            note(),
        )
        .expect("an operator decides consent");
}

/// One fold-backed reply admitted through the same response door.
///
/// The fold cursor and the horizon are separate parameters because they are
/// separate claims. A fixture that derived one from the other could not tell a
/// pane reading the wrong field from a pane reading the right one.
fn derived_answer(
    session: &Session,
    fold: &str,
    generation: u64,
    cursor: u64,
    rows: &[&str],
    overflow: Option<(u64, u64)>,
    horizon: u64,
) -> Admitted<Response<Derived<Row>>> {
    let body = Derived {
        fold: fold.parse().expect("fixture fold name"),
        r#gen: generation,
        cursor: seq(cursor),
        packed: Packed {
            rows: rows.iter().map(|row| Row((*row).to_owned())).collect(),
            overflow: overflow.map(|(omitted, continuation)| Overflow {
                omitted,
                continuation: seq(continuation),
            }),
            horizon: seq(horizon),
        },
    };
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid(COMMAND),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(horizon)),
            body,
        }),
    );
    session
        .admit_frame(&bytes)
        .expect("the derived fixture answer is admitted")
}

/// State holding one fold-backed answer from this generation.
///
/// The three positions are deliberately three different numbers: the fold had
/// applied through 48200, the sources are complete through 48213, and no frame
/// has moved the stream cursor at all.
fn fold_backed() -> BrowserState<Row> {
    let session = operator_session();
    let mut state = BrowserState::new(&accepted(&session, "operator", EPOCH));
    assert!(state.replace(&derived_answer(
        &session,
        "task::backlog",
        7,
        48200,
        &["SPICE-1kKRRsDP", "SPICE-1kKRRsDQ"],
        Some((41, 48250)),
        48213,
    )));
    state
}

/// State holding the same rows read straight off the authoritative heads.
///
/// Scoped to the same principal and carrying the same rows, overflow, and
/// horizon as [`fold_backed`], so the only difference between the two is the
/// wire shape the reply arrived in.
fn head_direct() -> BrowserState<Row> {
    let session = operator_session();
    let mut state = BrowserState::new(&accepted(&session, "operator", EPOCH));
    assert!(state.replace(&packed_answer(
        &session,
        &["SPICE-1kKRRsDP", "SPICE-1kKRRsDQ"],
        Some((41, 48250)),
        48213,
    )));
    state
}

#[test]
fn a_derived_answer_renders_its_fold_generation_and_cursor() {
    let operator = operator();
    let state = fold_backed();
    let read = ReadPane::over(&operator, &state).expect("state is from this generation");

    assert_eq!(
        read.fold().map(FoldName::as_str),
        Some("task::backlog"),
        "the pane must name the fold that answered"
    );
    assert_eq!(read.generation(), Some(7));
    assert_eq!(read.generation_cursor(), Some(seq(48200)));
}

#[test]
fn a_head_direct_answer_fabricates_no_provenance() {
    let operator = operator();
    let state = head_direct();
    let read = ReadPane::over(&operator, &state).expect("state is from this generation");

    // Same principal, same rows, same overflow, same horizon as the fold-backed
    // fixture, so the wire shape is the only thing that differs and the missing
    // provenance cannot be explained by anything else.
    let fold_backed = fold_backed();
    let derived = ReadPane::over(&operator, &fold_backed).expect("state is from this generation");
    assert_eq!(read.rows(), derived.rows());
    assert_eq!(read.overflow(), derived.overflow());
    assert_eq!(read.horizon(), derived.horizon());

    // The rows are there; only the provenance is absent, and it is absent
    // because a head-direct read has nowhere to carry one rather than because
    // the pane chose to hide it.
    assert_eq!(read.fold(), None);
    assert_eq!(read.generation(), None);
    assert_eq!(read.generation_cursor(), None);
}

#[test]
fn replacing_a_derived_answer_with_a_head_direct_one_drops_its_provenance() {
    let operator = operator();
    let mut state = fold_backed();
    assert!(state.replace(&packed_answer(
        &operator_session(),
        &["SPICE-1kKRRsDR"],
        None,
        48214
    )));

    let read = ReadPane::over(&operator, &state).expect("state is from this generation");

    // The fold that answered a moment ago did not answer this one. Retaining
    // its name would attribute the current rows to a generation that never
    // produced them.
    assert_eq!(read.rows(), [Row("SPICE-1kKRRsDR".to_owned())].as_slice());
    assert_eq!(read.fold(), None);
    assert_eq!(read.generation(), None);
    assert_eq!(read.generation_cursor(), None);
}

#[test]
fn a_refused_answer_leaves_the_provenance_the_pane_already_had() {
    let operator = operator();
    let mut state = fold_backed();

    // A refusal is an answer about an event that did not occur. Letting it
    // clear the fold would report "no generation answered" when one had.
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid(COMMAND),
        uuid(EPOCH),
        Outcome::<Derived<Row>>::Refusal(Refusal::bare(wire::RefusalDiagnostic::new(
            "fold::rebuilding".to_owned(),
            None,
            None,
            None,
        ))),
    );
    let refusal: Admitted<Response<Derived<Row>>> = operator_session()
        .admit_frame(&bytes)
        .expect("the fixture refusal is admitted");
    assert!(!state.replace(&refusal));

    let read = ReadPane::over(&operator, &state).expect("state is from this generation");
    assert_eq!(read.fold().map(FoldName::as_str), Some("task::backlog"));
    assert_eq!(read.generation(), Some(7));
    assert_eq!(read.rows().len(), 2);
}
