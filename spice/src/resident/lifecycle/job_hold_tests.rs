//! A vacant lane's held claim waiting on its own deadline-bounded jobs.

use super::tests::{instant, lane};
use super::*;

/// A vacant lane with a settled held claim and other work it could take.
fn vacant_holder() -> Target<'static> {
    Target {
        lane: lane(1),
        last_claim: None,
        pending_inbox: false,
        held_claim: Available::At(instant(3)),
        drain_queue: Available::Ready,
        available: Available::Ready,
        last_resort: false,
        task: None,
        matched: None,
    }
}

fn held(targets: &[Target], now: Instant) -> Resolution {
    resolve(targets, &Backoff::default(), now)
}

#[test]
fn an_unfinished_regular_job_holds_every_seat_but_operator_steering_until_its_expiry() {
    let delivered = instant(40);
    let mut targets = [vacant_holder()];
    await_regular_jobs(&mut targets, instant(10), |_| Some(delivered));
    assert_eq!(targets[0].held_claim, Available::At(delivered));
    assert_eq!(targets[0].drain_queue, Available::None);
    assert_eq!(targets[0].available, Available::None);
    assert_eq!(
        held(&targets, instant(10)),
        Resolution::Wait {
            trigger: Trigger::HeldClaim,
            lane: lane(1),
            deadline: delivered,
        },
        "the job's admitted expiry is the one scheduled seat"
    );

    targets[0].pending_inbox = true;
    assert_eq!(
        held(&targets, instant(10)),
        Resolution::Attempt {
            trigger: Trigger::PendingInbox,
            lane: lane(1),
        },
        "operator steering is never held behind a job"
    );

    // The admitted expiry arrives with no keeper notification.
    let mut targets = [vacant_holder()];
    await_regular_jobs(&mut targets, delivered, |_| Some(delivered));
    assert_eq!(
        held(&targets, delivered),
        Resolution::Attempt {
            trigger: Trigger::HeldClaim,
            lane: lane(1),
        }
    );
}

#[test]
fn a_finished_job_or_no_regular_job_reseats_the_held_claim_at_once() {
    // The keeper published the ending, or only services and ended jobs remain.
    let mut targets = [vacant_holder()];
    await_regular_jobs(&mut targets, instant(10), |_| None);
    assert_eq!(targets, [vacant_holder()]);
    assert_eq!(
        held(&targets, instant(10)),
        Resolution::Attempt {
            trigger: Trigger::HeldClaim,
            lane: lane(1),
        }
    );
}

#[test]
fn a_job_never_holds_a_lane_without_a_live_held_claim() {
    let mut free = vacant_holder();
    free.held_claim = Available::None;
    let mut targets = [free];
    await_regular_jobs(&mut targets, instant(10), |_| Some(instant(40)));
    assert_eq!(
        held(&targets, instant(10)),
        Resolution::Attempt {
            trigger: Trigger::DrainQueue,
            lane: lane(1),
        }
    );
}
