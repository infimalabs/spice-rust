/// One durable envelope around a fact body, at a position nothing else uses.
fn envelope(
    position: u64,
    actor: &str,
    origin: &str,
    fact: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "v": 1,
        "seq": position,
        "at": "2030-04-12T18:44:09.271Z",
        "actor": actor,
        "origin": origin,
        "fact": fact,
    })
}

/// The one expiry every timed row in the inventory carries.
const EXPIRES: &str = "2030-04-12T19:44:09.271Z";

/// One fact, and the cached reads it invalidates.
type StaleRow = (serde_json::Value, [bool; 5]);

/// Facts from `steer::*`, the namespace only the inbox is drawn from.
fn steer_rows() -> Vec<StaleRow> {
    vec![
        (
            serde_json::json!({
                "kind": "steer::sent",
                "directive": REQUEST,
                "key": "QmTKOamJ",
                "lane": LANE,
                "text": "look at the tile",
                "expires": EXPIRES,
            }),
            [false, false, true, false, false],
        ),
        (
            serde_json::json!({"kind": "steer::canceled", "key": "QmTKOamJ", "lane": LANE}),
            [false, false, true, false, false],
        ),
    ]
}

/// Facts from `lane::*`, which move the roster, the inbox and effort together.
fn lane_rows() -> Vec<StaleRow> {
    vec![
        (
            serde_json::json!({
                "kind": "lane::seated",
                "lane": LANE,
                "lineage_id": LANE,
                "worktree": "/tmp/lane",
                "life": "continuous",
                "vendor": "codex",
            }),
            [false, true, true, false, true],
        ),
        (
            serde_json::json!({
                "kind": "lane::exited",
                "lane": LANE,
                "occupant": REQUEST,
                "code": 0,
            }),
            [false, true, true, false, true],
        ),
    ]
}

/// Facts from `task::*` and `claim::*`, the two namespaces the board is drawn
/// from. Both move effort as well, because what a task cost is accounted
/// against the claim that was held while it was spent.
fn task_and_claim_rows() -> Vec<StaleRow> {
    vec![
        (
            serde_json::json!({"kind": "task::phased", "task": "SPICE-aBcD", "phase": "verify"}),
            [true, false, false, false, true],
        ),
        (
            serde_json::json!({
                "kind": "task::done",
                "task": "SPICE-aBcD",
                "validation": "the suite is green",
            }),
            [true, false, false, false, true],
        ),
        (
            serde_json::json!({
                "kind": "claim::won",
                "task": "SPICE-aBcD",
                "actor": "operator",
                "lane": LANE,
                "occupant": REQUEST,
                "epoch": EPOCH,
                "until": EXPIRES,
            }),
            [true, true, false, false, true],
        ),
        (
            serde_json::json!({"kind": "claim::freed", "task": "SPICE-aBcD", "actor": "operator"}),
            [true, true, false, false, true],
        ),
    ]
}

/// Facts from `consent::*`, the namespace only the consent queue is drawn from.
fn consent_rows() -> Vec<StaleRow> {
    vec![
        (
            serde_json::json!({
                "kind": "consent::asked",
                "request": REQUEST,
                "lane": LANE,
                "operation": "write /etc/hosts",
            }),
            [false, false, false, true, false],
        ),
        (
            serde_json::json!({
                "kind": "consent::granted",
                "request": REQUEST,
                "lane": LANE,
                "rule": "operator",
            }),
            [false, false, false, true, false],
        ),
    ]
}

/// Facts from `journal::*` and `lineage::*`, which no cached read but effort is
/// drawn from: the first rewrites the accounted transcript the counters come
/// from, the second re-attributes which lineage owns a source locus. Both move
/// effort while the board, the roster and the inbox stand, so these rows are
/// what separate a per-read mapping from one that widens every read together.
fn journal_and_lineage_rows() -> Vec<StaleRow> {
    vec![
        (
            serde_json::json!({
                "kind": "journal::ingested",
                "lane": LANE,
                "vendor": "codex",
                "stream": "transcript",
                "source": REQUEST,
                "from": {"member": 0, "offset": 0},
                "to": {"member": 0, "offset": 4096},
                "usage": [],
                "non_usage_records": 3,
                "residue_records": 0,
            }),
            [false, false, false, false, true],
        ),
        (
            serde_json::json!({"kind": "journal::migrated", "from": 1, "to": 2, "epoch": EPOCH}),
            [false, false, false, false, true],
        ),
        (
            serde_json::json!({
                "kind": "lineage::configured",
                "lineage_id": LANE,
                "member": REQUEST,
                "from": "steer",
                "life": "drive",
            }),
            [false, false, false, false, true],
        ),
        (
            serde_json::json!({
                "kind": "lineage::arranged",
                "from": [{"lineage_id": LANE, "lanes": [LANE]}],
                "lineages": [{"lineage_id": LANE, "lanes": []}],
            }),
            [false, false, false, false, true],
        ),
    ]
}

