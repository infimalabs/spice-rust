//! Operator-authorized automated room resumes, issued after each commit.

use super::*;

/// Issue every automated room resume the latest commit made ready and wake
/// each resumed target's inbox. A refused resume changes nothing and is
/// re-evaluated on the next commit; a fault is reported and never fails the
/// caller.
pub(in crate::resident) fn reconcile(
    bus: &Bus,
    paths: &RuntimePaths,
    config: &SurfaceConfig,
    diagnostic: &mut dyn io::Write,
) {
    let resumes = match crate::board_command::room_resumes_by_rule(
        bus,
        &config.steering,
        &config.projects,
        &config.allocation,
    ) {
        Ok(resumes) => resumes,
        Err(outcome) => {
            let _ = writeln!(
                diagnostic,
                "automated room resumes were not read: {outcome:?}"
            );
            return;
        }
    };
    for (room, resumed) in resumes {
        match resumed {
            Ok(Some(lane)) => steering::notify_inbox(paths, lane),
            Ok(None) => {}
            Err(outcome) => {
                let _ = writeln!(
                    diagnostic,
                    "automated resume of room {room} was not recorded: {outcome:?}"
                );
            }
        }
    }
}
