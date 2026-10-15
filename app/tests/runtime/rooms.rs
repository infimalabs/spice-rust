//! Room authority comes from each subscription's room snapshot and the live
//! facts after its horizon, never from browser-local foreground state.

use super::*;
use serde::Serialize;
use wire::principal::{PrincipalAdmitted, PrincipalProfile, PrincipalRole};
use wire::rooms::{
    CandidateRef, CandidateReference, CandidateRetained, CandidateWithdrawal, CandidateWithdrawn,
    CoverageRef, DecisionOutcome, DecisionRef, ExecutionAuthorization, ExecutionAuthorized,
    PreparationDecided, PreparationDecision, RetainedCandidate, RoomAuthorityStamp,
    RoomCoordinationStarted, RoomMessaged, RoomPutAside, RoomSource, RoomStopRequest, TextRange,
    WorkingGroup, WorkingGroupMoved, WorkingGroupPartition,
};

#[path = "rooms/contributions.rs"]
mod contributions;
#[path = "rooms/conversation.rs"]
mod conversation;

const ROOM_EVENT: &str = "dVsYdDj7d";
const COORDINATION_EVENT: &str = "dVsYdDj7k";
const GROUP: &str = "dVsYdDj7f";
const STOP_EVENT: &str = "dVsYdDj7g";
const CANDIDATE: &str = "dVsYdDj7h";
const SECOND_CANDIDATE: &str = "dVsYdDj7m";
const COORDINATION_KEY: &str = "dCoordKey1";
const STOP_KEY: &str = "dStopKey1";

fn authority(text: &str) -> RoomAuthorityStamp {
    text.parse().expect("fixture room authority")
}

fn envelope<T: Serialize>(position: u64, kind: &str, body: &T) -> serde_json::Value {
    let mut fact = serde_json::to_value(body)
        .expect("fact body serializes")
        .as_object()
        .expect("fact body is an object")
        .clone();
    fact.insert("kind".to_owned(), serde_json::json!(kind));
    serde_json::json!({
        "v": 1,
        "seq": position,
        "at": "2030-04-12T18:44:09.271Z",
        "actor": "operator",
        "origin": "thread:operator",
        "fact": fact,
    })
}

fn live_fact(fact: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "stream": "fact",
        "v": 1,
        "id": REQUEST,
        "epoch": EPOCH,
        "fact": fact,
    }))
    .expect("live fact frame serializes")
}

fn principal_admitted(position: u64, principal: &str) -> serde_json::Value {
    envelope(
        position,
        "principal::admitted",
        &PrincipalAdmitted {
            principal: uuid(principal),
            profile: PrincipalProfile::default(),
            roles: [PrincipalRole::Operator].into_iter().collect(),
            capability: None,
        },
    )
}

fn room() -> wire::TaskStamp {
    fixture_stamp("PUBLIC-work1")
}

/// The fixture group holding the fixture room and each `extra` design.
fn group(extra: &[&str]) -> WorkingGroup {
    WorkingGroup {
        group: authority(GROUP),
        anchors: std::iter::once(room())
            .chain(extra.iter().map(|design| fixture_stamp(design)))
            .collect(),
    }
}

/// The fixture group's last design moving in, leaving the partition at
/// revision `position` with that one group.
fn group_moved(position: u64, extra: &[&str]) -> serde_json::Value {
    let group = group(extra);
    envelope(
        position,
        "group::moved",
        &WorkingGroupMoved {
            design: group
                .anchors
                .last()
                .expect("fixture group holds a design")
                .clone(),
            from: None,
            to: Some(group.group.clone()),
            partition: WorkingGroupPartition {
                revision: position,
                groups: vec![group],
                fast: Vec::new(),
            },
        },
    )
}

/// The fixture room's design leaving its group at `position`, ending the
/// partition's only group.
fn group_ended(position: u64) -> serde_json::Value {
    envelope(
        position,
        "group::moved",
        &WorkingGroupMoved {
            design: room(),
            from: Some(authority(GROUP)),
            to: None,
            partition: WorkingGroupPartition {
                revision: position,
                groups: Vec::new(),
                fast: Vec::new(),
            },
        },
    )
}