/// Facts no cached read is drawn from at all. A rule that simply refreshed
/// everything on every fact would pass every row above and fail these.
fn unread_namespace_rows() -> Vec<StaleRow> {
    vec![
        (
            serde_json::json!({"kind": "up::dark", "epoch": EPOCH}),
            [false, false, false, false, false],
        ),
        (
            serde_json::json!({"kind": "vendor::armed", "vendor": "codex", "version": "0.1.0"}),
            [false, false, false, false, false],
        ),
        (
            serde_json::json!({"kind": "taste::cleared", "taste": "loud", "locus": "app/src"}),
            [false, false, false, false, false],
        ),
    ]
}

/// Which cached read each fact invalidates, as
/// `[board, lanes, inbox, consent, effort]`.
///
/// This inventory is written out above rather than read back off the mapping it
/// checks, so deleting an arm fails instead of agreeing with itself. The groups
/// are the namespaces the mapping keys on, and every one of them appears twice
/// under different kinds and different producers: a mapping keyed on one exact
/// kind rather than on the namespace fails on the sibling instead of passing on
/// the twin.
fn staleness_inventory() -> Vec<StaleRow> {
    steer_rows()
        .into_iter()
        .chain(lane_rows())
        .chain(task_and_claim_rows())
        .chain(consent_rows())
        .chain(journal_and_lineage_rows())
        .chain(unread_namespace_rows())
        .collect()
}

#[test]
fn a_live_fact_invalidates_exactly_the_reads_its_namespace_moves() {
    let session = session(OPERATOR, EPOCH);
    let inventory = staleness_inventory();
    // The inventory is assembled from one group per namespace, so a dropped
    // group would take its whole namespace out of this loop while every
    // surviving row still passed. The count is what makes that a failure.
    assert_eq!(inventory.len(), 17, "a namespace left the inventory");
    for (fact, expected) in inventory {
        let kind = fact["kind"]
            .as_str()
            .expect("fixture names its kind")
            .to_owned();
        let pushed = fact_frame(envelope(41, "spice", &format!("lane:{LANE}"), &fact));
        let stale = stale_reads(&admitted_stream(&session, &pushed));
        assert_eq!(
            [
                stale.board(),
                stale.lanes(),
                stale.inbox(),
                stale.consent(),
                stale.effort(),
            ],
            expected,
            "{kind} invalidated the wrong reads"
        );
        assert_eq!(
            stale.any(),
            expected.contains(&true),
            "{kind} disagrees with its own summary"
        );
    }
}

#[test]
fn only_the_tail_of_a_backlog_invalidates_reads_and_a_thread_walk_never_does() {
    let session = session(OPERATOR, EPOCH);
    let sent = serde_json::json!({
        "kind": "steer::sent",
        "directive": REQUEST,
        "key": "QmTKOamJ",
        "lane": LANE,
        "text": "look at the tile",
        "expires": "2030-04-12T19:44:09.271Z",
    });
    let won = serde_json::json!({
        "kind": "claim::won",
        "task": "SPICE-aBcD",
        "actor": "operator",
        "lane": LANE,
        "occupant": REQUEST,
        "epoch": EPOCH,
        "until": "2030-04-12T19:44:09.271Z",
    });
    let lane = format!("lane:{LANE}");
    let ascending = [
        envelope(41, "spice", &lane, &sent),
        envelope(42, "operator", "task:SPICE-aBcD", &won),
    ];

    let tail = admitted_stream(&session, &packed_frame("facts", &ascending, None, 42));
    let stale = stale_reads(&tail);
    assert_eq!(
        [
            stale.board(),
            stale.lanes(),
            stale.inbox(),
            stale.consent(),
            stale.effort(),
        ],
        [true, true, true, false, true],
        "a page widens to the union of its rows and no further"
    );

    let continued = admitted_stream(
        &session,
        &packed_frame("facts", &ascending, Some((7, 43)), 99),
    );
    assert_eq!(
        stale_reads(&continued),
        Stale::default(),
        "a page that continues defers to the page that ends the backlog"
    );

    let descending = [
        envelope(42, "operator", "task:SPICE-aBcD", &won),
        envelope(41, "spice", &lane, &sent),
    ];
    let thread = admitted_stream(&session, &packed_frame("thread", &descending, None, 42));
    assert_eq!(
        stale_reads(&thread),
        Stale::default(),
        "a causal walk is history rather than an advance"
    );
}

