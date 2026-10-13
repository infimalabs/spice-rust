//! Shadow relevance observations remain bounded, source-backed, and inert.

use super::*;
use wire::rooms::{
    RoomProgressStanding, RoomRelevanceAssessed, RoomRelevanceAssessment, RoomRelevanceClass,
    RoomRelevanceObservation, RoomRelevanceObserved, RoomRelevanceVerdict, RoomSource,
    RoomSourceUnavailable, RoomSourceView,
};

fn observation(
    room: &TaskStamp,
    identity: i64,
    through: SequenceCode,
    classification: RoomRelevanceClass,
    source: RoomSourceView,
) -> RoomRelevanceObservation {
    let progress = match classification {
        RoomRelevanceClass::RelevantContribution => RoomProgressStanding::Advanced,
        RoomRelevanceClass::Unknown => RoomProgressStanding::Unknown,
        _ => RoomProgressStanding::NoProgress,
    };
    RoomRelevanceObservation {
        observation: RoomAuthorityStamp::for_micros(1_700_000_000_000_000 + identity)
            .expect("observation identity"),
        room: room.clone(),
        through,
        sources: vec![source],
        classification,
        progress,
        explanation: format!("classification {classification:?} follows from the cited source"),
        exceptions: "none known".to_owned(),
        uncertainty: "later authority may change the operator's assessment".to_owned(),
    }
}

fn available(fact: SequenceCode) -> RoomSourceView {
    RoomSourceView::Available {
        source: RoomSource::Journal { fact },
    }
}

fn unavailable(fact: SequenceCode) -> RoomSourceView {
    RoomSourceView::Unavailable {
        source: RoomSource::Journal { fact },
        unavailable: RoomSourceUnavailable::Unreadable {},
    }
}

fn observe(journal: &mut Journal, observation: RoomRelevanceObservation) -> Commit {
    let mut write = JournalWrite::new(
        Actor::Spice,
        "task:rooms".parse().expect("room origin"),
        instant(),
        Producer::ResidentRecognition,
    );
    write
        .fact(RoomRelevanceObserved { observation }, FactCause::Root)
        .expect("observation fact builds");
    journal.apply(write)
}

fn assess(
    journal: &mut Journal,
    observation: RoomAuthorityStamp,
    verdict: RoomRelevanceVerdict,
) -> Commit {
    commit(
        journal,
        RoomRelevanceAssessed {
            assessment: RoomRelevanceAssessment {
                observation,
                verdict,
                explanation: format!("operator assessed this observation as {verdict:?}"),
            },
        },
    )
}

fn without_relevance(mut projection: crate::rooms::Projection) -> crate::rooms::Projection {
    projection.relevance = Default::default();
    projection
}

#[test]
fn observations_replay_exactly_without_moving_room_or_task_authority() {
    let mut journal = Journal::in_memory().expect("journal");
    let room = task(&mut journal, 1, None);
    let through = SequenceCode::new(
        cursor(&journal)
            .expect("task fact is a source horizon")
            .get(),
    );
    let before = without_relevance(projection(&journal));
    let classes = [
        RoomRelevanceClass::RelevantContribution,
        RoomRelevanceClass::Declined,
        RoomRelevanceClass::Duplicate,
        RoomRelevanceClass::StaleActivity,
        RoomRelevanceClass::MissedOpportunity,
        RoomRelevanceClass::Unknown,
        RoomRelevanceClass::Agreement,
        RoomRelevanceClass::MessageCount,
        RoomRelevanceClass::Silence,
        RoomRelevanceClass::ElapsedTime,
    ];
    let mut identities = Vec::new();
    for (index, classification) in classes.into_iter().enumerate() {
        let source = if classification == RoomRelevanceClass::Unknown {
            unavailable(through)
        } else {
            available(through)
        };
        let body = observation(&room, index as i64, through, classification, source);
        identities.push(body.observation.clone());
        observe(&mut journal, body).expect("bounded shadow observation commits");
    }
    assess(
        &mut journal,
        identities[0].clone(),
        RoomRelevanceVerdict::FalsePositive,
    )
    .expect("false positive is measurable");
    assess(
        &mut journal,
        identities[2].clone(),
        RoomRelevanceVerdict::Confirmed,
    )
    .expect("confirmed observation is measurable");
    assess(
        &mut journal,
        identities[5].clone(),
        RoomRelevanceVerdict::Unknown,
    )
    .expect("unknown assessment is measurable");

    assert_eq!(
        without_relevance(projection(&journal)),
        before,
        "shadow facts change no room, task, delivery, group, review, or authorization projection"
    );
    let budget =
        FactReadBudget::new(3, MAX_RETAINED_FACT_PAGE_BYTES).expect("three-row relevance budget");
    let first = journal
        .read(|read| read.room_relevance_page(&room, None, budget))
        .expect("bounded first page");
    assert_eq!(first.rows.len(), 3);
    assert_eq!(first.continuation, Some(identities[2].clone()));
    assert_eq!(first.metrics.observations, 10);
    assert_eq!(first.metrics.relevant_contributions, 1);
    assert_eq!(first.metrics.declines, 1);
    assert_eq!(first.metrics.duplicates, 1);
    assert_eq!(first.metrics.stale_activity, 1);
    assert_eq!(first.metrics.missed_opportunities, 1);
    assert_eq!(first.metrics.non_evidence_signals, 4);
    assert_eq!(first.metrics.unknown_observations, 1);
    assert_eq!(first.metrics.progress, 1);
    assert_eq!(first.metrics.confirmed, 1);
    assert_eq!(first.metrics.false_positives, 1);
    assert_eq!(first.metrics.unknown_assessments, 1);
    assert_eq!(first.metrics.unassessed, 7);

    let expected = journal
        .read(|read| read.room_relevance_page(&room, None, FactReadBudget::RETAINED))
        .expect("complete relevance page");
    assert!(matches!(
        &expected.rows[5].observation.body().sources[0],
        RoomSourceView::Unavailable {
            unavailable: RoomSourceUnavailable::Unreadable {},
            ..
        }
    ));
    let derived = projection(&journal);
    let facts = fact_bytes(&journal);
    journal.verify().expect("relevance projection verifies");
    journal.reindex().expect("relevance projection rebuilds");
    assert_eq!(projection(&journal), derived);
    assert_eq!(
        fact_bytes(&journal),
        facts,
        "rebuild does not rewrite authority"
    );
    let rebuilt = journal
        .read(|read| read.room_relevance_page(&room, None, FactReadBudget::RETAINED))
        .expect("rebuilt relevance page");
    assert_eq!(rebuilt, expected);
}

