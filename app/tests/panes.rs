//! What each capability is shown, and what it is offered.
//!
//! Every principal here is decoded from an acceptance document rather than
//! constructed, because that is the only way the app can obtain one: the
//! resident derives actor and rights when it accepts the connection, and a
//! test that built a `Principal` directly would be proving the app agrees with
//! itself about authority it is not allowed to assert.
//!
//! The classification `app` compiles is a restatement of prose carried by
//! `wire`'s law rows, so one test here reads those rows instead of the table.
//! An oracle derived from the artifact under test can only notice that the
//! table is missing an entry, never that the table and the rows disagree.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use app::{
    Admitted, BrowserState, Declined, GrantPane, Principal, ReadPane, Session, SteerPane, settled,
};
use serde::{Deserialize, Serialize};
use wire::{
    Accepted, CanonicalUuid, Command, CommandKind, Digest, FaultClass, Outcome, Overflow, Packed,
    Response, Seq, TaskHandle, TrustState, Witness,
};

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

/// One reply document answering `id`, as bytes off the socket.
fn response_bytes(id: &str, outcome: &str) -> Vec<u8> {
    format!(
        r#"{{
  "v": 1,
  "id": "{id}",
  "epoch": "{EPOCH}",
  "outcome": {outcome}
}}"#
    )
    .into_bytes()
}

/// One admitted answer to `id`, carrying `outcome` verbatim.
fn response(id: &str, outcome: &str) -> Admitted<Response<Note>> {
    lane_session()
        .admit_frame::<Note>(&response_bytes(id, outcome))
        .expect("the fixture answer is admitted")
}

/// One packed reply admitted through the session before browser state sees it.
fn packed_answer(
    session: &Session,
    rows: &[&str],
    overflow: Option<(u64, u64)>,
    horizon: u64,
) -> Admitted<Response<Packed<Row>>> {
    let rows = rows
        .iter()
        .map(|row| format!(r#""{row}""#))
        .collect::<Vec<_>>()
        .join(", ");
    let overflow = overflow.map_or_else(String::new, |(omitted, continuation)| {
        format!(r#""overflow": {{ "omitted": {omitted}, "continuation": {continuation} }},"#)
    });
    let bytes = format!(
        r#"{{
  "v": 1,
  "id": "{COMMAND}",
  "epoch": "{EPOCH}",
  "outcome": {{
    "kind": "reply",
    "cursor": {horizon},
    "body": {{ "rows": [{rows}], {overflow} "horizon": {horizon} }}
  }}
}}"#
    );
    session
        .admit_frame(&bytes.into_bytes())
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
    // The capability crosses once, inside the opening. An acceptance that
    // echoed it would place a live credential on the return path of every
    // accepted connection, including every lane-scoped one.
    let bytes = format!(
        r#"{{ "wire": 1, "actor": "lane:{LANE}", "epoch": "{EPOCH}", "capability": "{LANE_CAPABILITY}" }}"#
    )
    .into_bytes();

    let fault = lane_session()
        .admit_accepted(&bytes)
        .expect_err("an acceptance carrying a capability is not a frame this build reads");

    assert_eq!(fault.reason, "frame::undecodable");
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
        r#"{ "kind": "reply", "seq": { "first": 48213, "last": 48214 },
             "body": { "task": "SPICE-aBcD", "note": "observed in review" } }"#,
    );
    let refusal = response(
        COMMAND,
        r#"{ "kind": "refusal", "reason": "task::absent",
             "remedy": "name a task that exists" }"#,
    );
    let fault = response(
        COMMAND,
        r#"{ "kind": "fault", "class": "store", "reason": "journal::unreadable",
             "detail": "authority database is not readable" }"#,
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
        r#"{ "kind": "refusal", "reason": "claim::held",
             "remedy": "wait for the lease to lapse" }"#,
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
fn panes_restate_the_wire_law_rows() {
    let named = operator_commands_named_by_wire();
    let classified = GrantPane::decisions().collect::<BTreeSet<_>>();

    // A scan that matched nothing would agree with an empty table and prove
    // nothing about either.
    assert!(
        !named.is_empty(),
        "no wire law row was found to state an operator-capability requirement; \
         the scan in this test has stopped reading what it claims to read"
    );
    assert_eq!(
        named, classified,
        "app's operator classification and wire's law rows disagree; one of them \
         moved. wire states the requirement in prose on the law row, and \
         `requires_operator` in app/src/pane.rs restates it -- reconcile them \
         rather than editing this test"
    );
}

/// The commands `wire`'s own law rows say need the operator capability.
///
/// A law row carries that requirement in prose rather than in its witness
/// list, because the capability is carried by the connection and not proved by
/// a predicate. Each such sentence names its commands by the verb that ends
/// their wire spelling, so `steer::cancel` appears as "cancel".
///
/// This reads prose and is deliberately literal about it. A reworded row is
/// exactly the moment to re-check that the classification app compiles still
/// says what the row says.
fn operator_commands_named_by_wire() -> BTreeSet<CommandKind> {
    let source = fs::read_to_string(workspace_root().join("wire/src/command.rs"))
        .expect("the wire command registry must be readable");
    let prose = source
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("///"))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ");

    let mut named = BTreeSet::new();
    for sentence in prose
        .split('.')
        .filter(|sentence| sentence.to_lowercase().contains("operator capability"))
    {
        let words = sentence
            .split(|character: char| !character.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect::<BTreeSet<_>>();
        named.extend(CommandKind::ALL.iter().copied().filter(|kind| {
            kind.as_str()
                .rsplit("::")
                .next()
                .is_some_and(|verb| words.contains(verb))
        }));
    }
    named
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("app must be a direct workspace member")
        .to_path_buf()
}
