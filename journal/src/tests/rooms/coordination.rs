//! Room coordination records bound to the operator directions they name.

use wire::body::{SteerCanceled, SteerSent, SteeringStage};
use wire::rooms::{
    CoordinationControl, DesignNoticeEvidence, DesignNoticeOpportunity, DesignSession,
    RoomAutomatedResume, RoomCoordinationStarted, RoomPutAside, RoomResumeRule, RoomStopRequest,
};
use wire::{Disposition, SteeringHead};

use super::*;

/// Lane that holds the claim `add_claimed_task` wins.
const HOLDER: &str = "018f82d4-6f59-7d06-92e3-bce9131163d1";
const OTHER_LANE: &str = "018f82d4-6f59-7d06-92e3-bce9131163d9";
const KEYS: [&str; 4] = ["1kJK6N3R", "1kJK6N3S", "1kJK6N3T", "1kJK6N3V"];

fn target() -> wire::OccupantAddress {
    "6SNy".parse().expect("occupant address")
}

/// Stage one operator direction `KEYS[index]` to `lane` and return its key.
fn send(
    write: &mut JournalWrite,
    index: usize,
    control: Option<CoordinationControl>,
    lane: &str,
) -> String {
    let body = SteerSent {
        from: None,
        room: None,
        directive: uuid(&format!("018f82d4-6f59-7d06-92e3-bce91311640{index}")),
        fanout: None,
        key: KEYS[index].to_owned(),
        lane: uuid(lane),
        text: "contribute to the room".to_owned(),
        control: control.map(|control| control.to_string()),
        attachments: Vec::new(),
        expires: "2030-04-12T19:44:09.271Z".parse().expect("expiry"),
    };
    let pending = write
        .fact(body.clone(), FactCause::Root)
        .expect("direction builds");
    let key = body.key.clone();
    write.steering_head(pending, move |seq| SteeringHead {
        from: None,
        room: None,
        directive: body.directive,
        fanout: None,
        key: body.key,
        lane: body.lane,
        text: body.text,
        control: body.control,
        attachments: Vec::new(),
        published: instant(),
        expires: body.expires,
        stage: SteeringStage::Published,
        escalations: Vec::new(),
        disposition: Disposition::Live,
        origin: "task:rooms".parse().expect("steering origin"),
        created: seq,
        minted_micros: None,
    });
    key
}

/// Stage the cancellation of one committed live direction.
fn cancel(journal: &Journal, write: &mut JournalWrite, key: &str) {
    let head = journal
        .read(|read| read.steering_key(key))
        .expect("steering reads")
        .expect("direction exists");
    let pending = write
        .fact(
            SteerCanceled {
                key: key.to_owned(),
                lane: head.body().lane,
            },
            FactCause::Existing(head.body().created),
        )
        .expect("cancellation builds");
    let mut next = head.body().clone();
    next.disposition = Disposition::Canceled;
    write.steering_head(pending, move |_| next);
}

fn started(room: &TaskStamp, micros: i64, key: &str) -> RoomCoordinationStarted {
    RoomCoordinationStarted {
        room: room.clone(),
        publication: authority(micros),
        key: key.to_owned(),
        target: target(),
        rule: None,
        automated: None,
    }
}

/// One write staging `directions` in order and then a start naming `named`.
fn start(
    actor: Actor,
    room: &TaskStamp,
    micros: i64,
    directions: &[(usize, Option<CoordinationControl>)],
    named: &str,
) -> JournalWrite {
    record(actor, directions, started(room, micros, named))
}

/// One write staging `directions` in order and then `body`.
fn record(
    actor: Actor,
    directions: &[(usize, Option<CoordinationControl>)],
    body: RoomCoordinationStarted,
) -> JournalWrite {
    let mut write = write_as(actor);
    for (index, control) in directions {
        send(&mut write, *index, control.clone(), HOLDER);
    }
    write.fact(body, FactCause::Root).expect("start builds");
    write
}