fn room_message(position: u64) -> serde_json::Value {
    let body = RoomMessaged {
        room: room(),
        publication: authority(ROOM_EVENT),
        hunk: wire::rooms::TranscriptHunk {
            head: wire::TranscriptSourceHead {
                lane: uuid(LANE),
                vendor: "omp".parse().expect("fixture vendor"),
                stream: "session".to_owned(),
                source: uuid(REQUEST),
                next: wire::body::TranscriptPosition {
                    member: 1,
                    offset: 64,
                },
            },
            locus: wire::MessageLocus {
                member: 1,
                offset: 8,
                length: 32,
                event: 0,
            },
            text: TextRange { start: 0, end: 12 },
        },
        // An occupant is not a logical lane; the projection must not use this
        // identity as its participant key.
        author: uuid(SECOND_LANE),
        directed: Vec::new(),
    };
    envelope(position, "room::messaged", &body)
}

/// The operator direction a coordination record follows, one position earlier.
fn direction(position: u64, key: &str, lane: &str) -> serde_json::Value {
    let body = serde_json::json!({
        "directive": REQUEST,
        "fanout": REQUEST,
        "key": key,
        "lane": lane,
        "text": "Contribute the bounded review",
        "control": format!("room-coordinate:{}", room()),
        "expires": "2030-04-13T18:44:09.271Z",
    });
    envelope(position, "steer::sent", &body)
}

fn coordination(position: u64) -> serde_json::Value {
    let body = RoomCoordinationStarted {
        room: room(),
        publication: authority(COORDINATION_EVENT),
        key: COORDINATION_KEY.to_owned(),
        target: wire::OccupantAddress::new(seq(1)),
        rule: None,
        automated: None,
    };
    envelope(position, "room::directed", &body)
}

fn stop() -> RoomStopRequest {
    RoomStopRequest {
        key: STOP_KEY.to_owned(),
        target: wire::OccupantAddress::new(seq(1)),
        claim: wire::SequenceCode::new(3),
    }
}

fn put_aside(position: u64) -> serde_json::Value {
    let body = RoomPutAside {
        room: room(),
        publication: authority(STOP_EVENT),
        canceled: Vec::new(),
        stop: Some(stop()),
    };
    envelope(position, "room::shelved", &body)
}

fn authorization(decision: u64) -> ExecutionAuthorization {
    let candidate = CandidateRef {
        controller: room(),
        candidate: authority(CANDIDATE),
    };
    ExecutionAuthorization {
        decision: DecisionRef {
            controller: room(),
            decision: wire::SequenceCode::new(decision),
        },
        candidate: candidate.clone(),
        governed_inputs: vec![room()],
        selected_outputs: vec![fixture_stamp("HIDDEN-oops1")],
        coverage: vec![CoverageRef {
            candidate,
            coverage: wire::SequenceCode::new(1),
        }],
    }
}

fn authorized(position: u64, decision: u64) -> serde_json::Value {
    let body = ExecutionAuthorized {
        authorization: authorization(decision),
    };
    envelope(position, "proposal::authorized", &body)
}

fn retained(position: u64, candidate: &str, output: &str) -> serde_json::Value {
    envelope(
        position,
        "proposal::retained",
        &CandidateRetained {
            candidate: RetainedCandidate {
                candidate: CandidateRef {
                    controller: room(),
                    candidate: authority(candidate),
                },
                governed_inputs: vec![room()],
                outputs: vec![fixture_stamp(output)],
                references: vec![CandidateReference {
                    relation: "contract".parse().expect("fixture relation"),
                    source: RoomSource::Journal {
                        fact: wire::SequenceCode::new(1),
                    },
                }],
                evidence: Vec::new(),
            },
        },
    )
}

fn decided(
    position: u64,
    candidate: &str,
    decision: u64,
    outcome: impl FnOnce(CandidateRef) -> DecisionOutcome,
) -> serde_json::Value {
    let candidate = CandidateRef {
        controller: room(),
        candidate: authority(candidate),
    };
    envelope(
        position,
        "proposal::decided",
        &PreparationDecided {
            decision: PreparationDecision {
                decision: DecisionRef {
                    controller: room(),
                    decision: wire::SequenceCode::new(decision),
                },
                source: RoomSource::Journal {
                    fact: wire::SequenceCode::new(position - 1),
                },
                outcome: outcome(candidate),
            },
        },
    )
}

/// The fact page an operator subscription opens with: live, empty, at its horizon.
fn operator_facts(session: &Session, horizon: u64) -> app::Admitted<StreamFrame> {
    admitted_stream(session, &packed_frame("facts", &[], None, horizon))
}

