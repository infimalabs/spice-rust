use wire::intent::{BoardTask, RoomContributeAnswer};
use wire::rooms::{ROOM_CONTRIBUTION_SOURCE_LIMIT, RoomContributionKind, RoomSource};
use wire::{SequenceCode, Witness};

use super::{ContributionDraft, DraftNotice, DraftPhase};
use crate::work::tests::{stamp, task};

fn design() -> BoardTask {
    let mut design = task(1, "rooms.design");
    design.statement.body.intent = Some(wire::TaskIntent::Design);
    design.workflow.body.flow = vec!["design".parse().expect("fixture phase")];
    design.workflow.body.phase = "design".parse().expect("fixture phase");
    design
}

fn concluded() -> BoardTask {
    let mut design = design();
    design.workflow.body.status = "completed".to_owned();
    design.workflow.body.review = "clean".to_owned();
    design.workflow.body.completion = Some(
        "2026-09-23T06:00:00.000Z"
            .parse()
            .expect("fixture completion"),
    );
    design
}

fn refusal(draft: &ContributionDraft) -> &str {
    match draft.notice() {
        Some(DraftNotice::Refused(reason)) => reason.split(": ").next().unwrap_or(reason),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn digest() -> String {
    "ab".repeat(32)
}

fn ready() -> ContributionDraft {
    let mut draft = ContributionDraft::new(stamp(1));
    draft.set_kind(RoomContributionKind::Evidence);
    draft.set_text("  Measured under load.  ".to_owned());
    draft
}

#[test]
fn a_preview_needs_a_kind_nonempty_text_and_an_open_design() {
    let mut draft = ContributionDraft::new(stamp(1));
    draft.preview(&design());
    assert_eq!(refusal(&draft), "room::contribution-kind-missing");

    draft.set_kind(RoomContributionKind::Question);
    draft.set_text(" \n ".to_owned());
    draft.preview(&design());
    assert_eq!(refusal(&draft), "room::contribution-empty");
    assert_eq!(draft.phase(), &DraftPhase::Editing);

    let mut draft = ready();
    draft.preview(&concluded());
    assert_eq!(
        refusal(&draft),
        "room::design-not-open",
        "a concluded Design takes no contribution"
    );
    assert_eq!(draft.text(), "  Measured under load.  ", "the draft stays");

    let mut other = design();
    other.statement.body.born = stamp(2);
    draft.preview(&other);
    assert_eq!(refusal(&draft), "room::contribution-design-mismatch");

    draft.preview(&design());
    let DraftPhase::Previewing(submission) = draft.phase() else {
        panic!("an admissible draft previews: {:?}", draft.phase());
    };
    assert_eq!(submission.request.text, "Measured under load.");
    assert_eq!(submission.request.kind, RoomContributionKind::Evidence);
    assert_eq!(submission.request.room, design().statement.body.handle);
    assert!(matches!(
        submission.expected.as_slice(),
        [Witness::Task { .. }, Witness::Workflow { .. }]
    ));

    draft.revise();
    assert_eq!(
        draft.phase(),
        &DraftPhase::Editing,
        "cancel retracts the preview"
    );
    assert!(
        draft.begin_submit().is_none(),
        "a retracted preview submits nothing"
    );
}

#[test]
fn a_previewed_contribution_submits_once_and_a_refusal_keeps_the_draft() {
    let mut draft = ready();
    assert!(
        draft.begin_submit().is_none(),
        "nothing submits unpreviewed"
    );
    draft.preview(&design());
    let submitted = draft.begin_submit().expect("the preview submits");
    assert!(draft.is_pending());
    assert!(
        draft.begin_submit().is_none(),
        "a pending submission cannot double-post"
    );
    draft.set_text("Edited while pending".to_owned());
    draft.preview(&design());
    draft.revise();
    assert_eq!(
        draft.phase(),
        &DraftPhase::Pending(submitted.clone()),
        "a pending draft is frozen"
    );

    draft.settle(Err("room::design-not-open: the Design concluded".to_owned()));
    assert_eq!(refusal(&draft), "room::design-not-open");
    assert_eq!(draft.phase(), &DraftPhase::Editing);
    assert_eq!(draft.text(), "  Measured under load.  ");
    assert!(
        draft.begin_submit().is_none(),
        "a refusal needs a fresh preview before resubmitting"
    );

    draft.preview(&design());
    assert_eq!(draft.begin_submit(), Some(submitted.clone()));
    let publication: wire::rooms::RoomAuthorityStamp =
        "dVsYdDj7d".parse().expect("fixture publication");
    draft.settle(Ok(RoomContributeAnswer::new(
        submitted.request.room,
        publication.clone(),
        RoomContributionKind::Evidence,
    )
    .expect("fixture room handle")));
    assert_eq!(draft.notice(), Some(&DraftNotice::Contributed(publication)));
    assert!(draft.text().is_empty() && draft.sources().is_empty());
    assert_eq!(draft.kind(), Some(RoomContributionKind::Evidence));
    draft.settle(Err("late".to_owned()));
    assert!(
        matches!(draft.notice(), Some(DraftNotice::Contributed(_))),
        "only a pending submission settles"
    );
}

#[test]
fn cited_sources_witness_in_order_and_unreadable_spellings_refuse() {
    let mut draft = ready();
    draft.cite(&digest());
    draft.cite("dW1d7gTFv");
    draft.cite(&digest());
    let artifact = RoomSource::Artifact {
        digest: digest().parse().expect("fixture digest"),
    };
    let journal = RoomSource::Journal {
        fact: "dW1d7gTFv".parse::<SequenceCode>().expect("fixture code"),
    };
    assert_eq!(
        draft.sources(),
        [artifact.clone(), journal.clone()],
        "citing an already cited source keeps one"
    );

    draft.cite("not a source");
    assert_eq!(refusal(&draft), "room::contribution-source-unreadable");
    assert_eq!(draft.sources().len(), 2);

    draft.preview(&design());
    let DraftPhase::Previewing(submission) = draft.phase() else {
        panic!("cited draft previews");
    };
    assert_eq!(
        submission.expected[2..],
        [
            Witness::Source {
                source: artifact.authority_digest()
            },
            Witness::Source {
                source: journal.authority_digest()
            },
        ]
    );

    draft.toggle_source(artifact);
    assert_eq!(draft.sources(), [journal]);
    assert_eq!(
        draft.phase(),
        &DraftPhase::Editing,
        "an edit retires the earlier preview"
    );

    for fact in 1..=ROOM_CONTRIBUTION_SOURCE_LIMIT as u64 {
        draft.toggle_source(RoomSource::Journal {
            fact: SequenceCode::new(fact),
        });
    }
    assert_eq!(draft.sources().len(), ROOM_CONTRIBUTION_SOURCE_LIMIT);
    assert_eq!(refusal(&draft), "room::contribution-sources-limit");
}