fn admit_principal(journal: &mut Journal, principal: &str) -> Seq {
    let principal = uuid(principal);
    let roles = std::collections::BTreeSet::from([wire::principal::PrincipalRole::Operator]);
    let mut write = write_as(Actor::Spice);
    let admitted = write
        .fact(
            wire::principal::PrincipalAdmitted {
                principal,
                profile: wire::principal::PrincipalProfile::default(),
                roles: roles.clone(),
                capability: None,
            },
            FactCause::Root,
        )
        .expect("principal admission builds");
    write.principal_head(admitted, move |born| wire::principal::PrincipalHead {
        principal,
        born,
        profile: wire::principal::PrincipalProfile::default(),
        roles,
        capability: None,
    });
    journal.apply(write).expect("principal admission commits");
    cursor(journal).expect("principal admission has a position")
}

/// One stop direction: its key index, the claim win it cites, and its lane.
type Stop<'a> = (usize, u64, &'a str);

/// One write canceling `cancels`, then sending `stop`, then putting aside the
/// room with `listed` cancellations.
fn put_aside(
    journal: &Journal,
    actor: Actor,
    room: &TaskStamp,
    cancels: &[&str],
    listed: &[&str],
    stop: Option<Stop<'_>>,
) -> JournalWrite {
    let mut write = write_as(actor);
    for key in cancels {
        cancel(journal, &mut write, key);
    }
    let stop = stop.map(|(index, claim, lane)| {
        let claim = SequenceCode::new(claim);
        let control = CoordinationControl::Stop {
            room: room.clone(),
            claim,
        };
        RoomStopRequest {
            key: send(&mut write, index, Some(control), lane),
            target: target(),
            claim,
        }
    });
    write
        .fact(
            RoomPutAside {
                room: room.clone(),
                publication: authority(9),
                canceled: listed.iter().map(|key| (*key).to_owned()).collect(),
                stop,
            },
            FactCause::Root,
        )
        .expect("put-aside builds");
    write
}

/// A start is admitted only from the operator immediately after the same
/// write's direction carrying that room's coordination control under the key
/// it names; commit, and verification of planted history, refuse the rest.
#[test]
fn start_binds_to_its_writes_operator_coordination_direction() {
    let mut journal = Journal::in_memory().expect("journal");
    let room = task(&mut journal, 1, None);
    let other = task(&mut journal, 2, None);
    let coordinate = |room: &TaskStamp| Some(CoordinationControl::Coordinate(room.clone()));
    let refusals = [
        start(
            Actor::Lane(uuid(HOLDER)),
            &room,
            1,
            &[(0, coordinate(&room))],
            KEYS[0],
        ),
        start(Actor::Operator, &room, 1, &[], KEYS[0]),
        start(Actor::Operator, &room, 1, &[(0, None)], KEYS[0]),
        start(
            Actor::Operator,
            &room,
            1,
            &[(0, coordinate(&other))],
            KEYS[0],
        ),
        start(
            Actor::Operator,
            &room,
            1,
            &[(
                0,
                Some(CoordinationControl::Stop {
                    room: room.clone(),
                    claim: SequenceCode::new(1),
                }),
            )],
            KEYS[0],
        ),
        start(
            Actor::Operator,
            &room,
            1,
            &[(0, coordinate(&room)), (1, None)],
            KEYS[0],
        ),
    ];
    for write in refusals {
        let result = journal.apply(write);
        refused(&journal, result);
    }

    journal
        .apply(start(
            Actor::Operator,
            &room,
            1,
            &[(0, coordinate(&room))],
            KEYS[0],
        ))
        .expect("bound start commits");
    let head = journal
        .read(|read| read.room_direction(&room))
        .expect("coordination reads")
        .expect("start recorded");
    assert_eq!(head.body().key, KEYS[0]);
    assert_eq!(head.position(), cursor(&journal).expect("nonempty"));

    // A direction from an earlier write directly precedes the start, yet the
    // start's own write does not send it.
    let mut earlier = write();
    send(&mut earlier, 1, coordinate(&room), HOLDER);
    journal.apply(earlier).expect("bare direction commits");
    let result = commit(&mut journal, started(&room, 2, KEYS[1]));
    refused(&journal, result);
    journal.verify().expect("bound coordination verifies");

    // History that bypassed the commit binding cannot verify.
    let seq = cursor(&journal).expect("nonempty").get() + 1;
    let mut planted = fact(
        seq,
        None,
        FactKind::RoomCoordinationStarted,
        serde_json::to_value(started(&room, 3, KEYS[0])).expect("start body"),
    );
    planted.actor = Actor::Operator;
    insert_fact(&journal, planted);
    let sql_seq = i64::try_from(seq).expect("seq");
    journal
        .connection
        .execute_batch(&format!(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) VALUES ({sql_seq}, 'task:1kKRRsDP', NULL, {sql_seq}, 0);
             UPDATE cursor_head SET seq = {sql_seq} WHERE singleton = 1;
             UPDATE steering_index SET indexed_seq = {sql_seq} WHERE singleton = 1;",
        ))
        .expect("index the planted start");
    let violations = refused_violations(&journal);
    assert!(
        violations
            .iter()
            .any(|violation| violation.subject == format!("room_authority:{seq}")),
        "{violations:?}"
    );
}

