use super::*;

#[test]
fn task_generation_reminders_are_private_and_deduplicated_by_authority() {
    let sink = Sink::default();
    let driver = driver(&sink);
    let notice = TaskGenerationNotice {
        occupant: crate::resident::new_uuid(),
        task: "CLAIM-1kKRRsDP".to_owned(),
        claim: 33,
        generation: 4,
        change: "task edited".to_owned(),
        editor: "operator".to_owned(),
        position: 44,
    };

    driver.deliver_task_generation(&notice);
    driver.deliver_task_generation(&notice);
    let frames = sink.answered(1);
    assert_eq!(frames.len(), 1, "one authority identity interjects once");
    assert_eq!(frames[0]["type"], "aside", "a reminder is a private notice");

    driver.deliver_task_generation(&TaskGenerationNotice {
        generation: 5,
        ..notice.clone()
    });
    driver.deliver_task_generation(&notice);
    assert_eq!(
        sink.answered(2)
            .iter()
            .filter(|frame| frame["type"] == "aside")
            .count(),
        2,
        "a later generation replaces the queued reminder and suppresses older replay",
    );
}

#[test]
fn task_assignment_notices_are_private_and_deduplicated_by_claim() {
    let sink = Sink::default();
    let driver = driver(&sink);
    let notice = TaskAssignmentNotice {
        occupant: crate::resident::new_uuid(),
        task: "CLAIM-1kKRRsDP".to_owned(),
        claim: 33,
        phase: "todo".to_owned(),
    };

    driver.deliver_task_assignment(&notice);
    driver.deliver_task_assignment(&notice);
    let frames = sink.answered(1);
    assert_eq!(frames.len(), 1, "one claim identity interjects once");
    assert_eq!(
        frames[0]["type"], "aside",
        "an assignment is a private notice"
    );

    driver.deliver_task_assignment(&TaskAssignmentNotice {
        task: "CLAIM-1kKRRsDQ".to_owned(),
        claim: 34,
        ..notice
    });
    assert_eq!(
        sink.answered(2)
            .iter()
            .filter(|frame| frame["type"] == "aside")
            .count(),
        2,
        "a later automatic claim produces a distinct assignment notice",
    );
}

#[test]
fn claim_lapse_notices_are_private_and_deduplicated_by_lapse() {
    let sink = Sink::default();
    let driver = driver(&sink);
    let notice = ClaimLapseNotice {
        occupant: crate::resident::new_uuid(),
        task: "CLAIM-1kKRRsDP".to_owned(),
        expiry: "2026-10-12T17:25:28.368Z".parse().expect("fixture instant"),
    };

    driver.deliver_claim_lapse(&notice);
    driver.deliver_claim_lapse(&notice);
    let frames = sink.answered(1);
    assert_eq!(frames.len(), 1, "one lapse interjects once");
    assert_eq!(frames[0]["type"], "aside", "a lapse is a private notice");

    driver.deliver_claim_lapse(&ClaimLapseNotice {
        expiry: "2026-10-12T18:25:28.368Z".parse().expect("fixture instant"),
        ..notice
    });
    assert_eq!(
        sink.answered(2)
            .iter()
            .filter(|frame| frame["type"] == "aside")
            .count(),
        2,
        "a later lapse of the same task is a distinct notice",
    );
}
