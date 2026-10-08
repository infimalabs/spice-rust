//! The source boundary a membership transition records for the lanes it moves.
//!
//! All three transitions are proven by one test because they share one filter
//! and one failure. Each reads `lane_source_boundaries` before it opens its
//! write, so a boundary reaching two of the three facts would leave the third
//! transition's spend unattributable with nothing anywhere to report it. The
//! three facts are asserted elsewhere for the standings they carry, but no case
//! reads their `sources` back, and the one team configuration case that
//! exists moves no member -- the single shape whose moved list is already empty
//! before the filter is ever consulted.
//!
//! The negative case runs the identical sequence over the identical fixture and
//! differs in exactly one input: which lane the claim sits on. That is what
//! makes its absent `sources` the filter refusing an unclaimed lane rather than
//! a fixture that never had a boundary to record.

use super::*;

/// The `sources` the most recent fact of one kind recorded, as stored bytes.
///
/// Answered as an option rather than a list because the field is declared
/// `skip_serializing_if = "Vec::is_empty"`: *no boundary recorded* is the
/// absent key, not an empty array. Reading it back through `Vec` would turn
/// both answers into the same `[]` and hide which one the journal holds.
fn recorded_sources(bus: &Bus, kind: wire::FactKind) -> Option<serde_json::Value> {
    let facts = bus.fact_snapshot().expect("boundary journal reads");
    let recorded = facts
        .facts()
        .iter()
        .rfind(|fact| fact.fact.kind() == kind)
        .unwrap_or_else(|| panic!("no {kind:?} fact is durable"));
    recorded.fact.body().get("sources").cloned()
}

/// The accounted head one seeded batch leaves the lane's stream at.
fn accounted_head(lane: CanonicalUuid, source: CanonicalUuid) -> wire::TranscriptSourceHead {
    wire::TranscriptSourceHead {
        lane,
        vendor: "omp".parse().expect("fixture vendor"),
        stream: "session".to_owned(),
        source,
        next: wire::body::TranscriptPosition {
            member: 1,
            offset: 4096,
        },
    }
}

/// The one boundary entry a transition moving `lane` must record.
///
/// Spelled as literal wire shape rather than encoded from the same value the
/// production path reads back, so the assertion measures the bytes a later
/// reducer receives instead of restating the expression that produced them.
fn expected_sources(lane: CanonicalUuid, source: CanonicalUuid) -> serde_json::Value {
    serde_json::json!([{
        "lane": lane.to_string(),
        "source": {
            "proven": {
                "heads": [{
                    "lane": lane.to_string(),
                    "vendor": "omp",
                    "stream": "session",
                    "source": source.to_string(),
                    "next": { "member": 1, "offset": 4096 },
                }],
            },
        },
    }])
}

/// Account one batch of transcript for a lane, so its boundary carries content.
///
/// A lane with nothing accounted is proven at zero heads, and that empty list
/// is the same value the boundary would hold had the read been skipped
/// altogether. Real accounted position is what separates the two, and it is
/// seeded for the unclaimed lane too: an absent boundary there must be the
/// claim filter and not an absence of anything to prove.
fn account_transcript(bus: &Bus, epoch: CanonicalUuid, lane: CanonicalUuid, source: CanonicalUuid) {
    let head = accounted_head(lane, source);
    let vendor = head.vendor.clone();
    let stream = head.stream.clone();
    let to = head.next;
    let mut write =
        write(Actor::Spice, epoch, Producer::ResidentRecognition).expect("fixture write starts");
    let ingested = write
        .fact(
            wire::body::JournalIngested {
                lane,
                vendor,
                stream,
                source,
                supersedes: None,
                from: wire::body::TranscriptPosition {
                    member: 0,
                    offset: 0,
                },
                to,
                usage: Vec::new(),
                non_usage_records: 0,
                residue_records: 0,
            },
            FactCause::Root,
        )
        .expect("accounted batch");
    write.transcript_source_head(ingested, move |_| head);
    commit(bus, write).expect("fixture transcript commits");
}

