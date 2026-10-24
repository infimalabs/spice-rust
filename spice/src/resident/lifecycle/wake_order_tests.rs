use super::tests::{ROTATE, instant, lane, target};
use super::*;
#[test]
fn due_lanes_wake_in_the_configured_claim_order() {
    let lane_at = |lane: u8, claim: Option<u64>, available| Target {
        last_claim: claim.map(|seq| Seq::new(seq).expect("claim position")),
        available,
        ..target(lane)
    };
    let ready = |lane, claim| lane_at(lane, Some(claim), Available::Ready);
    let chosen =
        |order, targets: &[Target]| match resolve(targets, &Backoff::default(), instant(10), order)
        {
            Resolution::Attempt { trigger, lane } | Resolution::Wait { trigger, lane, .. } => {
                (trigger, lane)
            }
            Resolution::Idle => panic!("a due lane resolves"),
        };
    let work = |n| (Trigger::AvailableWork, lane(n));
    let held = |claim| Target {
        held_claim: Available::Ready,
        ..ready(5, claim)
    };
    let later = |lane, claim| lane_at(lane, Some(claim), Available::At(instant(20)));
    let never = lane_at(4, None, Available::Ready);

    // Rotation: older claims wake first whatever the lane ids or input
    // order, and once the woken lane claims, the next wake moves on.
    let rotate = |targets: &[Target]| chosen(ROTATE, targets);
    assert_eq!(rotate(&[ready(1, 90), ready(3, 40), ready(2, 70)]), work(3));
    assert_eq!(rotate(&[ready(1, 90), ready(3, 91), ready(2, 70)]), work(2));
    // A never-claimed lane outranks every claimed one; exact ties fall to lane id.
    assert_eq!(rotate(&[ready(1, 5), never]), work(4));
    assert_eq!(rotate(&[ready(2, 7), ready(1, 7)]), work(1));
    // Rank never overrides trigger precedence.
    assert_eq!(
        rotate(&[ready(1, 1), held(99)]),
        (Trigger::HeldClaim, lane(5))
    );
    // Equal deadlines wait on the same least recently claimed lane.
    assert_eq!(rotate(&[later(1, 50), later(2, 30)]), work(2));

    // Concentration: the newest claim wakes first and keeps waking while
    // it keeps claiming.
    let warm = |targets: &[Target]| chosen(lane::WakeOrder::MostRecentlyClaimed, targets);
    assert_eq!(warm(&[ready(1, 90), ready(3, 40), ready(2, 70)]), work(1));
    assert_eq!(warm(&[ready(1, 92), ready(3, 91), ready(2, 70)]), work(1));
    // A never-claimed lane comes last; exact ties still fall to lane id.
    assert_eq!(warm(&[ready(1, 5), never]), work(1));
    assert_eq!(warm(&[never, lane_at(3, None, Available::Ready)]), work(3));
    assert_eq!(warm(&[ready(2, 7), ready(1, 7)]), work(1));
    // Rank never overrides trigger precedence.
    assert_eq!(
        warm(&[ready(1, 99), held(1)]),
        (Trigger::HeldClaim, lane(5))
    );
    // Equal deadlines wait on the same most recently claimed lane.
    assert_eq!(warm(&[later(1, 50), later(2, 30)]), work(1));
}