fn room_page(
    session: &Session,
    rows: &[serde_json::Value],
    overflow: Option<(u64, u64)>,
    horizon: u64,
) -> app::Admitted<StreamFrame> {
    admitted_stream(session, &packed_frame("rooms", rows, overflow, horizon))
}

/// The fixture board with the room anchor under `project`, or absent,
/// optionally held by `LANE` under the claim won at `claimed`, and optionally
/// settled with `(status, review)`.
fn room_board(
    session: &Session,
    project: Option<&str>,
    claimed: Option<u64>,
    settlement: Option<(&str, &str)>,
) -> app::Admitted<Response<wire::intent::BoardAnswer>> {
    let original = board_response(session, "hidden");
    let Outcome::Reply(reply) = &original.get().outcome else {
        panic!("fixture board reply");
    };
    let mut body = reply.body.clone();
    let room = room();
    if let Some(project) = project {
        let task = body
            .tasks
            .iter_mut()
            .find(|task| task.statement.body.born == room)
            .expect("room anchor is on the fixture board");
        task.statement.body.project = project.to_owned();
        task.statement.body.handle = wire::TaskHandle::for_project(project, &room);
        task.claim = claimed.map(|claimed| {
            serde_json::from_value(serde_json::json!({
                "position": claimed,
                "body": {
                    "actor": format!("lane:{LANE}"),
                    "fact": claimed,
                    "claimed": claimed,
                    "cell": {"project": project, "phase": "todo"},
                    "lane": LANE,
                    "occupant": REQUEST,
                    "epoch": EPOCH,
                    "disposition": "active",
                },
            }))
            .expect("fixture claim head")
        });
        if let Some((status, review)) = settlement {
            task.workflow.body.status = status.to_owned();
            task.workflow.body.review = review.to_owned();
            task.workflow.body.completion =
                Some("2025-01-01T00:00:00.000Z".parse().expect("fixture instant"));
        }
    } else {
        body.tasks.retain(|task| task.statement.body.born != room);
    }
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(4)),
            body,
        }),
    );
    session
        .admit_frame(&bytes)
        .expect("room board response is admitted")
}

fn participants(runtime: &AppRuntime, now_micros: i64) -> Vec<app::RoomParticipant> {
    runtime
        .room_projection()
        .participants(&room(), runtime.board(), now_micros)
}

const MICROS_PER_SECOND: i64 = 1_000_000;

/// The moment `seconds` after the fixture room message was published.
fn after_message(seconds: i64) -> i64 {
    authority(ROOM_EVENT).micros() + seconds * MICROS_PER_SECOND
}

/// A moment past the activity window of every fixture publication.
fn aged() -> i64 {
    after_message(2 * wire::rooms::ROOM_ACTIVITY_SECONDS)
}

/// The frames an operator subscription opens with: its live-start fact page,
/// then a two-page room snapshot at the same horizon.
fn opening(session: &Session) -> [app::Admitted<StreamFrame>; 3] {
    [
        operator_facts(session, 8),
        room_page(
            session,
            &[group_moved(1, &[]), room_message(2)],
            Some((4, 4)),
            8,
        ),
        room_page(
            session,
            &[
                authorized(4, 1),
                direction(5, COORDINATION_KEY, SECOND_LANE),
                coordination(6),
                put_aside(7),
            ],
            None,
            8,
        ),
    ]
}