/// Putting a room aside lists exactly its write's cancellations of the room's
/// coordination directions, and a stop is the write's direction to the lane
/// holding the active claim win it cites.
#[test]
fn put_aside_lists_its_writes_cancellations_and_stops_the_active_holder() {
    let mut journal = Journal::in_memory().expect("journal");
    let room =
        super::super::claim_index::add_claimed_task(&mut journal, 1, Actor::Lane(uuid(HOLDER)));
    let win = cursor(&journal).expect("claim won").get();
    let coordinate = Some(CoordinationControl::Coordinate(room.clone()));
    journal
        .apply(start(
            Actor::Operator,
            &room,
            1,
            &[(0, coordinate)],
            KEYS[0],
        ))
        .expect("start commits");
    let mut plain = write();
    send(&mut plain, 2, None, HOLDER);
    journal.apply(plain).expect("plain direction commits");

    let operator = Actor::Operator;
    let holder = Actor::Lane(uuid(HOLDER));
    let refusals = [
        put_aside(&journal, operator.clone(), &room, &[], &[], None),
        put_aside(&journal, operator.clone(), &room, &[], &[KEYS[0]], None),
        put_aside(
            &journal,
            operator.clone(),
            &room,
            &[KEYS[0]],
            &[],
            Some((1, win, HOLDER)),
        ),
        put_aside(
            &journal,
            operator.clone(),
            &room,
            &[KEYS[2]],
            &[KEYS[2]],
            None,
        ),
        put_aside(
            &journal,
            operator.clone(),
            &room,
            &[KEYS[0]],
            &[KEYS[0]],
            Some((1, win + 1, HOLDER)),
        ),
        put_aside(
            &journal,
            operator.clone(),
            &room,
            &[KEYS[0]],
            &[KEYS[0]],
            Some((1, win, OTHER_LANE)),
        ),
        put_aside(
            &journal,
            holder,
            &room,
            &[KEYS[0]],
            &[KEYS[0]],
            Some((1, win, HOLDER)),
        ),
    ];
    for write in refusals {
        let result = journal.apply(write);
        refused(&journal, result);
    }

    journal
        .apply(put_aside(
            &journal,
            operator,
            &room,
            &[KEYS[0]],
            &[KEYS[0]],
            Some((1, win, HOLDER)),
        ))
        .expect("bound put-aside commits");
    let head = journal
        .read(|read| read.room_shelving(&room))
        .expect("coordination reads")
        .expect("put-aside recorded");
    assert_eq!(head.body().canceled, vec![KEYS[0].to_owned()]);
    assert_eq!(
        head.body().stop.as_ref().map(|stop| stop.claim.get()),
        Some(win)
    );
    journal.verify().expect("bound coordination verifies");
}

