//! Room authority comes from each subscription's room snapshot and the live
//! facts after its horizon, never from browser-local foreground state.

use super::*;
use serde::Serialize;
use wire::principal::{PrincipalAdmitted, PrincipalProfile, PrincipalRole};
use wire::rooms::{
    CandidateRef, CandidateReference, CandidateWithdrawal, CandidateWithdrawn, CoverageRef,
    DecisionOutcome, DecisionRef, DesignPassProposal, DesignPassRecommendation, DesignProposed,
    ExecutionAuthorization, ExecutionAuthorized, PreparationDecided, PreparationDecision,
    RoomAuthorityStamp, RoomMessaged, RoomSource, TextRange, WorkingGroup, WorkingGroupMoved,
    WorkingGroupPartition,
};

#[path = "rooms/contributions.rs"]
mod contributions;
#[path = "rooms/conversation.rs"]
mod conversation;

const ROOM_EVENT: &str = "dVsYdDj7d";
const LEAVE_EVENT: &str = "dVsYdDj7k";
const GROUP: &str = "dVsYdDj7f";
const CANDIDATE: &str = "dVsYdDj7h";
const SECOND_CANDIDATE: &str = "dVsYdDj7m";
const STOP_KEY: &str = "dStopKey1";
/// The claim win the fixture stop request names.
const STOPPED_CLAIM: u64 = 3;

fn authority(text: &str) -> RoomAuthorityStamp {
    text.parse().expect("fixture room authority")
}

