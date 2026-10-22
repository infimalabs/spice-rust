//! Room coordination records bound to the operator directions they name.

use wire::body::{SteerCanceled, SteerSent, SteeringStage};
use wire::rooms::{
    CoordinationControl, DesignNoticeEvidence, DesignNoticeOpportunity, DesignSession,
    RoomCoordinationStarted, RoomPutAside, RoomStopRequest,
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

/// Every Design notice and nudge belongs to the room snapshot, so a lane's
/// invitation and the posts a nudge carried survive a reconnect; the tasks a
/// notice names do not enter it.
#[test]
fn room_authority_pages_every_design_notice_and_nudge() {
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
                design: design.clone(),
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
                    support: wire::rooms::DesignSupport::Implemented,
                },
                excerpt: "coordinated rooms".to_owned(),
                terms: vec!["rooms".to_owned()],
                stance: None,
                asks: Vec::new(),
            },
            crate::FactCause::Root,
        )
        .expect("notice builds");
    journal.apply(write).expect("notice stored");
    let noticed = cursor(&journal).expect("noticed");
    let mut write = JournalWrite::new(
        Actor::Operator,
        "task:rooms".parse().expect("room origin"),
        instant(),
        wire::Producer::CommandDoor,
    );
    write
        .fact(
            wire::rooms::DesignNudged {
                design,
                message: Some("dVsYdDj7d".parse::<RoomAuthorityStamp>().expect("message")),
            },
            crate::FactCause::Root,
        )
        .expect("nudge builds");
    journal.apply(write).expect("nudge stored");
    let nudged = cursor(&journal).expect("nudged");
    let page = journal
        .read(|read| {
            read.room_authority_page(crate::FactPageStart::After(None), nudged, fact_budget(8))
        })
        .expect("room authority reads");
    assert_eq!(
        page.rows.iter().map(|fact| fact.seq).collect::<Vec<_>>(),
        vec![noticed, nudged]
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

/// Schema 92 let a coordination record carry an automated-resume rule and
/// the step an automated record spent. The crossing drops both from the
/// retained record, its direction head, and the receipts that carried the
/// request's repeat count or the view's budget; the record reads back as the
/// plain coordination it links, and nothing re-folds a rule.
#[test]
fn retained_resume_rules_cross_out_of_their_records_heads_and_receipts() {
    let scratch = Scratch::new();
    let path = scratch.path().join("room-rules.sqlite3");
    let mut journal = authority_file_fixture(&path);
    let room = task(&mut journal, 1, None);
    journal
        .apply(start(
            Actor::Operator,
            &room,
            1,
            &[(0, Some(CoordinationControl::Coordinate(room.clone())))],
            KEYS[0],
        ))
        .expect("coordination commits");
    let before = journal
        .read(|read| read.room_direction(&room))
        .expect("direction reads")
        .expect("direction exists");
    let rule = json!({
        "limit": 3,
        "criterion": "criterion",
        "contribution": "contribution",
        "stop": "stop",
    });
    let automated = json!({"rule": "dVwVYC9yR", "step": 1, "evidence": "dVwVYC9yS"});
    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = CAST(json_set(CAST(envelope AS TEXT), \
             '$.fact.rule', json(?1), '$.fact.automated', json(?2)) AS BLOB) \
             WHERE kind = 'room::directed'",
            params![rule.to_string(), automated.to_string()],
        )
        .expect("restore the retained ruled record");
    journal
        .connection
        .execute(
            "UPDATE room_directions SET body = CAST(json_set(CAST(body AS TEXT), \
             '$.rule', json(?1), '$.automated', json(?2)) AS BLOB)",
            params![rule.to_string(), automated.to_string()],
        )
        .expect("restore the retained ruled head");
    plant_ruled_receipt(&journal, &room, before.position());
    crate::tests::schema::restore_predecessor(&journal, 92);
    drop(journal);

    let journal = Journal::open(&path).expect("ruled coordination crosses");
    journal
        .verify()
        .expect("the crossed record, head, and receipt verify");
    let crossed = journal
        .read(|read| read.room_direction(&room))
        .expect("direction reads")
        .expect("direction still exists");
    assert_eq!(crossed, before, "the record reads back without its rule");
    let (request, response): (Vec<u8>, Vec<u8>) = journal
        .connection
        .query_row(
            "SELECT request, response FROM command_receipts WHERE command_id = ?1",
            [RULED_COMMAND],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("receipt reads");
    let request: wire::Command<wire::intent::RoomCoordinate> =
        wire::decode(&request).expect("the crossed request is current");
    assert_eq!(request.body.criterion, "criterion");
    let response: Response<wire::intent::RoomCoordinationView> =
        wire::decode(&response).expect("the crossed answer is current");
    let Outcome::Reply(reply) = response.outcome else {
        panic!("the receipt kept its reply");
    };
    assert!(reply.body.requests.is_empty());
    drop(journal);
    let reopened = Journal::open(&path).expect("crossed authority reopens");
    assert_eq!(reopened.migration(), None, "the crossing runs exactly once");
}

const RULED_COMMAND: &str = "018f82d4-6f59-7d06-92e3-bce9131163d2";

/// A schema-92 `room::coordinate` receipt whose request carried a repeat
/// count and whose reply carried the rule's budget, committed at `position`.
fn plant_ruled_receipt(journal: &Journal, room: &TaskStamp, position: Seq) {
    const EPOCH: &str = "018f82d4-6f59-7d06-92e3-bce9131163c3";
    let handle = format!("ROOMS-{room}");
    let request = json!({
        "v": 1,
        "id": RULED_COMMAND,
        "epoch": EPOCH,
        "kind": "room::coordinate",
        "expected": [],
        "body": {
            "room": handle,
            "target": "6SNy",
            "action": "start",
            "criterion": "criterion",
            "contribution": "contribution",
            "stop": "stop",
            "auto_resume": 3,
        },
    });
    let response = Response {
        v: WireVersion::CURRENT,
        id: uuid(RULED_COMMAND),
        epoch: uuid(EPOCH),
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: position,
                last: position,
            }),
            body: json!({
                "room": handle,
                "requests": [],
                "rule": {"limit": 3, "spent": 1},
                "hide": false,
            }),
        }),
    };
    journal
        .connection
        .execute(
            "INSERT INTO command_receipts \
             (command_id, actor, command_kind, request, evaluated_witnesses, response, \
              first_seq, last_seq, snapshot_seq, epoch) \
             VALUES (?1, 'operator', 'room::coordinate', ?2, ?3, ?4, ?5, ?5, NULL, ?6)",
            params![
                RULED_COMMAND,
                serde_json::to_vec(&request).unwrap(),
                wire::encode(&Vec::<Witness>::new()).unwrap(),
                wire::encode(&response).unwrap(),
                i64::try_from(position.get()).unwrap(),
                EPOCH,
            ],
        )
        .expect("restore the retained ruled receipt");
}