/// The tile ladder is the predecessor's, and every rung has its own cause.
///
/// The five rungs are read from three independent inputs — queued direction,
/// typed occupant liveness, and a declared session — so the fixture varies one
/// at a time rather than presenting lanes that differ in every field. The
/// labels are written out here rather than read back from [`LaneStatus`],
/// because a test that asks the enum what its wording is passes after the
/// wording changes.
///
/// The rank is asserted through a shuffled sort because the strip orders its
/// tiles by `Ord` alone: a ladder that named the five states in the right
/// order but derived the wrong comparison would still put the lane waiting on
/// the operator at the far end of the strip.
#[test]
fn a_lane_tile_ranks_queued_direction_above_liveness_and_liveness_above_a_cold_seat() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let bytes = lane_response(&[
        (SECOND_LANE, SECOND_LANE, None, false),
        (LANE, LANE, Some(OPERATOR), false),
        (THIRD_LANE, THIRD_LANE, None, true),
    ]);
    let admitted: app::Admitted<Response<wire::intent::LaneAnswer>> = session
        .admit_frame(&bytes)
        .expect("lane answer is admitted");
    runtime.apply_lanes(&admitted);

    let lanes = runtime
        .lanes()
        .expect("the roster is retained")
        .lanes
        .clone();
    let [never_seated, occupied, seated_before] = <[_; 3]>::try_from(lanes)
        .unwrap_or_else(|rows| panic!("three rows survive admission, got {}", rows.len()));
    let never_seated = never_seated.host().expect("lineage has one member");
    let occupied = occupied.host().expect("lineage has one member");
    let seated_before = seated_before.host().expect("lineage has one member");
    assert_eq!(never_seated.status(0), LaneStatus::Unbound);
    assert_eq!(
        seated_before.status(0),
        LaneStatus::Bound,
        "a seat with a declared session is bound rather than unbound once its occupant leaves"
    );
    assert_eq!(occupied.status(0), LaneStatus::Running);
    assert_eq!(occupied.status(2), LaneStatus::Queued);
    assert_eq!(
        never_seated.status(1),
        LaneStatus::Queued,
        "queued direction outranks liveness even in a seat nobody has ever occupied"
    );

    assert_eq!(LaneStatus::Queued.label(), "steering queued");
    assert_eq!(LaneStatus::Running.label(), "agent running");
    assert_eq!(LaneStatus::RunningStale.label(), "agent running-stale");
    assert_eq!(LaneStatus::Bound.label(), "agent bound");
    assert_eq!(LaneStatus::Unbound.label(), "agent unbound");
    assert_eq!(LaneStatus::Queued.key(), "queued");
    assert_eq!(LaneStatus::Running.key(), "running");
    assert_eq!(LaneStatus::RunningStale.key(), "running-stale");
    assert_eq!(LaneStatus::Bound.key(), "bound");
    assert_eq!(LaneStatus::Unbound.key(), "unbound");

    let mut strip = vec![
        LaneStatus::Unbound,
        LaneStatus::RunningStale,
        LaneStatus::Running,
        LaneStatus::Bound,
        LaneStatus::Queued,
    ];
    strip.sort_unstable();
    assert_eq!(
        strip,
        vec![
            LaneStatus::Queued,
            LaneStatus::Running,
            LaneStatus::RunningStale,
            LaneStatus::Bound,
            LaneStatus::Unbound,
        ],
        "the strip reads left to right as the predecessor's targetChoiceStatusValues"
    );
}