#[test]
fn operator_subscription_rebuilds_room_authority_from_its_paged_room_snapshot() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    assert!(runtime.subjects().contains(&ViewSubject::Rooms));
    let [facts, first, last] = opening(&session);

    runtime.apply_stream(&facts);
    assert_eq!(
        runtime.room_projection().status(),
        app::RoomProjectionStatus::Loading,
        "a live-start fact page carries no room authority"
    );
    assert_eq!(
        runtime.apply_stream(&first),
        vec![StreamDirective::Continue {
            subject: ViewSubject::Rooms,
            cursor: wire::StreamCursor {
                epoch: uuid(EPOCH),
                seq: seq(4),
            },
        }]
    );
    assert_eq!(
        runtime.room_projection().status(),
        app::RoomProjectionStatus::Loading
    );
    assert!(runtime.apply_stream(&last).is_empty());
    let projection = runtime.room_projection();
    assert_eq!(projection.status(), app::RoomProjectionStatus::Available);
    assert_eq!(projection.partition().groups, vec![group(&[])]);
    assert_eq!(
        projection.preparation().authorizations[&authorization(1).decision].value,
        authorization(1)
    );
    let positions = projection
        .room(&room())
        .expect("room")
        .events()
        .iter()
        .map(|(id, event)| (id.clone(), event.position))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        positions,
        [(ROOM_EVENT, 2), (COORDINATION_EVENT, 6), (STOP_EVENT, 7)]
            .map(|(id, position)| (authority(id), seq(position)))
            .into()
    );
    assert_eq!(
        participants(&runtime, aged()),
        vec![
            app::RoomParticipant {
                lane: uuid(LANE),
                reasons: vec![app::ParticipantReason::Contribution(authority(ROOM_EVENT))],
            },
            app::RoomParticipant {
                lane: uuid(SECOND_LANE),
                reasons: vec![app::ParticipantReason::Coordination(
                    COORDINATION_KEY.to_owned()
                )],
            },
        ],
        "the coordination lane comes from its direction, never a message author occupant"
    );
    assert!(
        runtime
            .room_projection()
            .current_stop(&room(), runtime.board())
            .is_none(),
        "no board snapshot proves the stopped claim active"
    );
}

#[test]
fn proposal_projection_keeps_every_version_and_each_versions_decision_history() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    runtime.apply_stream(&operator_facts(&session, 5));
    runtime.apply_stream(&room_page(
        &session,
        &[
            room_message(1),
            retained(2, CANDIDATE, "HIDDEN-oops1"),
            decided(3, CANDIDATE, 1, |candidate| DecisionOutcome::Returned {
                candidate,
                reason: "the evidence is incomplete".to_owned(),
            }),
            retained(4, SECOND_CANDIDATE, "PUBLIC-work2"),
            decided(5, SECOND_CANDIDATE, 2, |candidate| {
                DecisionOutcome::Collapse {
                    candidate,
                    reason: "the premise no longer holds".to_owned(),
                }
            }),
        ],
        None,
        5,
    ));

    let preparation = runtime.room_projection().preparation();
    assert_eq!(
        preparation
            .candidates
            .values()
            .map(|candidate| candidate.position)
            .collect::<Vec<_>>(),
        [seq(2), seq(4)]
    );
    assert_eq!(
        preparation
            .decisions
            .values()
            .map(|decision| (&decision.value.outcome, decision.position))
            .collect::<Vec<_>>(),
        [
            (
                &DecisionOutcome::Returned {
                    candidate: CandidateRef {
                        controller: room(),
                        candidate: authority(CANDIDATE),
                    },
                    reason: "the evidence is incomplete".to_owned(),
                },
                seq(3),
            ),
            (
                &DecisionOutcome::Collapse {
                    candidate: CandidateRef {
                        controller: room(),
                        candidate: authority(SECOND_CANDIDATE),
                    },
                    reason: "the premise no longer holds".to_owned(),
                },
                seq(5),
            ),
        ]
    );
}

#[test]
fn a_withdrawn_version_stays_in_history_with_its_author_reason_and_time() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    runtime.apply_stream(&operator_facts(&session, 4));
    let mistaken = CandidateRef {
        controller: room(),
        candidate: authority(CANDIDATE),
    };
    let mut withdrawn = envelope(
        4,
        "proposal::withdrawn",
        &CandidateWithdrawn {
            withdrawal: CandidateWithdrawal {
                candidate: mistaken.clone(),
                reason: "the next version corrects its sources".to_owned(),
            },
        },
    );
    withdrawn["actor"] = format!("lane:{LANE}").into();
    runtime.apply_stream(&room_page(
        &session,
        &[
            room_message(1),
            retained(2, CANDIDATE, "HIDDEN-oops1"),
            retained(3, SECOND_CANDIDATE, "PUBLIC-work2"),
            withdrawn,
        ],
        None,
        4,
    ));

    let preparation = runtime.room_projection().preparation();
    assert_eq!(
        preparation.candidates.len(),
        2,
        "withdrawal keeps the version"
    );
    assert_eq!(
        preparation.withdrawals.keys().collect::<Vec<_>>(),
        [&mistaken],
        "only the withdrawn version leaves operator choice"
    );
    let record = &preparation.withdrawals[&mistaken];
    assert_eq!(record.position, seq(4));
    assert_eq!(record.value.author, wire::Actor::Lane(uuid(LANE)));
    assert_eq!(
        record.value.withdrawal.reason,
        "the next version corrects its sources"
    );
    assert_eq!(
        serde_json::to_value(record.value.at).expect("instant encodes"),
        "2030-04-12T18:44:09.271Z"
    );
}