fn operator_post() -> RoomAuthorityStamp {
    RoomAuthorityStamp::for_micros(authority(ROOM_EVENT).micros() + 1)
        .expect("fixture operator post authority")
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

fn room_post(position: u64) -> serde_json::Value {
    envelope(
        position,
        "room::published",
        &wire::rooms::RoomPublished {
            room: room(),
            publication: operator_post(),
            content: wire::rooms::RoomPublicationContent::Authored {
                text: "Please inspect this room.".to_owned(),
            },
        },
    )
}

/// The stop request asking `lane`, holding the room's claim won at
/// [`STOPPED_CLAIM`], to checkpoint and free.
fn stop_direction(position: u64, lane: &str) -> serde_json::Value {
    let body = serde_json::json!({
        "directive": REQUEST,
        "key": STOP_KEY,
        "lane": lane,
        "text": "Checkpoint and free this Design",
        "control": format!("room-stop:{}:{}", room(), wire::SequenceCode::new(STOPPED_CLAIM)),
        "expires": "2030-04-13T18:44:09.271Z",
    });
    envelope(position, "steer::sent", &body)
}

fn stop() -> app::StopRequest {
    app::StopRequest {
        key: STOP_KEY.to_owned(),
        lane: uuid(LANE),
        claim: wire::SequenceCode::new(STOPPED_CLAIM),
    }
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

fn retained(
    position: u64,
    candidate: &str,
    recommendation: DesignPassRecommendation,
    output: Option<&str>,
) -> serde_json::Value {
    envelope(
        position,
        "proposal::retained",
        &DesignProposed {
            candidate: DesignPassProposal {
                candidate: CandidateRef {
                    controller: room(),
                    candidate: authority(candidate),
                },
                statement: seq(10),
                claim: Some(seq(11)),
                recommendation,
                governed_inputs: output.is_some().then(room).into_iter().collect(),
                outputs: output.into_iter().map(fixture_stamp).collect(),
                references: vec![CandidateReference {
                    relation: "contract".parse().expect("fixture relation"),
                    source: RoomSource::Journal {
                        fact: wire::SequenceCode::new(1),
                    },
                }],
                justification: "the cited contract supports this pass".to_owned(),
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
/// settled.
fn room_board(
    session: &Session,
    project: Option<&str>,
    claimed: Option<u64>,
    settlement: Option<wire::WorkflowStatus>,
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
        if let Some(status) = settlement {
            task.workflow.body.status = status;
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

fn participants(runtime: &AppRuntime) -> Vec<app::RoomParticipant> {
    runtime
        .room_projection()
        .participants(&room(), runtime.board())
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
            &[authorized(4, 1), stop_direction(7, LANE)],
            None,
            8,
        ),
    ]
}

#[test]
fn unheard_operator_post_tracks_current_listening_and_later_lane_events() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    runtime.apply_stream(&operator_facts(&session, 1));
    runtime.apply_stream(&room_page(&session, &[room_post(1)], None, 1));
    runtime.apply_board(&room_board(&session, Some("app.rooms"), None, None));

    let is_unheard = |runtime: &AppRuntime| {
        let projection = runtime.room_projection();
        let has_current_listener = projection.has_current_listener(&room(), runtime.board());
        projection.unheard_post(&room(), &operator_post(), has_current_listener)
    };
    assert!(is_unheard(&runtime), "no lane can have heard the post");
    assert!(
        !runtime
            .room_projection()
            .unheard_post(&room(), &authority(ROOM_EVENT), false),
        "non-post room authority is never marked unheard"
    );

    runtime.apply_board(&room_board(&session, Some("app.rooms"), Some(3), None));
    assert!(
        !is_unheard(&runtime),
        "a current participant receives the post through catch-up"
    );

    runtime.apply_board(&room_board(&session, Some("app.rooms"), None, None));
    runtime.apply_stream(&admitted_stream(&session, &live_fact(room_message(2))));
    assert!(
        !runtime
            .room_projection()
            .has_current_listener(&room(), runtime.board()),
        "authoring the message creates participation but no listening authority"
    );
    let left = authority(LEAVE_EVENT);
    runtime.apply_stream(&admitted_stream(
        &session,
        &live_fact(envelope(
            3,
            "room::left",
            &wire::rooms::RoomLeft {
                room: room(),
                publication: left,
                lane: uuid(LANE),
                clause: "read the post".to_owned(),
            },
        )),
    ));
    assert!(
        !is_unheard(&runtime),
        "a later lane event permanently proves that the post was heard"
    );
    assert!(
        !runtime
            .room_projection()
            .unheard_post(&room(), &authority(ROOM_EVENT), false),
        "a lane-authored room event is not an operator post"
    );
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
        [(ROOM_EVENT, 2)]
            .map(|(id, position)| (authority(id), seq(position)))
            .into()
    );
    assert_eq!(
        participants(&runtime),
        vec![app::RoomParticipant {
            lane: uuid(LANE),
            reasons: vec![app::ParticipantReason::Publication(authority(ROOM_EVENT))],
        }],
        "a message author occupant is never a participant key, and a stop without a board is no duty"
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
            retained(
                2,
                CANDIDATE,
                DesignPassRecommendation::Work,
                Some("HIDDEN-oops1"),
            ),
            decided(3, CANDIDATE, 1, |candidate| DecisionOutcome::Returned {
                candidate,
                reason: "the evidence is incomplete".to_owned(),
            }),
            retained(
                4,
                SECOND_CANDIDATE,
                DesignPassRecommendation::Collapse,
                None,
            ),
            decided(5, SECOND_CANDIDATE, 2, |candidate| {
                DecisionOutcome::Accepted { candidate }
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
                &DecisionOutcome::Accepted {
                    candidate: CandidateRef {
                        controller: room(),
                        candidate: authority(SECOND_CANDIDATE),
                    },
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
            retained(
                2,
                CANDIDATE,
                DesignPassRecommendation::Work,
                Some("HIDDEN-oops1"),
            ),
            retained(
                3,
                SECOND_CANDIDATE,
                DesignPassRecommendation::Work,
                Some("PUBLIC-work2"),
            ),
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
            retained(
                3,
                CANDIDATE,
                DesignPassRecommendation::Work,
                Some("HIDDEN-oops1"),
            ),
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
    runtime.apply_board(&room_board(
        &session,
        Some("app.rooms"),
        Some(STOPPED_CLAIM),
        None,
    ));
    assert_eq!(
        runtime
            .room_projection()
            .current_stop(&room(), runtime.board())
            .map(|stop| (stop.position, stop.value.clone())),
        Some((seq(7), stop()))
    );
    assert_eq!(
        participants(&runtime)[0].reasons,
        vec![
            app::ParticipantReason::Publication(authority(ROOM_EVENT)),
            app::ParticipantReason::Work(room()),
            app::ParticipantReason::Stop(STOP_KEY.to_owned()),
        ]
    );

    runtime.apply_board(&room_board(&session, Some("app.rooms"), Some(5), None));
    assert!(
        runtime
            .room_projection()
            .current_stop(&room(), runtime.board())
            .is_none(),
        "a later claim win leaves the stop sent but not current"
    );
    assert_eq!(
        participants(&runtime)[0].reasons,
        vec![
            app::ParticipantReason::Publication(authority(ROOM_EVENT)),
            app::ParticipantReason::Work(room()),
        ]
    );

    runtime.apply_board(&room_board(
        &session,
        Some("app.rooms"),
        Some(STOPPED_CLAIM),
        None,
    ));
    let canceled = envelope(
        9,
        "steer::canceled",
        &wire::body::SteerCanceled {
            key: STOP_KEY.to_owned(),
            lane: uuid(LANE),
        },
    );
    runtime.apply_stream(&admitted_stream(&session, &live_fact(canceled)));
    runtime.apply_stream(&admitted_stream(&session, &live_fact(authorized(10, 2))));
    assert!(
        runtime
            .room_projection()
            .current_stop(&room(), runtime.board())
            .is_none(),
        "a retired stop no longer binds its lane"
    );
    assert_eq!(
        participants(&runtime)[0].reasons,
        vec![
            app::ParticipantReason::Publication(authority(ROOM_EVENT)),
            app::ParticipantReason::Work(room()),
        ]
    );
    assert_eq!(
        runtime.room_projection().preparation().authorizations.len(),
        2
    );
}

#[test]
fn authored_message_records_participation_without_author_attention() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = AppRuntime::new(&accepted);
    for frame in opening(&session) {
        runtime.apply_stream(&frame);
    }
    assert_eq!(
        participants(&runtime),
        vec![app::RoomParticipant {
            lane: uuid(LANE),
            reasons: vec![app::ParticipantReason::Publication(authority(ROOM_EVENT))],
        }],
        "the author's room history is not a reason to deliver later catch-up"
    );
}

const MICROS_PER_SECOND: i64 = 1_000_000;

/// A room publication identity `seconds` after the fixture room message.
fn later_publication(seconds: i64) -> RoomAuthorityStamp {
    RoomAuthorityStamp::for_micros(authority(ROOM_EVENT).micros() + seconds * MICROS_PER_SECOND)
        .expect("fixture publication")
}

#[test]
fn an_authored_message_does_not_end_a_leave() {
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
    let current = participants(&runtime);
    assert_eq!(
        current,
        vec![app::RoomParticipant {
            lane: uuid(LANE),
            reasons: vec![
                app::ParticipantReason::Publication(authority(ROOM_EVENT)),
                app::ParticipantReason::Left {
                    publication: left,
                    clause: "my part is done".to_owned(),
                },
            ],
        }],
        "the leave stands with its clause"
    );

    let returned = later_publication(3);
    let mut contribution = room_message(10);
    contribution["fact"]["publication"] = serde_json::json!(returned.to_string());
    live(&mut runtime, &session, contribution);
    assert_eq!(
        participants(&runtime)[0].reasons,
        vec![
            app::ParticipantReason::Publication(authority(ROOM_EVENT)),
            app::ParticipantReason::Publication(returned),
            app::ParticipantReason::Left {
                publication: later_publication(1),
                clause: "my part is done".to_owned(),
            },
        ],
        "later authorship adds history without restoring listening"
    );
}

#[test]
fn an_asked_lane_stands_asked_until_it_answers_or_leaves() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let evidence = wire::rooms::DesignNoticeEvidence::Task {
        task: room(),
        statement: wire::Seq::new(1).expect("fixture evidence statement"),
        completed: wire::Seq::new(8).expect("fixture completion"),
        support: wire::rooms::DesignSupport::Implemented,
    };
    let admission = wire::rooms::DesignAdmission::Scored(
        wire::rooms::DesignNoticeMatch::new(wire::rooms::DesignReach::Wide, 0.25, 0.1, [7; 32])
            .expect("fixture match"),
    );
    let asked = || {
        let mut runtime = AppRuntime::new(&accepted);
        for frame in opening(&session) {
            runtime.apply_stream(&frame);
        }
        live(
            &mut runtime,
            &session,
            envelope(
                9,
                "room::noticed",
                &wire::rooms::DesignNoticed {
                    design: room(),
                    statement: wire::Seq::new(1).expect("fixture statement"),
                    lane: uuid(THIRD_LANE),
                    occupant: uuid(REQUEST),
                    session: wire::rooms::DesignSession {
                        vendor: "omp".parse().expect("fixture vendor"),
                        identity: "fixture-session".to_owned(),
                    },
                    compaction: None,
                    opportunity: wire::rooms::DesignNoticeOpportunity::DesignStatement,
                    evidence: evidence.clone(),
                    excerpt: "fixture rooms".to_owned(),
                    terms: vec!["rooms".to_owned()],
                    admission: admission.clone(),
                    asks: Vec::new(),
                },
            ),
        );
        runtime
    };
    let reasons = |runtime: &AppRuntime| {
        participants(runtime)
            .into_iter()
            .find(|participant| participant.lane == uuid(THIRD_LANE))
            .map(|participant| participant.reasons)
    };

    let mut answered = asked();
    assert_eq!(
        reasons(&answered),
        Some(vec![app::ParticipantReason::Asked {
            evidence: evidence.clone(),
            questions: Vec::new(),
            admission: admission.clone(),
        }])
    );
    let contribution = later_publication(1);
    live(
        &mut answered,
        &session,
        envelope(
            10,
            "room::contributed",
            &wire::rooms::RoomContributed {
                room: room(),
                publication: contribution.clone(),
                lane: uuid(THIRD_LANE),
                contribution: wire::rooms::RoomContributionKind::charge(1),
                question: None,
                item: None,
                statement: None,
                text: "the fold already covers this".to_owned(),
                sources: Vec::new(),
            },
        ),
    );
    assert_eq!(
        reasons(&answered),
        Some(vec![app::ParticipantReason::Publication(contribution)]),
        "a contribution answers the request and subscribes no one"
    );

    let mut declined = asked();
    let left = later_publication(1);
    live(
        &mut declined,
        &session,
        envelope(
            10,
            "room::left",
            &wire::rooms::RoomLeft {
                room: room(),
                publication: left.clone(),
                lane: uuid(THIRD_LANE),
                clause: "not my problem space".to_owned(),
            },
        ),
    );
    assert_eq!(
        reasons(&declined),
        Some(vec![app::ParticipantReason::Left {
            publication: left,
            clause: "not my problem space".to_owned(),
        }]),
        "a leave declines the request"
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

    // The socket drops; a renamed group lands unseen.
    assert!(!runtime.reconnect(&accepted));
    assert_eq!(
        runtime.room_projection().status(),
        app::RoomProjectionStatus::Loading
    );
    assert!(runtime.room_projection().room(&room()).is_none());
    runtime.apply_stream(&operator_facts(&session, 7));
    runtime.apply_stream(&room_page(
        &session,
        &[room_message(2), group_moved(4, &["PUBLIC-work2"])],
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
    let at = || "2025-01-01T00:00:00.000Z".parse().expect("fixture instant");
    for (status, standing) in [
        (
            wire::WorkflowStatus::reviewed(at()),
            app::RoomAnchorSettlement::Completed,
        ),
        (
            wire::WorkflowStatus::closed(at()),
            app::RoomAnchorSettlement::Closed,
        ),
        (
            wire::WorkflowStatus::deleted(at()),
            app::RoomAnchorSettlement::Deleted,
        ),
    ] {
        runtime.apply_board(&room_board(&session, Some("app.rooms"), None, Some(status)));
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