#[test]
fn one_lineage_groups_multiple_steerable_worktrees_without_losing_member_order() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let bytes = lane_response(&[
        (LANE, EPOCH, Some(OPERATOR), false),
        (SECOND_LANE, EPOCH, None, true),
        (THIRD_LANE, THIRD_LANE, None, false),
    ]);
    let admitted: app::Admitted<Response<wire::intent::LaneAnswer>> = session
        .admit_frame(&bytes)
        .expect("plural lineage answer is admitted");

    runtime.apply_lanes(&admitted);

    let lane_view = runtime.lanes().expect("the roster is retained");
    assert_eq!(lane_view.topology_revision, 9);
    let lanes = &lane_view.lanes;
    assert_eq!(lanes.len(), 2);
    assert_eq!(lanes[0].lane, uuid(EPOCH));
    assert_eq!(lanes[0].members.len(), 2);
    assert_eq!(lanes[0].members[0].lane, uuid(LANE));
    assert_eq!(lanes[0].members[1].lane, uuid(SECOND_LANE));
    assert_eq!(lanes[1].members[0].lane, uuid(THIRD_LANE));
    assert!(
        lanes
            .iter()
            .all(|lineage| lineage.subscription == wire::intent::LineageLife::Steer)
    );
}

#[test]
fn every_distinct_lineage_forms_one_console_even_when_members_have_no_name() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let bytes = lane_response(&[
        (LANE, LANE, None, false),
        (SECOND_LANE, SECOND_LANE, None, false),
        (THIRD_LANE, THIRD_LANE, None, false),
    ]);
    let admitted: app::Admitted<Response<wire::intent::LaneAnswer>> = session
        .admit_frame(&bytes)
        .expect("distinct lineage answer is admitted");

    runtime.apply_lanes(&admitted);

    let lineages = &runtime.lanes().expect("the roster is retained").lanes;
    assert_eq!(
        lineages.len(),
        3,
        "distinct lineage keys never share a console"
    );
    assert_eq!(
        lineages
            .iter()
            .map(|lineage| lineage.lane)
            .collect::<Vec<_>>(),
        vec![uuid(LANE), uuid(SECOND_LANE), uuid(THIRD_LANE)]
    );
    assert!(lineages.iter().all(|lineage| {
        lineage.members.len() == 1 && lineage.host().is_some_and(|member| member.name.is_none())
    }));
}

/// The one steering receipt shape quotes the key exactly as the resident set it.
///
/// The key is a case-sensitive stamp drawn from a vowel-free mixed-case
/// alphabet, and it is the handle an operator types back at the CLI to cancel
/// or recover the directive they just sent. Folding its case would print a key
/// that names no directive. Only the disposition is a word, and it is spelled
/// by an exhaustive match rather than by rendering the variant, so a new
/// retirement state must state its spelling instead of reaching the operator
/// as a debug rendering.
#[test]
fn a_steering_receipt_preserves_the_directive_key_and_lowercases_only_its_disposition() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(41)),
            body: wire::intent::DirectiveAnswer {
                directive: uuid(LANE),
                key: "1kKQtLsP".to_owned(),
                disposition: wire::Disposition::Acked,
                delivery: Box::new(wire::intent::DeliveryStanding::Unknown),
            },
        }),
    );
    let admitted: app::Admitted<Response<wire::intent::DirectiveAnswer>> =
        session.admit_frame(&bytes).expect("response is admitted");
    runtime.apply_steer(&admitted, uuid(LANE));

    let event = runtime
        .events()
        .first()
        .expect("steering records one event");
    assert_eq!(event.title(), format!("Steered lane {LANE}"));
    assert_eq!(event.detail(), "key 1kKQtLsP acked");
}

