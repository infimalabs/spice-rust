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
    Accepted, Answer, Authority, CanonicalUuid, Command, CommandKind, Derived, Digest, Fault,
    FaultClass, FoldName, Name, Outcome, Overflow, Packed, Refusal, Reply, ReplyPosition, Response,
    Seq, SeqRange, TaskHandle, TrustState, WireVersion, Witness, WitnessKind,
};

mod support;

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
    format!(r#"{{ "wire": 1, "actor": "{actor}", "epoch": "{epoch}" }}"#).into_bytes()
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
        &["SPICE-aBcD", "SPICE-eFgH"],
        Some((41, 48250)),
        48213,
    )));
    state
}

/// The one witness `task::note` requires.
fn existence() -> Vec<Witness> {
    vec![Witness::Existence {
        task: "SPICE-aBcD".parse::<TaskHandle>().expect("fixture handle"),
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
        task: "SPICE-aBcD".to_owned(),
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
fn the_resident_names_the_principal_a_session_acts_as() {
    assert!(operator().is_operator());
    assert!(!lane().is_operator());
    assert_eq!(operator().epoch(), uuid(EPOCH));
    assert_eq!(lane().actor().to_string(), format!("lane:{LANE}"));
}

#[test]
fn an_acceptance_for_another_generation_never_names_a_principal() {
    let fault = operator_session()
        .admit_accepted(&accepted_bytes("operator", NEXT_EPOCH))
        .expect_err("an acceptance from a replaced generation faults");

    assert_eq!(fault.class, FaultClass::Protocol);
    assert_eq!(fault.reason, "frame::epoch-mismatch");
    assert!(fault.detail.contains(NEXT_EPOCH), "{}", fault.detail);
}

#[test]
fn an_acceptance_at_another_wire_version_never_names_a_principal() {
    let bytes =
        format!(r#"{{ "wire": 99, "actor": "operator", "epoch": "{EPOCH}" }}"#).into_bytes();

    let fault = operator_session()
        .admit_accepted(&bytes)
        .expect_err("a foreign version faults");

    assert_eq!(fault.reason, "frame::wire-mismatch");
}

#[test]
fn an_acceptance_that_is_not_a_frame_names_no_principal() {
    let fault = operator_session()
        .admit_accepted(br#"{"wire": 1, "actor":"#)
        .expect_err("truncated bytes fault");

    assert_eq!(fault.reason, "frame::undecodable");
}

#[test]
fn an_acceptance_carrying_a_capability_does_not_decode() {
    // The capability crosses once, inside the opening. The one current
    // acceptance shape omits it; echoing it would place a live credential on
    // every return path, including every lane-scoped one.
    let bytes = format!(
        concat!(
            r#"{{ "wire": 1, "actor": "lane:{LANE}", "epoch": "{EPOCH}", "#,
            r#""capability": "{LANE_CAPABILITY}" }}"#,
        ),
        LANE = LANE,
        EPOCH = EPOCH,
        LANE_CAPABILITY = LANE_CAPABILITY
    )
    .into_bytes();

    let fault = lane_session()
        .admit_accepted(&bytes)
        .expect_err("an acceptance carrying a capability is not a frame this build reads");

    assert_eq!(fault.reason, "frame::undecodable");
}

#[test]
fn principal_debug_renders_the_epoch_but_not_the_capability() {
    let rendered = format!("{:?}", lane());

    assert!(
        rendered.contains(EPOCH),
        "principal debug omitted its generation: {rendered}"
    );
    assert!(
        !rendered.contains(LANE_CAPABILITY),
        "principal debug rendered its session capability: {rendered}"
    );
}

#[test]
fn a_read_pane_shows_the_answer_it_was_given() {
    let lane = lane();
    let state = answered();

    let pane = ReadPane::over(&lane, &state).expect("state is from this generation");

    assert_eq!(
        pane.rows(),
        [Row("SPICE-aBcD".to_owned()), Row("SPICE-eFgH".to_owned())].as_slice()
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
fn a_read_pane_invents_no_rows_horizon_or_overflow_before_an_answer() {
    let session = operator_session();
    let operator = operator();
    let state = BrowserState::<Row>::new(&accepted(&session, "operator", EPOCH));

    let pane = ReadPane::over(&operator, &state).expect("state is from this generation");

    assert!(pane.rows().is_empty());
    assert_eq!(pane.horizon(), None);
    assert_eq!(pane.overflow(), None);
    assert_eq!(pane.cursor(), None);
    assert!(!pane.is_overflowing());
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
fn a_lane_principal_opens_no_grant_pane() {
    assert!(GrantPane::over(&lane()).is_none());
    assert!(GrantPane::over(&operator()).is_some());
}

#[test]
fn a_consent_decision_is_an_operator_grant_control() {
    let lane = lane();
    let error = SteerPane::over(&lane)
        .command(
            uuid(COMMAND),
            CommandKind::ConsentDecide,
            Vec::new(),
            note(),
        )
        .expect_err("a lane cannot decide operator consent");

    assert_eq!(
        error,
        Declined::Authority {
            kind: CommandKind::ConsentDecide,
            actor: lane.actor().clone(),
        }
    );
    assert!(GrantPane::decisions().any(|kind| kind == CommandKind::ConsentDecide));
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
    assert!(!principal.is_operator());
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
fn a_lane_steer_pane_declines_every_command_the_operator_capability_carries() {
    let lane = lane();
    let steer = SteerPane::over(&lane);

    let mut declined = 0_usize;
    for kind in GrantPane::decisions() {
        let error = steer
            .command(uuid(COMMAND), kind, trust(), note())
            .expect_err("a lane is offered no operator-capability command");

        assert_eq!(
            error,
            Declined::Authority {
                kind,
                actor: lane.actor().clone(),
            }
        );
        assert!(error.to_string().contains(&format!("lane:{LANE}")));
        declined += 1;
    }

    // A loop that found nothing to decline would pass while proving nothing.
    assert!(declined >= 1, "the operator decision set is empty");
}

#[test]
fn an_operator_steer_pane_offers_every_command_in_the_registry() {
    let operator = operator();
    let steer = SteerPane::over(&operator);

    for kind in CommandKind::ALL {
        assert!(steer.offers(*kind), "{kind} was withheld from the operator");
    }
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
fn the_operator_builds_a_decision_through_the_pane_that_submits_it() {
    let operator = operator();
    let grant = GrantPane::over(&operator).expect("the operator opens a grant pane");

    let command = grant
        .steer()
        .command(uuid(COMMAND), CommandKind::TrustGrant, trust(), note())
        .expect("the operator is offered trust::grant");

    assert_eq!(command.kind, CommandKind::TrustGrant);
    assert!(command.witnessed().is_ok());
    assert!(grant.principal().is_operator());
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
fn a_reply_a_refusal_and_a_fault_reach_the_pane_as_three_distinct_outcomes() {
    let lane = lane();
    let command = noted(&lane);

    let reply = response(
        COMMAND,
        Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(48213),
                last: seq(48214),
            }),
            body: note(),
        }),
    );
    let refusal = response(
        COMMAND,
        Outcome::Refusal(Refusal {
            reason: "task::absent".to_owned(),
            witness: None,
            expected: None,
            observed: None,
            remedy: "name a task that exists".to_owned(),
        }),
    );
    let fault = response(
        COMMAND,
        Outcome::Fault(Fault {
            class: FaultClass::Store,
            reason: "journal::unreadable".to_owned(),
            detail: "authority database is not readable".to_owned(),
        }),
    );

    let outcomes = [&reply, &refusal, &fault].map(|admitted| {
        settled(&command, admitted).expect("each answer names the command it answers")
    });

    assert!(
        matches!(outcomes[0], Outcome::Reply(_)),
        "{:?}",
        outcomes[0]
    );
    assert!(
        matches!(outcomes[1], Outcome::Refusal(_)),
        "{:?}",
        outcomes[1]
    );
    assert!(
        matches!(outcomes[2], Outcome::Fault(_)),
        "{:?}",
        outcomes[2]
    );

    // Three variants, three values: no pair collapses into the other.
    assert_ne!(outcomes[0], outcomes[1]);
    assert_ne!(outcomes[1], outcomes[2]);
    assert_ne!(outcomes[0], outcomes[2]);
}

#[test]
fn an_answer_to_another_command_faults_rather_than_being_shown() {
    let lane = lane();
    let command = noted(&lane);
    let elsewhere = response(
        OTHER_COMMAND,
        Outcome::Refusal(Refusal {
            reason: "claim::held".to_owned(),
            witness: None,
            expected: None,
            observed: None,
            remedy: "wait for the lease to lapse".to_owned(),
        }),
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
        &["SPICE-aBcD", "SPICE-eFgH"],
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
fn the_grant_pane_presents_exactly_the_commands_wire_gives_operator_authority() {
    let classified = CommandKind::ALL
        .iter()
        .copied()
        .filter(|kind| kind.law().authority == Authority::Operator)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        CommandKind::SteerSend.law().authority,
        Authority::Operator,
        "a lane principal must never submit operator direction",
    );

    // The expected set is written out rather than derived, because a test that
    // computed it the way the pane does would agree with any classification,
    // including an empty one.
    assert_eq!(
        classified,
        BTreeSet::from([
            CommandKind::BoardHandout,
            CommandKind::SteerSend,
            CommandKind::SteerResend,
            CommandKind::SteerRecover,
            CommandKind::SteerRepair,
            CommandKind::SteerCancel,
            CommandKind::ConsentRead,
            CommandKind::ConsentPolicy,
            CommandKind::ConsentDecide,
            CommandKind::TrustGrant,
            CommandKind::TrustRevoke,
            CommandKind::LaneRead,
            CommandKind::LaneSeat,
            CommandKind::LaneArrange,
            CommandKind::LaneState,
            CommandKind::LineageConfigure,
            CommandKind::LineageArrange,
            CommandKind::ReleasePublish,
            CommandKind::UpChange,
        ])
    );
    assert_eq!(GrantPane::decisions().collect::<BTreeSet<_>>(), classified);
}

#[test]
fn every_command_states_an_authority_and_most_need_no_operator() {
    let mut operator = 0;
    let mut connected = 0;

    // The match is exhaustive over `Authority`, so a third capability cannot
    // enter the vocabulary without this count being confronted with it. Summing
    // the two arms back to `ALL.len()` would prove nothing instead: every kind
    // lands in exactly one arm, so that total holds under any classification.
    for kind in CommandKind::ALL {
        match kind.law().authority {
            Authority::Operator => operator += 1,
            Authority::Connected => connected += 1,
        }
    }

    assert_eq!(operator, 19);
    assert_eq!(connected, 32);
    assert_eq!(
        CommandKind::TaskOopsAdd.law().authority,
        Authority::Connected,
        "the exact oops route remains available to authenticated operators and lanes",
    );
    assert_eq!(
        CommandKind::EffortRead.law().authority,
        Authority::Connected,
        "reading what a task cost is an authenticated read, not an operator move",
    );
    assert_eq!(
        CommandKind::ALL.len(),
        51,
        "a command joined or left the registry; the counts above are the claim \
         to re-make, not the number to bump"
    );
}

#[test]
fn commands_sharing_witnesses_do_not_share_an_authority() {
    // These pairs witness identically and answer to different capabilities,
    // which is why each holds its own registry row. A single row per move
    // would have to state one authority for both and would be wrong for one.
    for (unrestricted, restricted) in [
        (CommandKind::TrustPend, CommandKind::TrustGrant),
        (CommandKind::TrustPend, CommandKind::TrustRevoke),
    ] {
        assert_eq!(
            unrestricted.law().required,
            restricted.law().required,
            "{} and {} were split for authority, not for witnesses",
            unrestricted.as_str(),
            restricted.as_str()
        );
        assert_eq!(unrestricted.law().authority, Authority::Connected);
        assert_eq!(restricted.law().authority, Authority::Operator);
    }
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
        &["SPICE-aBcD", "SPICE-eFgH"],
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
        &["SPICE-aBcD", "SPICE-eFgH"],
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
fn the_generation_cursor_is_neither_the_horizon_nor_the_stream_cursor() {
    let operator = operator();
    let state = fold_backed();
    let read = ReadPane::over(&operator, &state).expect("state is from this generation");

    // Three claims about three subjects: how far the fold had applied the
    // journal, how far the answer's sources are complete, and how far this
    // browser has applied what it received. Reading one for another would let a
    // stale generation render as current.
    assert_eq!(read.generation_cursor(), Some(seq(48200)));
    assert_eq!(read.horizon(), Some(seq(48213)));
    assert_eq!(read.cursor(), None, "no frame has been applied yet");
    assert_ne!(read.generation_cursor(), read.horizon());
}

#[test]
fn replacing_a_derived_answer_with_a_head_direct_one_drops_its_provenance() {
    let operator = operator();
    let mut state = fold_backed();
    assert!(state.replace(&packed_answer(
        &operator_session(),
        &["SPICE-iJkL"],
        None,
        48214
    )));

    let read = ReadPane::over(&operator, &state).expect("state is from this generation");

    // The fold that answered a moment ago did not answer this one. Retaining
    // its name would attribute the current rows to a generation that never
    // produced them.
    assert_eq!(read.rows(), [Row("SPICE-iJkL".to_owned())].as_slice());
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
        Outcome::<Derived<Row>>::Refusal(Refusal {
            reason: "fold::rebuilding".to_owned(),
            witness: None,
            expected: None,
            observed: None,
            remedy: "read again once the generation settles".to_owned(),
        }),
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

#[test]
fn truncation_is_read_from_the_answer_rather_than_counted_from_the_rows() {
    let operator = operator();

    // The truncated answer carries fewer rows than the complete one, so any
    // rule comparing row count against a page size gets these two backwards.
    let session = operator_session();
    let mut truncated = BrowserState::new(&accepted(&session, "operator", EPOCH));
    assert!(truncated.replace(&packed_answer(
        &session,
        &["SPICE-aBcD"],
        Some((41, 48250)),
        48213
    )));

    let mut complete = BrowserState::new(&accepted(&session, "operator", EPOCH));
    assert!(complete.replace(&packed_answer(
        &session,
        &["SPICE-aBcD", "SPICE-eFgH"],
        None,
        48213
    )));

    let short = ReadPane::over(&operator, &truncated).expect("state is from this generation");
    let long = ReadPane::over(&operator, &complete).expect("state is from this generation");

    assert!(short.rows().len() < long.rows().len());
    assert!(
        short.is_overflowing(),
        "the shorter answer is the truncated one"
    );
    assert!(
        !long.is_overflowing(),
        "the longer answer is the complete one"
    );
    assert_eq!(
        short.overflow(),
        Some(Overflow {
            omitted: 41,
            continuation: seq(48250)
        })
    );
}

#[test]
fn a_pane_reports_the_horizon_the_answer_declared_and_no_other() {
    let operator = operator();
    let state = fold_backed();
    let read = ReadPane::over(&operator, &state).expect("state is from this generation");

    // A pane with no answer yet reports absence rather than position zero.
    let empty = BrowserState::<Row>::new(&accepted(&operator_session(), "operator", EPOCH));
    let blank = ReadPane::over(&operator, &empty).expect("state is from this generation");

    assert_eq!(read.horizon(), Some(seq(48213)));
    assert_eq!(blank.horizon(), None);
    assert!(blank.rows().is_empty());
    assert!(!blank.is_overflowing());
}