/// The room snapshot pages exactly the facts behind current authority
/// through its horizon: a head that later moves past the horizon leaves the
/// set, a coordination brings the direction it follows, and nothing else
/// enters.
#[test]
fn room_authority_pages_current_heads_through_their_horizon() {
    let mut journal = Journal::in_memory().expect("journal");
    let room = task(&mut journal, 1, None);
    let grouped = moved(&journal, &room, Some(1));
    commit(&mut journal, grouped).expect("group stored");
    let control = CoordinationControl::Coordinate(room.clone());
    journal
        .apply(start(
            Actor::Operator,
            &room,
            2,
            &[(0, Some(control))],
            KEYS[0],
        ))
        .expect("coordination starts");
    let directed = cursor(&journal).expect("directed");
    let sent = Seq::new(directed.get() - 1).expect("direction position");
    let horizon = directed;

    journal
        .apply(put_aside(
            &journal,
            Actor::Operator,
            &room,
            &[KEYS[0]],
            &[KEYS[0]],
            None,
        ))
        .expect("coordination put aside");
    let shelved = cursor(&journal).expect("shelved");
    let canceled = Seq::new(shelved.get() - 1).expect("cancellation position");
    let restated = arranged(&journal, vec![(1, vec![room.clone()])]);
    commit(&mut journal, restated).expect("group restated");
    let restored = cursor(&journal).expect("restored");

    let pages = |horizon: Seq| {
        let mut start = crate::FactPageStart::After(None);
        let mut pages = Vec::new();
        loop {
            let page = journal
                .read(|read| read.room_authority_page(start, horizon, fact_budget(2)))
                .expect("room authority reads");
            assert_eq!(page.horizon, horizon);
            let rows = page.rows.iter().map(|fact| fact.seq).collect::<Vec<_>>();
            pages.push((rows, page.overflow.map(|overflow| overflow.omitted)));
            let Some(overflow) = page.overflow else {
                return pages;
            };
            start = crate::FactPageStart::At(overflow.continuation);
        }
    };
    assert_eq!(
        pages(horizon),
        vec![(vec![sent, directed], None)],
        "the group head moved past the horizon, so live facts carry it"
    );
    assert_eq!(
        pages(restored),
        vec![
            (vec![sent, directed], Some(3)),
            (vec![canceled, shelved], Some(1)),
            (vec![restored], None),
        ],
        "the current steering head and group head replace their predecessors"
    );
}

/// Every Design notice belongs to the room snapshot, so a lane's invitation
/// survives a reconnect; the tasks it names do not enter it.
#[test]
fn room_authority_pages_every_design_notice() {
    let mut journal = Journal::in_memory().expect("journal");
    let design = task(&mut journal, 1, None);
    let claimed = task(&mut journal, 2, None);
    let mut write = JournalWrite::new(
        Actor::Spice,
        "task:rooms".parse().expect("room origin"),
        instant(),
        wire::Producer::ResidentRecognition,
    );
    write
        .fact(
            wire::rooms::DesignNoticed {
                design,
                statement: wire::Seq::new(1).expect("fixture statement"),
                lane: uuid(HOLDER),
                occupant: uuid(HOLDER),
                session: DesignSession {
                    vendor: "omp".parse().expect("vendor"),
                    identity: "fixture-session".to_owned(),
                },
                compaction: None,
                opportunity: DesignNoticeOpportunity::TaskCompletion {
                    task: claimed.clone(),
                    completed: wire::Seq::new(3).expect("completion"),
                },
                evidence: DesignNoticeEvidence::Task {
                    task: claimed,
                    statement: wire::Seq::new(2).expect("task statement"),
                    completed: wire::Seq::new(3).expect("completion"),
                },
                terms: vec!["rooms".to_owned()],
            },
            crate::FactCause::Root,
        )
        .expect("notice builds");
    journal.apply(write).expect("notice stored");
    let noticed = cursor(&journal).expect("noticed");
    let page = journal
        .read(|read| {
            read.room_authority_page(crate::FactPageStart::After(None), noticed, fact_budget(8))
        })
        .expect("room authority reads");
    assert_eq!(
        page.rows.iter().map(|fact| fact.seq).collect::<Vec<_>>(),
        vec![noticed]
    );
}

/// The room snapshot carries exactly the first admission needed to resolve
/// historical operator authors, in one bounded query; later births cannot
/// move that identity.
#[test]
fn room_authority_pages_the_first_principal_admission_only() {
    let mut journal = Journal::in_memory().expect("journal");
    let first = admit_principal(&mut journal, "018f82d4-6f59-7d06-92e3-bce9131163f0");
    let second = admit_principal(&mut journal, "018f82d4-6f59-7d06-92e3-bce9131163a0");
    crate::guard::reset_indexed_read_metrics();
    let page = journal
        .read(|read| {
            read.room_authority_page(crate::FactPageStart::After(None), second, fact_budget(8))
        })
        .expect("room authority reads");
    assert_eq!(crate::guard::indexed_read_metrics().queries, 1);
    assert_eq!(
        page.rows.iter().map(|fact| fact.seq).collect::<Vec<_>>(),
        vec![first]
    );
}