/// A seating receipt names the lane and both dispositions it moved between.
///
/// This is the only confirmation the operator gets that the seat form did
/// anything: the lane list refreshes on its own schedule, so between the click
/// and that refresh the receipt is the whole answer. All three fields are
/// resident-authored and each is load-bearing — the lane says which of several
/// rows moved, and `absent` to `occupied` is what distinguishes a fresh seat
/// from a wake of a lane that was merely vacant.
#[test]
fn a_seating_receipt_names_the_lane_and_the_dispositions_it_moved_between() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(17)),
            body: wire::intent::LaneChangeAnswer {
                lane: uuid(LANE),
                topology_revision: 10,
                from: "absent".to_owned(),
                to: "occupied".to_owned(),
                git: Some("current".into()),
            },
        }),
    );
    let admitted: app::Admitted<Response<wire::intent::LaneChangeAnswer>> =
        session.admit_frame(&bytes).expect("response is admitted");
    runtime.apply_lane_change(&admitted);

    let event = runtime.events().first().expect("seating records one event");
    assert_eq!(event.title(), "Agent seated");
    assert_eq!(
        event.detail(),
        format!("{LANE} moved from absent to occupied")
    );
}

#[test]
fn admitted_command_reply_refusal_and_fault_remain_three_chronology_rows() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let outcomes: [Outcome<wire::intent::TaskAnswer>; 3] = [
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(41)),
            body: wire::intent::TaskAnswer {
                task: "SPICE-aBcD".parse().expect("fixture task handle"),
                status: "done".to_owned(),
                learning: None,
                next: None,
            },
        }),
        Outcome::Refusal(Refusal::bare(wire::RefusalDiagnostic::new(
            "task::missing".to_owned(),
            None,
            None,
            None,
        ))),
        Outcome::Fault(Fault {
            class: FaultClass::Protocol,
            reason: "command::stale".to_owned(),
            detail: "epoch moved".to_owned(),
        }),
    ];
    for outcome in outcomes {
        let bytes = support::response(WireVersion::CURRENT, uuid(REQUEST), uuid(EPOCH), outcome);
        let admitted: app::Admitted<Response<wire::intent::TaskAnswer>> =
            session.admit_frame(&bytes).expect("response is admitted");
        runtime.apply_note(&admitted, "SPICE-aBcD");
    }

    assert_eq!(
        runtime
            .events()
            .iter()
            .map(app::RuntimeEvent::class)
            .collect::<Vec<_>>(),
        vec!["fault", "refusal", "reply"]
    );
}

#[test]
fn the_second_command_outcome_is_the_ticker_source() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let outcomes: [Outcome<wire::intent::TaskAnswer>; 2] = [
        Outcome::Refusal(Refusal::bare(wire::RefusalDiagnostic::new(
            "task::first".to_owned(),
            None,
            None,
            None,
        ))),
        Outcome::Fault(Fault {
            class: FaultClass::Protocol,
            reason: "command::second".to_owned(),
            detail: "second detail".to_owned(),
        }),
    ];
    for outcome in outcomes {
        let bytes = support::response(WireVersion::CURRENT, uuid(REQUEST), uuid(EPOCH), outcome);
        let admitted: app::Admitted<Response<wire::intent::TaskAnswer>> =
            session.admit_frame(&bytes).expect("response is admitted");
        runtime.apply_note(&admitted, "SPICE-aBcD");
    }

    let ticker = runtime.newest_event().expect("the ticker has one source");
    assert_eq!(ticker.class(), "fault");
    assert_eq!(ticker.title(), "command::second");
    assert_eq!(ticker.detail(), "second detail");
}

#[test]
fn admitted_steer_reply_names_the_durable_directive_in_chronology() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let bytes = support::response(
        WireVersion::CURRENT,
        uuid(REQUEST),
        uuid(EPOCH),
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(41)),
            body: wire::intent::DirectiveAnswer {
                directive: uuid(LANE),
                key: "1kQTest".to_owned(),
                disposition: wire::Disposition::Live,
                delivery: Box::new(wire::intent::DeliveryStanding::Unknown),
            },
        }),
    );
    let admitted: app::Admitted<Response<wire::intent::DirectiveAnswer>> = session
        .admit_frame(&bytes)
        .expect("steer response is admitted");

    runtime.apply_steer(&admitted, uuid(LANE));

    assert_eq!(runtime.events()[0].class(), "reply");
    assert_eq!(runtime.events()[0].title(), format!("Steered lane {LANE}"));
    assert_eq!(runtime.events()[0].detail(), "key 1kQTest live");
}