#[test]
fn historical_operator_withdrawal_keeps_the_first_principal_after_another_birth() {
    const FIRST: &str = "018f82d4-6f59-7d06-92e3-bce9131163f0";
    const SECOND: &str = "018f82d4-6f59-7d06-92e3-bce9131163a0";
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    runtime.apply_stream(&operator_facts(&session, 4));
    let candidate = CandidateRef {
        controller: room(),
        candidate: authority(CANDIDATE),
    };
    let withdrawn = envelope(
        4,
        "proposal::withdrawn",
        &CandidateWithdrawn {
            withdrawal: CandidateWithdrawal {
                candidate: candidate.clone(),
                reason: "superseded".to_owned(),
            },
        },
    );
    runtime.apply_stream(&room_page(
        &session,
        &[
            principal_admitted(1, FIRST),
            room_message(2),
            retained(3, CANDIDATE, "HIDDEN-oops1"),
            withdrawn,
        ],
        None,
        4,
    ));

    let author = runtime.room_projection().preparation().withdrawals[&candidate]
        .value
        .author
        .clone();
    assert_eq!(author, wire::Actor::Operator);
    let resolved = wire::Actor::Human(uuid(FIRST));
    assert_eq!(runtime.room_projection().author(&author), resolved);

    runtime.apply_stream(&admitted_stream(
        &session,
        &live_fact(principal_admitted(5, SECOND)),
    ));
    assert_eq!(
        runtime.room_projection().author(&author),
        resolved,
        "a later birth does not move historical authorship"
    );
}

#[test]
fn snapshot_stops_follow_the_board_and_later_live_facts_retire_authority() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    for frame in opening(&session) {
        runtime.apply_stream(&frame);
    }
    runtime.apply_board(&room_board(&session, Some("app.rooms"), Some(3), None));
    assert_eq!(
        runtime
            .room_projection()
            .current_stop(&room(), runtime.board())
            .map(|stop| &stop.value),
        Some(&stop())
    );
    assert_eq!(
        participants(&runtime, aged())[0].reasons,
        vec![
            app::ParticipantReason::Contribution(authority(ROOM_EVENT)),
            app::ParticipantReason::Work(room()),
            app::ParticipantReason::Stop(STOP_KEY.to_owned()),
        ]
    );

    let canceled = envelope(
        9,
        "steer::canceled",
        &wire::body::SteerCanceled {
            key: COORDINATION_KEY.to_owned(),
            lane: uuid(SECOND_LANE),
        },
    );
    runtime.apply_stream(&admitted_stream(&session, &live_fact(canceled)));
    runtime.apply_stream(&admitted_stream(&session, &live_fact(authorized(10, 2))));
    assert_eq!(
        participants(&runtime, aged())
            .iter()
            .map(|participant| participant.lane)
            .collect::<Vec<_>>(),
        vec![uuid(LANE)],
        "a retired coordination no longer places its lane in the room"
    );
    assert_eq!(
        runtime.room_projection().preparation().authorizations.len(),
        2
    );

    runtime.apply_board(&room_board(&session, Some("app.rooms"), Some(5), None));
    assert!(
        runtime
            .room_projection()
            .current_stop(&room(), runtime.board())
            .is_none(),
        "a later claim win leaves the stop in the timeline but not current"
    );
    assert_eq!(
        runtime
            .room_projection()
            .room(&room())
            .expect("room")
            .events()[&authority(STOP_EVENT)]
            .position,
        seq(7)
    );
}

#[test]
fn activity_ages_out_while_contribution_history_and_native_duties_stay() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    for frame in opening(&session) {
        runtime.apply_stream(&frame);
    }
    let window = wire::rooms::ROOM_ACTIVITY_SECONDS;
    assert_eq!(
        participants(&runtime, after_message(window))[0].reasons,
        vec![
            app::ParticipantReason::Contribution(authority(ROOM_EVENT)),
            app::ParticipantReason::Activity(authority(ROOM_EVENT)),
        ],
        "a contribution at the window's edge is still current activity"
    );
    let aged = participants(&runtime, after_message(window + 1));
    assert_eq!(
        aged[0].reasons,
        vec![app::ParticipantReason::Contribution(authority(ROOM_EVENT))],
        "aging removes only the activity reason, never the contribution"
    );
    assert_eq!(
        aged[1].reasons,
        vec![app::ParticipantReason::Coordination(
            COORDINATION_KEY.to_owned()
        )],
        "a live coordination duty does not age"
    );
}

