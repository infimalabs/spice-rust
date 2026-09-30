use super::*;
use app::runtime::{MemberCommand, MemberControlState};

fn pending(runtime: &CanvasRuntime, lane: wire::CanonicalUuid) -> Option<&MemberCommand> {
    runtime
        .member_control(lane)
        .and_then(MemberControlState::pending)
}

fn issue(runtime: &CanvasRuntime, lane: wire::CanonicalUuid) -> Option<&str> {
    runtime
        .member_control(lane)
        .and_then(MemberControlState::issue)
}

#[test]
fn member_commands_settle_only_the_exact_reservation_they_answer() {
    let session = session(OPERATOR, EPOCH);
    let accepted = acceptance(&session, "operator", EPOCH);
    let mut runtime = CanvasRuntime::new(&accepted);
    let lane = uuid(THIRD_LANE);

    let drain = runtime
        .begin_member_command(lane, MemberCommand::Drain)
        .expect("an idle member reserves a command");
    assert!(
        runtime
            .begin_member_command(lane, MemberCommand::Remove)
            .is_none(),
        "a member runs one command at a time"
    );
    assert_eq!(pending(&runtime, lane), Some(&MemberCommand::Drain));

    runtime.settle_member_command(&drain, Some("Could not drain this agent: busy.".to_owned()));
    assert_eq!(pending(&runtime, lane), None);
    assert_eq!(
        issue(&runtime, lane),
        Some("Could not drain this agent: busy.")
    );

    let retry = runtime
        .begin_member_command(lane, MemberCommand::Drain)
        .expect("a refused member can retry");
    assert_eq!(
        issue(&runtime, lane),
        None,
        "a new command clears the refusal"
    );
    runtime.settle_member_command(&drain, Some("late".to_owned()));
    assert_eq!(
        pending(&runtime, lane),
        Some(&MemberCommand::Drain),
        "a late answer to an earlier drain cannot settle its retry"
    );
    assert_eq!(issue(&runtime, lane), None);

    assert!(!runtime.reconnect(&accepted));
    assert_eq!(
        pending(&runtime, lane),
        None,
        "a reconnect abandons the command"
    );
    runtime.settle_member_command(&retry, Some("after reconnect".to_owned()));
    assert_eq!(
        issue(&runtime, lane),
        None,
        "an abandoned command's answer no longer lands on the member"
    );
    let wake = runtime
        .begin_member_command(lane, MemberCommand::Wake)
        .expect("the member is free again after reconnect");
    runtime.settle_member_command(&wake, None);
    assert_eq!(
        runtime.member_control(lane),
        Some(&MemberControlState::default())
    );
}
