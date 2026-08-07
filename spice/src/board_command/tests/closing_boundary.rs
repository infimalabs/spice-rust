// A closing boundary is written by the transaction that advances the phase,
// and until these tests existed nothing read it back: recording the migrated
// unrecorded value at both sites left the whole package green. The two facts
// are pinned separately on purpose, so severing one site cannot be masked by
// the assertion covering the other.

/// The boundary a closing fact recorded, exactly as it reached the journal.
///
/// This reads the stored body rather than a decoded value, because the
/// distinction under test is a wire one: a proven boundary is an explicit key,
/// while the migrated unrecorded value is skipped and leaves no key at all.
/// Decoding first would turn that absence back into a value and hide it.
fn closing_boundary(bus: &Bus, kind: wire::FactKind) -> Option<serde_json::Value> {
    let facts = bus.fact_snapshot().expect("closing boundary facts read");
    let closing = facts
        .facts()
        .iter()
        .find(|fact| fact.fact.kind() == kind)
        .expect("the closing fact is durable");
    closing.fact.body().get("source").cloned()
}

/// What a lane with no accounted transcript proves at a closing boundary.
///
/// Empty heads is a real answer -- the lane opens at position zero -- and is
/// not the same answer as no boundary. Asserting this shape rather than mere
/// presence is what separates the two states the enum exists to keep apart.
fn proven_at_zero() -> serde_json::Value {
    serde_json::json!({ "proven": { "heads": [] } })
}

/// Pins the closing boundary on `TaskDone` at `progress.rs:270`.
#[test]
fn a_completed_task_records_an_explicit_proven_closing_boundary() {
    let (_root, bus, epoch, seats) = fixture("closing-boundary-done", 1);
    let lane = Actor::Lane(seats[0].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "CLOSE-done1",
        "Complete once",
        None,
    );
    claim_and_complete_task(&bus, epoch, &seats[0], &lane, &task);

    assert_eq!(
        closing_boundary(&bus, wire::FactKind::TaskDone),
        Some(proven_at_zero()),
        "completing a task must record the boundary it closed at; the migrated \
         unrecorded value is skipped on the wire, so a missing key here means \
         the closing boundary was never written"
    );
}

/// Pins the closing boundary on `TaskReviewed` at `progress.rs:571`.
///
/// The review is issued by a second lane, because a review routes to a peer,
/// and the boundary recorded is the reviewing lane's own.
#[test]
fn a_completed_review_records_an_explicit_proven_closing_boundary() {
    let (_root, bus, epoch, seats) = fixture("closing-boundary-review", 2);
    let lane_one = Actor::Lane(seats[0].lane);
    let lane_two = Actor::Lane(seats[1].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "CLOSE-rev1",
        "Complete and review",
        None,
    );
    advance_task_to_review(&bus, epoch, &seats[0], &lane_one, &task);
    complete_review_with_finding(&bus, epoch, &seats[1], &lane_two, &task);

    assert_eq!(
        closing_boundary(&bus, wire::FactKind::TaskReviewed),
        Some(proven_at_zero()),
        "completing a review must record the boundary it closed at, and must \
         do so independently of the boundary the completion before it recorded"
    );
}
