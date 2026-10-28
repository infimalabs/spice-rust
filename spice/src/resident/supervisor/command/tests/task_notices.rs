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
        taste: None,
        completion_contract: "Completion contract: own AC.".to_owned(),
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
        completion_contract: "Completion contract: own AC.".to_owned(),
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
fn an_ended_claim_withdraws_only_its_own_queued_assignment_notice() {
    let sink = Sink::default();
    let driver = driver(&sink);
    let notice = TaskAssignmentNotice {
        occupant: crate::resident::new_uuid(),
        task: "CLAIM-1kKRRsDP".to_owned(),
        claim: 33,
        phase: "plan".to_owned(),
        completion_contract: "Completion contract: own AC.".to_owned(),
    };
    let ended = |claim| TaskAssignmentEnd {
        occupant: notice.occupant,
        task: notice.task.clone(),
        claim,
    };
    driver.deliver_task_assignment(&notice);

    driver.withdraw_task_assignment(&ended(34));
    assert_eq!(
        sink.answered(1).len(),
        1,
        "another claim on the same task leaves the queued assignment alone"
    );
    driver.withdraw_task_assignment(&ended(33));
    let frames = sink.answered(2);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[1]["purpose"], "task:CLAIM-1kKRRsDP");
    assert_eq!(
        frames[1]["message"], "",
        "the ended claim's queued assignment is withdrawn before delivery"
    );
    driver.deliver_task_assignment(&notice);
    assert_eq!(
        sink.answered(2).len(),
        2,
        "a withdrawn assignment is never announced again for the same claim"
    );

    driver.deliver_claim_lapse(&ClaimLapseNotice {
        occupant: notice.occupant,
        task: notice.task.clone(),
        expiry: "2026-10-12T17:25:28.368Z".parse().expect("fixture instant"),
    });
    driver.withdraw_task_assignment(&ended(33));
    let frames = sink.answered(3);
    assert_eq!(
        frames.len(),
        3,
        "a different notice queued under the task's purpose is never withdrawn"
    );
    assert_ne!(frames[2]["message"], "");
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

#[test]
fn taste_hold_notices_are_private_deduplicated_and_keep_a_queued_assignment() {
    let sink = Sink::default();
    let driver = driver(&sink);
    let assignment = TaskAssignmentNotice {
        occupant: crate::resident::new_uuid(),
        task: "CLAIM-1kKRRsDP".to_owned(),
        claim: 33,
        phase: "todo".to_owned(),
        completion_contract: "Completion contract: own AC.".to_owned(),
    };
    let hold = TasteHoldNotice {
        occupant: assignment.occupant,
        task: assignment.task.clone(),
        position: 44,
        matches: vec!["title line 1 taste taskwording".to_owned()],
        completion_contract: "Completion contract: own AC.".to_owned(),
    };

    driver.deliver_task_assignment(&assignment);
    driver.deliver_taste_hold(&hold);
    driver.deliver_taste_hold(&hold);
    let frames = sink.answered(2);
    assert_eq!(frames.len(), 2, "one hold interjects once");
    assert_eq!(frames[1]["type"], "aside", "a hold is a private notice");
    assert_ne!(
        frames[1]["purpose"], frames[0]["purpose"],
        "the hold never replaces the queued assignment for the same task",
    );

    driver.deliver_taste_hold(&TasteHoldNotice {
        position: 45,
        ..hold
    });
    assert_eq!(
        sink.answered(3).len(),
        3,
        "a later write that leaves a fresh hold is a distinct notice",
    );
}

#[test]
fn changed_completion_authority_replaces_notices_without_a_new_claim_or_generation() {
    let sink = Sink::default();
    let assignment_driver = driver(&sink);
    let mut assignment = TaskAssignmentNotice {
        occupant: crate::resident::new_uuid(),
        task: "CLAIM-1kKRRsDP".to_owned(),
        claim: 33,
        phase: "todo".to_owned(),
        completion_contract: "own AC".to_owned(),
    };
    assignment_driver.deliver_task_assignment(&assignment);
    assignment.completion_contract = "unspecified".to_owned();
    assignment_driver.deliver_task_assignment(&assignment);
    assignment_driver.deliver_task_assignment(&assignment);
    let frames = sink.answered(2);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["purpose"], frames[1]["purpose"]);
    assert_ne!(frames[0]["message"], frames[1]["message"]);

    let sink = Sink::default();
    let driver = driver(&sink);
    let mut generation = TaskGenerationNotice {
        occupant: assignment.occupant,
        task: assignment.task,
        claim: assignment.claim,
        generation: 4,
        change: "task edited".to_owned(),
        editor: "operator".to_owned(),
        position: 44,
        taste: None,
        completion_contract: "own AC".to_owned(),
    };
    driver.deliver_task_generation(&generation);
    generation.completion_contract = "unspecified".to_owned();
    driver.deliver_task_generation(&generation);
    driver.deliver_task_generation(&generation);
    let frames = sink.answered(2);
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["purpose"], frames[1]["purpose"]);
    assert_ne!(frames[0]["message"], frames[1]["message"]);
}
