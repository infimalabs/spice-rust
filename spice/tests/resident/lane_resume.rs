//! The operator-supplied session identity, from the seat request to the vendor.
//!
//! Seating a lane on an identity this journal has never seen is how an agent
//! already running under another system is adopted rather than replaced, so the
//! operand has to survive the whole way down: `lane add --resume` carries it
//! over the wire, `prepare_seat` turns it into a `vendor::Resume::Session`, and
//! the launch spends it on the vendor's own resume flag. Every step between is
//! shared with the fresh path and proves nothing about this one, which is why
//! the fixture vendor here reports which of the two it was actually given.

use super::lane_process::{SESSION_ID, configure_fake_vendor, configure_successor_worktree};
use super::*;

/// Exit code a fault takes, spelled where the case can read it.
const FAULT_EXIT: i32 = 1;

/// Which launch modes the fixture vendor has recorded, oldest first.
///
/// The vendor appends one line per launch, so the file is the launch history
/// rather than the last launch, and a case that reads only the tail cannot tell
/// a seat that never ran from one that ran the other way.
fn launches(fixture: &Fixture) -> Vec<String> {
    let path = fixture.root.join("vendor-launches");
    if !path.exists() {
        return Vec::new();
    }
    fs::read_to_string(path)
        .expect("fixture vendor launch modes read")
        .lines()
        .map(str::to_owned)
        .collect()
}

fn reply_body<T: serde::de::DeserializeOwned>(output: &Output) -> T {
    assert!(
        output.status.success(),
        "command failed: stdout={:?} stderr={:?}",
        output.stdout,
        output.stderr
    );
    let Outcome::Reply(reply) = value_outcome(output) else {
        panic!("successful command must reply");
    };
    let bytes = wire::encode(&reply.body).expect("reply value encodes");
    wire::decode(&bytes).expect("reply body decodes as its selected DTO")
}

fn add_lane(fixture: &Fixture, tail: &[&str]) -> wire::intent::LaneChangeAnswer {
    let mut arguments = vec!["lane", "add", "--life", "Drive", "--vendor", "claude"];
    arguments.extend_from_slice(tail);
    reply_body(&invoke_args(fixture, &arguments, true))
}

/// A seat carries the operator's session to the vendor, and a bare seat does not.
///
/// Both halves run against one fixture vendor, because a case that only ever
/// asks for a resume cannot distinguish the mapping under test from a vendor
/// script that says `resume` whatever it is handed. The bare seat is that
/// control, and it takes a second worktree because a drained lane keeps the one
/// it was seated on.
#[test]
fn an_operator_supplied_session_reaches_the_vendor_and_a_bare_seat_stays_fresh() {
    let fixture = Fixture::new("lane-resume");
    configure_fake_vendor(&fixture);
    let successor = configure_successor_worktree(&fixture);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);

    let resumed = add_lane(&fixture, &["--resume", SESSION_ID]);
    assert_eq!(
        launches(&fixture),
        ["resume"],
        "the operator's session never reached the vendor"
    );

    let successor = successor.display().to_string();
    let fresh = add_lane(&fixture, &[&successor]);
    assert_ne!(fresh.lane, resumed.lane, "the control seat reused the lane");
    assert_eq!(
        launches(&fixture),
        ["resume", "fresh"],
        "the fixture vendor reports one launch mode whatever it is given"
    );

    for lane in [resumed.lane, fresh.lane] {
        let drained = invoke_args(&fixture, &["lane", "drain", &lane.to_string()], true);
        assert!(
            drained.status.success(),
            "drain failed: {:?}",
            drained.stderr
        );
    }
    stop(&fixture, resident);
}

/// Stop a resident that has already released every occupant it started.
///
/// A live vendor holds the resident past the client's own answer bound, so a
/// case that stops without draining reports a deadline rather than whatever it
/// was testing.
fn stop(fixture: &Fixture, resident: RunningResident) {
    let down = invoke(fixture, "down");
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

/// An unusable session operand is refused before anything is seated or launched.
///
/// Whitespace is the operand that reaches furthest: it survives the shell, the
/// parser, and the wire, and only the vendor's own identity rule rejects it. The
/// launch history is read afterwards because a fault that still started a vendor
/// would leave a process behind that no lane row admits to owning.
#[test]
fn a_blank_session_operand_faults_before_any_lane_is_seated() {
    let fixture = Fixture::new("lane-resume-blank");
    configure_fake_vendor(&fixture);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);

    let refused = invoke_args(
        &fixture,
        &[
            "lane", "add", "--life", "Drive", "--vendor", "claude", "--resume", "   ",
        ],
        true,
    );
    assert_eq!(refused.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = value_outcome(&refused) else {
        panic!("a blank session operand must fault: {:?}", refused.stdout);
    };
    assert_eq!(fault.reason, "lane::resume-invalid");
    assert!(
        fault.detail.contains("must not be empty"),
        "the fault kept the vendor's own reason: {:?}",
        fault.detail
    );

    let lanes = reply_body::<wire::intent::LaneAnswer>(&invoke_args(&fixture, &["lane"], true));
    assert!(lanes.lanes.is_empty(), "a faulted seat left a lane row");
    assert!(launches(&fixture).is_empty(), "a faulted seat launched");

    stop(&fixture, resident);
}