/// A room publication identity `seconds` after the fixture room message.
fn later_publication(seconds: i64) -> RoomAuthorityStamp {
    RoomAuthorityStamp::for_micros(after_message(seconds)).expect("fixture publication")
}

#[test]
fn a_leave_ends_activity_until_the_lane_contributes_again() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    for frame in opening(&session) {
        runtime.apply_stream(&frame);
    }
    let left = later_publication(1);
    live(
        &mut runtime,
        &session,
        envelope(
            9,
            "room::left",
            &wire::rooms::RoomLeft {
                room: room(),
                publication: left.clone(),
                lane: uuid(LANE),
                clause: "my part is done".to_owned(),
            },
        ),
    );
    let current = participants(&runtime, after_message(2));
    assert_eq!(
        current[0].reasons,
        vec![
            app::ParticipantReason::Contribution(authority(ROOM_EVENT)),
            app::ParticipantReason::Left {
                publication: left,
                clause: "my part is done".to_owned(),
            },
        ],
        "a leave ends activity and stands with its clause"
    );
    assert_eq!(
        current[1].reasons,
        vec![app::ParticipantReason::Coordination(
            COORDINATION_KEY.to_owned()
        )],
        "another lane's native duty is untouched"
    );

    let returned = later_publication(3);
    let mut contribution = room_message(10);
    contribution["fact"]["publication"] = serde_json::json!(returned.to_string());
    live(&mut runtime, &session, contribution);
    assert_eq!(
        participants(&runtime, after_message(4))[0].reasons,
        vec![
            app::ParticipantReason::Contribution(authority(ROOM_EVENT)),
            app::ParticipantReason::Contribution(returned.clone()),
            app::ParticipantReason::Activity(returned),
        ],
        "a later contribution supersedes the leave"
    );
}

#[test]
fn only_a_leave_divides_a_working_group_plane() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    for frame in opening(&session) {
        runtime.apply_stream(&frame);
    }
    let left = later_publication(1);
    live(
        &mut runtime,
        &session,
        envelope(
            9,
            "room::left",
            &wire::rooms::RoomLeft {
                room: room(),
                publication: left.clone(),
                lane: uuid(LANE),
                clause: "my part is done".to_owned(),
            },
        ),
    );
    let returned = later_publication(3);
    let mut contribution = room_message(10);
    contribution["fact"]["publication"] = serde_json::json!(returned.to_string());
    live(&mut runtime, &session, contribution);

    let projection = runtime.room_projection();
    let events = projection.room(&room()).expect("room").events();
    let dividers = events
        .iter()
        .filter(|(_, event)| event.kind.divides_plane())
        .map(|(publication, _)| publication.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        dividers,
        vec![left],
        "the departure alone divides; the same lane's earlier and later publications stay cards"
    );
    assert!(
        events.contains_key(&returned) && events.contains_key(&authority(ROOM_EVENT)),
        "the ordinary publications on either side of the leave remain in the timeline"
    );
}

/// A coordination record at `position` under `key`, carrying a rule of
/// `limit` resumes and, for an automated resume, its `step`.
fn ruled(
    position: u64,
    publication: &str,
    key: &str,
    limit: u8,
    step: Option<u8>,
) -> serde_json::Value {
    let body = RoomCoordinationStarted {
        room: room(),
        publication: authority(publication),
        key: key.to_owned(),
        target: wire::OccupantAddress::new(seq(1)),
        rule: Some(wire::rooms::RoomResumeRule {
            limit,
            criterion: "Which candidate survives review".to_owned(),
            contribution: "Assess the open candidate".to_owned(),
            stop: "Stop once the candidate is assessed".to_owned(),
        }),
        automated: step.map(|step| wire::rooms::RoomAutomatedResume {
            rule: authority(COORDINATION_EVENT),
            step,
            evidence: authority(ROOM_EVENT),
        }),
    };
    envelope(position, "room::directed", &body)
}

