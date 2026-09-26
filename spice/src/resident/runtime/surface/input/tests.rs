use super::*;
use std::future;

#[tokio::test(flavor = "current_thread")]
async fn continuously_ready_supervisor_and_control_sources_share_every_two_turns() {
    let mut lead = Lead::Supervisor;

    for _ in 0..8 {
        let first = next(&mut lead, future::ready(()), future::ready(())).await;
        let second = next(&mut lead, future::ready(()), future::ready(())).await;

        assert!(matches!(
            (first, second),
            (Input::Supervisor(()), Input::Control(()))
                | (Input::Control(()), Input::Supervisor(()))
        ));
    }
}