#[test]
fn room_snapshot_carries_relevance_observations_and_assessments() {
    let mut journal = Journal::in_memory().expect("journal");
    let room = task(&mut journal, 3, None);
    let through = SequenceCode::new(
        cursor(&journal)
            .expect("task fact is a source horizon")
            .get(),
    );
    let body = observation(
        &room,
        0,
        through,
        RoomRelevanceClass::Declined,
        available(through),
    );
    let identity = body.observation.clone();
    observe(&mut journal, body).expect("bounded shadow observation commits");
    assess(&mut journal, identity, RoomRelevanceVerdict::Confirmed)
        .expect("confirmed observation is measurable");
    let horizon = cursor(&journal).expect("relevance facts are the horizon");
    let snapshot = journal
        .read(|read| {
            read.room_authority_page(
                crate::FactPageStart::After(None),
                horizon,
                FactReadBudget::RETAINED,
            )
        })
        .expect("room authority reads");
    let carried = |kind: FactKind| {
        snapshot
            .rows
            .iter()
            .filter(|fact| fact.fact.kind() == kind)
            .count()
    };
    assert_eq!(
        (
            carried(FactKind::RoomRelevanceObserved),
            carried(FactKind::RoomRelevanceAssessed)
        ),
        (1, 1),
        "the browser's room snapshot carries every observation and assessment"
    );
}

#[test]
fn non_evidence_signals_cannot_declare_progress() {
    let mut journal = Journal::in_memory().expect("journal");
    let room = task(&mut journal, 2, None);
    let through = SequenceCode::new(
        cursor(&journal)
            .expect("task fact is a source horizon")
            .get(),
    );
    for (index, classification) in [
        RoomRelevanceClass::Duplicate,
        RoomRelevanceClass::Agreement,
        RoomRelevanceClass::MessageCount,
        RoomRelevanceClass::Silence,
        RoomRelevanceClass::ElapsedTime,
    ]
    .into_iter()
    .enumerate()
    {
        let mut body = observation(
            &room,
            100 + index as i64,
            through,
            classification,
            available(through),
        );
        body.progress = RoomProgressStanding::Advanced;
        let result = observe(&mut journal, body);
        assert!(
            matches!(&result, Err(CommitError::Refused(refusal))
                if refusal.to_string().contains("requires progress NoProgress")),
            "{classification:?} cannot become progress: {result:?}"
        );
        refused(&journal, result);
    }
    let future = SequenceCode::new(through.get() + 1);
    let body = observation(
        &room,
        200,
        through,
        RoomRelevanceClass::Unknown,
        unavailable(future),
    );
    let result = observe(&mut journal, body);
    assert!(
        matches!(&result, Err(CommitError::Refused(refusal))
            if refusal.to_string().contains("exceeds horizon")),
        "an unavailable source still stays within its declared horizon: {result:?}"
    );
    refused(&journal, result);
}