#[test]
fn automation_standing_follows_the_latest_record_its_request_and_put_aside() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    for frame in opening(&session) {
        runtime.apply_stream(&frame);
    }
    let automation = |runtime: &AppRuntime| {
        runtime
            .room_projection()
            .coordination_standing(&room())
            .map(|standing| standing.automation)
    };
    assert_eq!(
        automation(&runtime),
        Some(app::ResumeAutomation::Off),
        "a record without a rule leaves automation off"
    );
    let live = |runtime: &mut AppRuntime, fact| {
        runtime.apply_stream(&admitted_stream(&session, &live_fact(fact)));
    };
    live(&mut runtime, direction(9, "dRuleKey1", SECOND_LANE));
    live(&mut runtime, ruled(10, "dVsYdDj7m", "dRuleKey1", 2, None));
    assert_eq!(
        automation(&runtime),
        Some(app::ResumeAutomation::Armed { limit: 2, spent: 0 })
    );
    live(&mut runtime, direction(11, "dRuleKey2", SECOND_LANE));
    live(
        &mut runtime,
        ruled(12, "dVsYdDj7n", "dRuleKey2", 2, Some(1)),
    );
    let canceled = wire::body::SteerCanceled {
        key: "dRuleKey2".to_owned(),
        lane: uuid(SECOND_LANE),
    };
    live(&mut runtime, envelope(13, "steer::canceled", &canceled));
    assert_eq!(
        automation(&runtime),
        Some(app::ResumeAutomation::Ended {
            limit: 2,
            spent: 1,
            disposition: wire::Disposition::Canceled,
        }),
        "a request retired without acknowledgment ends the rule"
    );
    live(&mut runtime, direction(14, "dRuleKey3", SECOND_LANE));
    live(
        &mut runtime,
        ruled(15, "dVsYdDj7p", "dRuleKey3", 2, Some(2)),
    );
    assert_eq!(
        automation(&runtime),
        Some(app::ResumeAutomation::Spent { limit: 2 })
    );
    live(&mut runtime, direction(16, "dRuleKey4", SECOND_LANE));
    live(&mut runtime, ruled(17, "dVsYdDj7q", "dRuleKey4", 3, None));
    live(&mut runtime, put_aside(18));
    assert_eq!(
        automation(&runtime),
        Some(app::ResumeAutomation::PutAside { limit: 3, spent: 0 }),
        "a put-aside after a ruled record ends it with budget left"
    );
}

#[test]
fn same_scope_reconnect_rebuilds_from_the_new_snapshot_with_stable_identities() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let mut other_browser = AppRuntime::new(&accepted);
    for browser in [&mut runtime, &mut other_browser] {
        browser.apply_stream(&operator_facts(&session, 2));
        browser.apply_stream(&room_page(
            &session,
            &[group_moved(1, &[]), room_message(2)],
            None,
            2,
        ));
    }
    let removal = admitted_stream(&session, &live_fact(group_ended(3)));
    runtime.apply_stream(&removal);
    other_browser.apply_stream(&removal);
    for browser in [&runtime, &other_browser] {
        assert!(browser.room_projection().partition().groups.is_empty());
        assert!(
            browser
                .room_projection()
                .room(&room())
                .expect("room")
                .events()
                .contains_key(&authority(ROOM_EVENT)),
            "shared catalog edits never replace, mount, reorder, or close room cards"
        );
    }

    // The socket drops; a renamed group and a coordination land unseen.
    assert!(!runtime.reconnect(&accepted));
    assert_eq!(
        runtime.room_projection().status(),
        app::RoomProjectionStatus::Loading
    );
    assert!(runtime.room_projection().room(&room()).is_none());
    runtime.apply_stream(&operator_facts(&session, 7));
    runtime.apply_stream(&room_page(
        &session,
        &[
            room_message(2),
            group_moved(4, &["PUBLIC-work2"]),
            direction(5, COORDINATION_KEY, SECOND_LANE),
            coordination(6),
        ],
        None,
        7,
    ));
    let projection = runtime.room_projection();
    assert_eq!(projection.status(), app::RoomProjectionStatus::Available);
    assert_eq!(
        projection.partition().groups,
        vec![group(&["PUBLIC-work2"])]
    );
    assert_eq!(projection.partition().revision, 4);
    let events = projection.room(&room()).expect("room").events();
    assert_eq!(events[&authority(ROOM_EVENT)].position, seq(2));
    assert!(events.contains_key(&authority(COORDINATION_EVENT)));

    assert_eq!(
        runtime
            .room_projection()
            .anchor_authority(&room(), runtime.board()),
        app::RoomAnchorAuthority::Unavailable
    );
    runtime.apply_board(&room_board(&session, None, None, None));
    assert_eq!(
        runtime
            .room_projection()
            .anchor_authority(&room(), runtime.board()),
        app::RoomAnchorAuthority::Missing
    );
    runtime.apply_board(&room_board(&session, Some("app.rooms"), None, None));
    assert_eq!(
        runtime
            .room_projection()
            .anchor_authority(&room(), runtime.board()),
        app::RoomAnchorAuthority::Available(wire::TaskHandle::for_project("app.rooms", &room()))
    );
    for (status, review, standing) in [
        ("completed", "clean", app::RoomAnchorSettlement::Completed),
        ("completed", "closed", app::RoomAnchorSettlement::Closed),
        ("deleted", "closed", app::RoomAnchorSettlement::Deleted),
    ] {
        runtime.apply_board(&room_board(
            &session,
            Some("app.rooms"),
            None,
            Some((status, review)),
        ));
        assert_eq!(
            runtime
                .room_projection()
                .anchor_authority(&room(), runtime.board()),
            app::RoomAnchorAuthority::Settled {
                handle: wire::TaskHandle::for_project("app.rooms", &room()),
                standing,
            }
        );
    }
    assert!(
        runtime
            .room_projection()
            .room(&room())
            .expect("re-homed room")
            .events()
            .contains_key(&authority(ROOM_EVENT)),
        "a re-homed anchor keeps its durable room cards"
    );
}

