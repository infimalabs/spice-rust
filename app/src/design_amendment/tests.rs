use wire::intent::BoardTask;
use wire::rooms::{DesignNoticeProvenance, DesignNoticed};
use wire::{CanonicalUuid, SequenceCode, Witness};

use super::{DesignAmendment, DraftNotice, DraftPhase, visible};
use crate::work::tests::task;

fn design() -> BoardTask {
    let mut design = task(1, "rooms.design");
    design.presentation_title = "Current title".to_owned();
    design.statement.body.description = Some("Current description".to_owned());
    design.statement.body.intent = Some(wire::TaskIntent::Design);
    design.workflow.body.flow = vec!["design".parse().expect("fixture phase")];
    design.workflow.body.phase = "design".parse().expect("fixture phase");
    design
}

fn uuid(suffix: u32) -> CanonicalUuid {
    format!("11111111-1111-4111-8111-{suffix:012}")
        .parse()
        .expect("fixture UUID")
}

fn notice(design: &BoardTask, lane: CanonicalUuid, occupant: CanonicalUuid) -> DesignNoticed {
    DesignNoticed {
        statement: design.statement.position,
        provenance: DesignNoticeProvenance::Organic,
        design: design.statement.body.born.clone(),
        task: design.statement.body.born.clone(),
        claim: SequenceCode::new(1),
        terms: Default::default(),
        lane,
        occupant,
    }
}

fn refusal(draft: &DesignAmendment) -> &str {
    match draft.notice() {
        Some(DraftNotice::Refused(reason)) => reason.split(": ").next().unwrap_or(reason),
        other => panic!("expected refusal, got {other:?}"),
    }
}

#[test]
fn preview_freezes_the_shown_head_and_omits_unchanged_fields() {
    let design = design();
    let mut draft = DesignAmendment::new(&design);
    draft.preview(&design);
    assert_eq!(refusal(&draft), "task::edit-empty");
    assert_eq!(draft.phase(), &DraftPhase::Editing);

    draft.set_description("  Revised description  ".to_owned());
    draft.preview(&design);
    let DraftPhase::Previewing(submission) = draft.phase() else {
        panic!("changed description previews: {:?}", draft.phase());
    };
    assert_eq!(submission.request.title, None);
    assert_eq!(
        submission.request.description.as_deref(),
        Some("Revised description")
    );
    assert_eq!(submission.request.acceptance, Vec::<String>::new());
    assert_eq!(
        submission.expected,
        [Witness::Task {
            task: design.statement.body.born.clone(),
            head: design.statement.position,
        }]
    );

    draft.revise();
    draft.set_description("Current description".to_owned());
    draft.set_title("  Revised title  ".to_owned());
    draft.preview(&design);
    let DraftPhase::Previewing(submission) = draft.phase() else {
        panic!("changed title previews: {:?}", draft.phase());
    };
    assert_eq!(submission.request.title.as_deref(), Some("Revised title"));
    assert_eq!(submission.request.description, None);
}

#[test]
fn a_refusal_keeps_both_fields_and_resubmission_reads_the_new_head() {
    let design = design();
    let mut draft = DesignAmendment::new(&design);
    draft.set_title("  Proposed title  ".to_owned());
    draft.set_description("  Proposed description  ".to_owned());
    draft.preview(&design);
    assert!(draft.begin_submit().is_some());
    assert!(draft.is_pending());
    draft.set_title("lost while pending".to_owned());
    assert_eq!(
        draft.title(),
        "  Proposed title  ",
        "pending input is frozen"
    );

    draft.settle(Err(
        "board::witness-moved: the statement changed first".to_owned()
    ));
    assert_eq!(refusal(&draft), "board::witness-moved");
    assert_eq!(draft.title(), "  Proposed title  ");
    assert_eq!(draft.description(), "  Proposed description  ");
    assert!(
        draft.begin_submit().is_none(),
        "a refusal needs a new preview"
    );

    let mut moved = design.clone();
    moved.statement.position =
        wire::Seq::new(design.statement.position.get() + 10).expect("moved position");
    moved.presentation_title = "Competing title".to_owned();
    draft.preview(&moved);
    let second = draft.begin_submit().expect("retained fields resubmit");
    assert_eq!(
        second.expected,
        [Witness::Task {
            task: moved.statement.body.born.clone(),
            head: moved.statement.position,
        }]
    );
    assert_eq!(second.request.title.as_deref(), Some("Proposed title"));
    assert_eq!(
        second.request.description.as_deref(),
        Some("Proposed description")
    );
}

#[test]
fn visibility_requires_the_noticed_current_occupant_and_an_open_design() {
    let design = design();
    let lane = uuid(1);
    let occupant = uuid(2);
    let notices = [notice(&design, lane, occupant)];
    assert!(visible(&design, Some((lane, occupant)), &notices));
    assert!(
        !visible(&design, None, &notices),
        "an operator has no lane occupant identity"
    );
    assert!(!visible(&design, Some((lane, uuid(3))), &notices));
    assert!(!visible(&design, Some((uuid(4), occupant)), &notices));
    assert!(!visible(&design, Some((lane, occupant)), &[]));

    let mut settled = design.clone();
    settled.workflow.body.status = "completed".to_owned();
    settled.workflow.body.review = "clean".to_owned();
    settled.workflow.body.completion = Some(
        "2026-10-15T07:00:00.000Z"
            .parse()
            .expect("fixture completion"),
    );
    assert!(!visible(&settled, Some((lane, occupant)), &notices));
}

#[test]
fn success_rebases_to_the_submitted_values() {
    let design = design();
    let mut draft = DesignAmendment::new(&design);
    draft.set_title("  Revised title  ".to_owned());
    draft.set_description("  Revised description  ".to_owned());
    draft.preview(&design);
    assert!(draft.begin_submit().is_some());
    draft.settle(Ok(()));

    assert_eq!(draft.phase(), &DraftPhase::Editing);
    assert_eq!(draft.title(), "Revised title");
    assert_eq!(draft.description(), "Revised description");
    assert_eq!(draft.notice(), Some(&DraftNotice::Saved));
    draft.settle(Err("late".to_owned()));
    assert_eq!(
        draft.notice(),
        Some(&DraftNotice::Saved),
        "only a pending edit settles"
    );
}