#[test]
fn best_match_wakes_the_closest_ordinary_lane_then_the_most_recent_claim() {
    let lane_at = |lane: u8, claim: u64, matched: Option<f64>| Target {
        last_claim: Some(Seq::new(claim).expect("claim position")),
        available: Available::Ready,
        matched,
        ..target(lane)
    };
    let best = |targets: &[Target]| match resolve(
        targets,
        &Backoff::default(),
        instant(10),
        lane::WakeOrder::BestMatch,
    ) {
        Resolution::Attempt { trigger, lane } => (trigger, lane),
        other => panic!("a ready lane attempts: {other:?}"),
    };
    let work = |n| (Trigger::AvailableWork, lane(n));

    // The higher match wins over a more recent claim and a lower id.
    assert_eq!(
        best(&[lane_at(1, 90, Some(0.2)), lane_at(2, 10, Some(0.6))]),
        work(2)
    );
    // Any score outranks no score, even a zero one.
    assert_eq!(
        best(&[lane_at(1, 90, None), lane_at(2, 10, Some(0.0))]),
        work(2)
    );
    // An ordinary lane outranks a better-matched lane's own last resort.
    let resort = Target {
        last_resort: true,
        ..lane_at(1, 90, Some(0.9))
    };
    assert_eq!(best(&[resort, lane_at(2, 10, None)]), work(2));
    // Equal or absent matches fall to the newest claim, then lane id.
    assert_eq!(
        best(&[lane_at(1, 10, Some(0.5)), lane_at(2, 90, Some(0.5))]),
        work(2)
    );
    assert_eq!(best(&[lane_at(1, 10, None), lane_at(2, 90, None)]), work(2));
    assert_eq!(best(&[lane_at(2, 7, None), lane_at(1, 7, None)]), work(1));
    // A match never overrides trigger precedence.
    let held = Target {
        held_claim: Available::Ready,
        ..lane_at(3, 1, None)
    };
    assert_eq!(
        best(&[lane_at(1, 90, Some(0.9)), held]),
        (Trigger::HeldClaim, lane(3))
    );
}

#[test]
fn ordinary_work_wakes_before_a_lanes_own_last_resort() {
    let chosen =
        |order, targets: &[Target]| match resolve(targets, &Backoff::default(), instant(10), order)
        {
            Resolution::Attempt { trigger, lane } | Resolution::Wait { trigger, lane, .. } => {
                (trigger, lane)
            }
            Resolution::Idle => panic!("a due lane resolves"),
        };
    let due = |lane: u8, claim: u64, last_resort, available| Target {
        last_claim: Some(Seq::new(claim).expect("claim position")),
        available,
        last_resort,
        ..target(lane)
    };
    // The author's claim is the newest and the peer's is old, so neither
    // claim order alone keeps the author last; the class decides first.
    let author = due(1, 90, true, Available::Ready);
    let peer = due(2, 10, false, Available::Ready);
    let fresh = Target {
        last_claim: None,
        ..peer
    };
    for order in [
        ROTATE,
        lane::WakeOrder::MostRecentlyClaimed,
        lane::WakeOrder::BestMatch,
    ] {
        let pick = |targets: &[Target]| chosen(order, targets);
        assert_eq!(pick(&[author, peer]), (Trigger::AvailableWork, lane(2)));
        assert_eq!(pick(&[author, fresh]), (Trigger::AvailableWork, lane(2)));
        // The class ranks drain-queue lanes the same way.
        let drain = |target: Target<'static>| Target {
            drain_queue: target.available,
            available: Available::None,
            ..target
        };
        assert_eq!(
            pick(&[drain(author), drain(peer)]),
            (Trigger::DrainQueue, lane(2))
        );
        // A peerless author still wakes for its last resort.
        assert_eq!(pick(&[author]), (Trigger::AvailableWork, lane(1)));
        // Within one class the configured order still decides.
        let other_author = due(3, 50, true, Available::Ready);
        let warm_author = if order == ROTATE { 3 } else { 1 };
        assert_eq!(
            pick(&[author, other_author]),
            (Trigger::AvailableWork, lane(warm_author))
        );
        // The class never overrides trigger precedence, and only work
        // triggers carry it.
        let holding = Target {
            held_claim: Available::Ready,
            ..author
        };
        assert_eq!(pick(&[peer, holding]), (Trigger::HeldClaim, lane(1)));
        let holding_peer = Target {
            lane: lane(4),
            last_claim: Some(Seq::new(10).expect("claim position")),
            last_resort: false,
            ..holding
        };
        assert_eq!(
            pick(&[holding, holding_peer]),
            (
                Trigger::HeldClaim,
                if order == ROTATE { lane(4) } else { lane(1) }
            )
        );
        // A due last resort still wakes before a peer whose work is not yet due.
        let pending_peer = due(2, 10, false, Available::At(instant(20)));
        assert_eq!(
            pick(&[author, pending_peer]),
            (Trigger::AvailableWork, lane(1))
        );
        // Equal wait deadlines name the ordinary lane.
        let later = |target: Target<'static>| Target {
            available: Available::At(instant(20)),
            ..target
        };
        assert_eq!(
            pick(&[later(author), later(peer)]),
            (Trigger::AvailableWork, lane(2))
        );
    }
}