/// Seed the task a claim is bound to.
///
/// `claim_heads.stamp` references `task_heads(stamp)` under enforced foreign
/// keys, so the claim cannot stand without one.
fn seed_task(write: &mut JournalWrite, task: &wire::TaskStamp) -> journal::PendingFact {
    let added = write
        .fact(
            wire::body::TaskAdded {
                task: task.clone(),
                title: "Boundary fixture".into(),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::default(),
                project: "journal.effort".into(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: vec!["todo".parse().expect("fixture phase")],
                phase: "todo".parse().expect("fixture phase"),
                creation: wire::body::TaskCreationContext::unavailable(
                    wire::body::TaskCreationUnavailable::SpiceActor,
                ),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("task fact");
    let born = task.clone();
    write.task_head(added, move |position| wire::TaskHead {
        born,
        queue_origin: position,
        title: "Boundary fixture".into(),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: "journal.effort".into(),
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: "resident:018f82d4-6f59-7d06-92e3-000000000001"
            .parse()
            .expect("fixture origin"),
        creation: wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::SpiceActor,
        ),
        revision: position,
    });
    let born = task.clone();
    write.workflow_head(born, added, move |_| wire::WorkflowHead {
        flow: vec!["todo".parse().expect("fixture phase")],
        phase: "todo".parse().expect("fixture phase"),
        status: "pending".into(),
        review: "unassigned".into(),
        assignment: None,
        deferral: None,
        completion: None,
        generation: 0,
    });
    added
}

/// Bind one live claim to a lane, the state a boundary is recorded for.
///
/// The disposition is the whole point of the seed: `actively_claimed_lanes`
/// decides membership from the decoded head, so an ended claim seeded here
/// would prove nothing about a lane anyone is working.
fn seed_active_claim(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    handle: &str,
) {
    let micros = handle.bytes().fold(0_i64, |value, byte| {
        (value * 131 + i64::from(byte)) % 10_000_000_000_000
    });
    let task = wire::TaskStamp::for_micros(micros).expect("fixture task stamp");
    let mut write =
        write(Actor::Operator, epoch, Producer::CommandDoor).expect("fixture write starts");
    let added = seed_task(&mut write, &task);
    let won = write
        .fact(
            wire::body::ClaimWon {
                task: task.clone(),
                actor: Actor::Operator,
                lane,
                occupant,
                epoch,
                cell: Some(wire::ClaimCell {
                    project: "journal.effort".into(),
                    phase: "todo".parse().expect("fixture phase"),
                }),
                until: "2030-08-01T07:00:00.000Z"
                    .parse()
                    .expect("fixture claim expiry"),
                git: None,
                recovery: None,
                source: wire::body::SourceBoundary::Proven { heads: Vec::new() },
            },
            FactCause::Pending(added),
        )
        .expect("claim fact");
    write.claim_head(task, won, move |fact| wire::ClaimHead {
        actor: Actor::Operator,
        fact,
        claimed: fact,
        cell: wire::ClaimCell {
            project: "journal.effort".into(),
            phase: "todo".parse().expect("fixture phase"),
        },
        lane,
        occupant,
        epoch,
        recovery: None,
        disposition: wire::ClaimDisposition::Active,
        acknowledged: 0,
    });
    commit(bus, write).expect("fixture claim commits");
}

/// Configure one team while moving a member into it.
///
/// The member is what distinguishes this from the configuration case already
/// proven elsewhere: that one states a policy and moves nobody, so its moved
/// list is empty before the boundary filter is ever consulted.
fn configure_member(
    fixture: &Arranged,
    id: CanonicalUuid,
    team: CanonicalUuid,
    member: CanonicalUuid,
) {
    let request = wire::intent::TeamConfigure {
        topology_revision: fixture.head().topology_revision,
        team,
        member: Some(member),
        life: None,
        fast_mode: None,
    };
    let command = Command {
        v: wire::WireVersion::CURRENT,
        id,
        epoch: fixture.epoch,
        kind: CommandKind::TeamConfigure,
        expected: Vec::new(),
        body: serde_json::to_value(&request).expect("team configuration encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch: fixture.epoch,
    })
    .expect("operator team configuration admits");
    let outcome = lane_surface::configure(
        &fixture.bus,
        &fixture.paths,
        fixture.epoch,
        &fixture.seats,
        &command,
        &request,
    );
    let Outcome::Reply(reply) = outcome else {
        panic!("team configuration must commit rather than refuse: {outcome:?}");
    };
    assert!(matches!(reply.position, ReplyPosition::Committed(_)));
}

/// Move one lane through all three membership transitions, in order.
///
/// The lane leaves its team by lane arrangement, is brought back by team
/// configuration, and leaves again by team rearrangement, so every step
/// genuinely relocates it and every step writes its own fact. `stay` is the
/// team-mate that keeps the source team populated, which is what stops a
/// step from also retiring a subscription and changing what the next one moves.
fn transition_sources(
    fixture: &Arranged,
    moved: CanonicalUuid,
    stay: CanonicalUuid,
) -> [Option<serde_json::Value>; 3] {
    let [first_org, second_org] = fixture.teams;
    let anchor = fixture.lanes[2];
    arrange_lane_for_test(
        &fixture.bus,
        &fixture.paths,
        fixture.epoch,
        &fixture.seats,
        test_uuid(41),
        moved,
        anchor,
    );
    let arranged = recorded_sources(&fixture.bus, wire::FactKind::LaneArranged);
    configure_member(fixture, test_uuid(42), first_org, moved);
    let configured = recorded_sources(&fixture.bus, wire::FactKind::TeamConfigured);
    fixture.commit(
        43,
        vec![
            standing(second_org, &[anchor, moved]),
            standing(first_org, &[stay]),
        ],
    );
    let rearranged = recorded_sources(&fixture.bus, wire::FactKind::TeamArranged);
    [arranged, configured, rearranged]
}

/// Every membership transition records the boundary of the claimed lane it moves.
///
/// One test covers all three because the property is one property: the spend a
/// lane accrued before the move belongs to the team it left, and the only
/// thing that lets a reducer split it is a boundary travelling with the fact.
/// A transition that moved a worked lane and recorded nothing would not fail
/// anywhere -- it would silently produce attribution that cannot be divided.
#[test]
fn every_membership_transition_records_the_boundary_of_the_claimed_lane_it_moves() {
    let fixture = Arranged::seat("arrangement-boundary-claimed", &[0, 0, 1]);
    let claimed = fixture.lanes[0];
    let stay = fixture.lanes[1];
    let accounted = test_uuid(60);
    account_transcript(&fixture.bus, fixture.epoch, claimed, accounted);
    seed_active_claim(
        &fixture.bus,
        fixture.epoch,
        claimed,
        fixture.occupants[0],
        "BOUND-claimed1",
    );

    let [arranged, configured, rearranged] = transition_sources(&fixture, claimed, stay);

    let expected = Some(expected_sources(claimed, accounted));
    assert_eq!(
        arranged, expected,
        "a lane arrangement moved a claimed lane without recording its boundary",
    );
    assert_eq!(
        configured, expected,
        "a team configuration moved a claimed lane without recording its boundary",
    );
    assert_eq!(
        rearranged, expected,
        "a team rearrangement moved a claimed lane without recording its boundary",
    );

    fixture.retire();
}

/// A transition that moves no claimed lane records no boundary at all.
///
/// This is what proves the read is a filter rather than an accident of the
/// fixture. A claim is live throughout, on a lane none of the three transitions
/// touches, and the lane they do move has its own accounted transcript -- so
/// the only reason its boundary is absent is that nobody is working it. A
/// boundary for an unworked lane is attribution noise a reducer would have to
/// filter back out, and it would name a team split that never happened.
///
/// Absence here is the omitted key, which is also what a producer that never
/// recorded the field at all would leave behind. That is why this case is not
/// evidence on its own: the test above, over the same three transitions and the
/// same fixture shape, is what shows the key is written when it is earned.
#[test]
fn a_membership_transition_moving_no_claimed_lane_records_no_boundary() {
    let fixture = Arranged::seat("arrangement-boundary-unclaimed", &[0, 0, 1]);
    let claimed = fixture.lanes[0];
    let unclaimed = fixture.lanes[1];
    account_transcript(&fixture.bus, fixture.epoch, claimed, test_uuid(60));
    account_transcript(&fixture.bus, fixture.epoch, unclaimed, test_uuid(61));
    seed_active_claim(
        &fixture.bus,
        fixture.epoch,
        claimed,
        fixture.occupants[0],
        "BOUND-unclaim1",
    );

    let [arranged, configured, rearranged] = transition_sources(&fixture, unclaimed, claimed);

    assert_eq!(
        arranged, None,
        "a lane arrangement recorded a boundary for a lane no claim is bound to",
    );
    assert_eq!(
        configured, None,
        "a team configuration recorded a boundary for a lane no claim is bound to",
    );
    assert_eq!(
        rearranged, None,
        "a team rearrangement recorded a boundary for a lane no claim is bound to",
    );

    fixture.retire();
}