/// An operator record may authorize 1 through 8 automated resumes; each
/// automated record spends the room's latest rule one step at a time, to the
/// same target, citing a room publication, and none follows a put-aside.
#[test]
fn an_automated_record_spends_its_rooms_live_rule_one_step_at_a_time() {
    let mut journal = Journal::in_memory().expect("journal");
    let room = task(&mut journal, 1, None);
    let coordinate = || Some(CoordinationControl::Coordinate(room.clone()));
    let rule = |limit| RoomResumeRule {
        limit,
        criterion: "criterion".to_owned(),
        contribution: "contribution".to_owned(),
        stop: "stop".to_owned(),
    };
    let resumed = |rule: i64, step, evidence: i64| RoomAutomatedResume {
        rule: authority(rule),
        step,
        evidence: authority(evidence),
    };
    let ruled = |micros, index: usize, rule, automated| {
        let mut body = started(&room, micros, KEYS[index]);
        body.rule = rule;
        body.automated = automated;
        record(Actor::Operator, &[(index, coordinate())], body)
    };

    for refusal in [
        ruled(1, 0, Some(rule(0)), None),
        ruled(1, 0, Some(rule(9)), None),
        ruled(1, 0, Some(rule(3)), Some(resumed(1, 1, 1))),
    ] {
        let result = journal.apply(refusal);
        refused(&journal, result);
    }
    journal
        .apply(ruled(1, 0, Some(rule(3)), None))
        .expect("operator rule commits");

    for refusal in [
        ruled(2, 1, Some(rule(3)), Some(resumed(1, 2, 1))),
        ruled(2, 1, Some(rule(3)), Some(resumed(7, 1, 1))),
        ruled(2, 1, Some(rule(4)), Some(resumed(1, 1, 1))),
        ruled(2, 1, None, Some(resumed(1, 1, 1))),
        ruled(2, 1, Some(rule(3)), Some(resumed(1, 1, 77))),
    ] {
        let result = journal.apply(refusal);
        refused(&journal, result);
    }
    journal
        .apply(ruled(2, 1, Some(rule(3)), Some(resumed(1, 1, 1))))
        .expect("first automated step commits");
    journal
        .apply(ruled(3, 2, Some(rule(3)), Some(resumed(1, 2, 1))))
        .expect("second automated step commits");

    journal
        .apply(put_aside(
            &journal,
            Actor::Operator,
            &room,
            &[KEYS[2]],
            &[KEYS[2]],
            None,
        ))
        .expect("room put aside");
    let result = journal.apply(ruled(4, 3, Some(rule(3)), Some(resumed(1, 3, 1))));
    refused(&journal, result);
    journal.verify().expect("ruled coordination verifies");
}

/// No automated record spends more steps than its rule authorizes.
#[test]
fn an_automated_record_beyond_its_rules_limit_is_refused() {
    let mut journal = Journal::in_memory().expect("journal");
    let room = task(&mut journal, 1, None);
    let rule = RoomResumeRule {
        limit: 1,
        criterion: "criterion".to_owned(),
        contribution: "contribution".to_owned(),
        stop: "stop".to_owned(),
    };
    let ruled = |micros, index: usize, step| {
        let mut body = started(&room, micros, KEYS[index]);
        body.rule = Some(rule.clone());
        body.automated = (step > 0).then(|| RoomAutomatedResume {
            rule: authority(1),
            step,
            evidence: authority(1),
        });
        record(
            Actor::Operator,
            &[(index, Some(CoordinationControl::Coordinate(room.clone())))],
            body,
        )
    };
    journal
        .apply(ruled(1, 0, 0))
        .expect("operator rule commits");
    journal.apply(ruled(2, 1, 1)).expect("the one step commits");
    let result = journal.apply(ruled(3, 2, 2));
    refused(&journal, result);
}