#[test]
fn room_snapshots_refuse_moved_horizons_regressions_and_live_gaps() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    let requires_snapshot = |runtime: &AppRuntime| {
        runtime.room_projection().status() == app::RoomProjectionStatus::RequiresSnapshot
    };

    runtime.apply_stream(&room_page(
        &session,
        &[group_moved(1, &[])],
        Some((1, 3)),
        4,
    ));
    assert_eq!(
        runtime.apply_stream(&room_page(&session, &[room_message(3)], None, 5)),
        vec![StreamDirective::Reconnect],
        "every page of one snapshot declares the same horizon"
    );
    assert!(requires_snapshot(&runtime));

    assert!(!runtime.reconnect(&accepted));
    runtime.apply_stream(&room_page(
        &session,
        &[group_moved(1, &[])],
        Some((1, 3)),
        4,
    ));
    assert_eq!(
        runtime.apply_stream(&room_page(&session, &[room_message(2)], None, 4)),
        vec![StreamDirective::Reconnect],
        "a continuation never moves backward"
    );
    assert!(requires_snapshot(&runtime));

    assert!(!runtime.reconnect(&accepted));
    runtime.apply_stream(&admitted_stream(
        &session,
        &live_fact(group_moved(6, &["PUBLIC-work2"])),
    ));
    assert_eq!(
        runtime.apply_stream(&room_page(&session, &[group_moved(1, &[])], None, 5)),
        vec![StreamDirective::Reconnect],
        "a live fact past the snapshot horizon before completion is a gap"
    );
    assert!(requires_snapshot(&runtime));

    // The runtime already admitted live position 6; overlap is judged above it.
    assert!(!runtime.reconnect(&accepted));
    runtime.apply_stream(&room_page(&session, &[group_moved(1, &[])], None, 8));
    assert_eq!(
        runtime.room_projection().status(),
        app::RoomProjectionStatus::Available
    );
    assert_eq!(
        runtime.apply_stream(&admitted_stream(
            &session,
            &live_fact(group_moved(7, &["PUBLIC-work2"]))
        )),
        vec![StreamDirective::Reconnect],
        "a live fact at or before the horizon overlaps the snapshot"
    );
    assert!(requires_snapshot(&runtime));

    assert!(!runtime.reconnect(&accepted));
    runtime.apply_stream(&room_page(&session, &[group_moved(1, &[])], None, 8));
    runtime.apply_stream(&admitted_stream(
        &session,
        &live_fact(group_moved(9, &["PUBLIC-work2"])),
    ));
    assert_eq!(
        runtime.room_projection().partition().groups,
        vec![group(&["PUBLIC-work2"])]
    );
    assert_eq!(
        runtime.room_projection().status(),
        app::RoomProjectionStatus::Available
    );
}

fn live(runtime: &mut AppRuntime, session: &Session, fact: serde_json::Value) {
    runtime.apply_stream(&admitted_stream(session, &live_fact(fact)));
}
