//! A settled result in a task room classifies by how it settled; only an
//! answer declares progress, and every observation cites the ASK and the
//! settling fact at or before its horizon.

use serde_json::json;
use wire::rooms::{
    QuestionResultRecord, QuestionResultState, RoomProgressStanding, RoomRelevanceClass,
    RoomSource, RoomSourceUnavailable, RoomSourceView,
};
use wire::{Seq, SequenceCode};

use super::{Settled, observe};

const RESULT: &str = "1kKRRsDR3";
const QUESTION: &str = "1kKRRsDR2";
const ROOM: &str = "dVrD244Jd";

fn record(context: bool) -> QuestionResultRecord {
    let mut record = json!({
        "result": RESULT,
        "question": QUESTION,
        "requester": {"kind": "lane", "lane": "00000000-0000-4000-8000-000000000007"},
        "respondent": {"kind": "operator"},
        "source": {"kind": "journal", "fact": SequenceCode::new(3)},
        "deadline": "2030-01-02T00:00:00.000Z",
    });
    if context {
        record["context"] = json!(ROOM);
    }
    serde_json::from_value(record).expect("fixture result record decodes")
}

fn settled(state: QuestionResultState, asked: Option<u64>) -> Settled {
    Settled {
        record: record(true),
        state,
        settled: Seq::new(9).expect("positive"),
        asked: asked.and_then(Seq::new),
        answered: Seq::new(8),
    }
}

fn journal(fact: u64) -> RoomSourceView {
    RoomSourceView::Available {
        source: RoomSource::Journal {
            fact: SequenceCode::new(fact),
        },
    }
}

#[test]
fn only_an_answer_declares_progress() {
    let answered: QuestionResultState = serde_json::from_value(json!({
        "state": "answered",
        "answer": "1kKRRsDR4",
        "return_delivery": {
            "destination": {"kind": "lane", "lane": "00000000-0000-4000-8000-000000000007"},
            "delivery": {"kind": "directive", "directive": "00000000-0000-4000-8000-000000000008"},
        },
    }))
    .expect("fixture answered state decodes");
    let cases = [
        (
            answered,
            RoomRelevanceClass::RelevantContribution,
            RoomProgressStanding::Advanced,
            8,
        ),
        (
            QuestionResultState::Declined {
                source: RoomSource::Journal {
                    fact: SequenceCode::new(9),
                },
            },
            RoomRelevanceClass::Declined,
            RoomProgressStanding::NoProgress,
            9,
        ),
        (
            QuestionResultState::TimedOut {},
            RoomRelevanceClass::MissedOpportunity,
            RoomProgressStanding::NoProgress,
            9,
        ),
        (
            QuestionResultState::Cancelled {
                source: RoomSource::Journal {
                    fact: SequenceCode::new(9),
                },
            },
            RoomRelevanceClass::StaleActivity,
            RoomProgressStanding::NoProgress,
            9,
        ),
    ];
    for (state, classification, progress, closing) in cases {
        let observation = observe(&settled(state.clone(), Some(5)))
            .unwrap_or_else(|| panic!("{} observes", state.name()));
        assert_eq!(
            observation.classification,
            classification,
            "{}",
            state.name()
        );
        assert_eq!(observation.progress, progress, "{}", state.name());
        assert_eq!(observation.observation.to_string(), RESULT);
        assert_eq!(observation.room.as_str(), ROOM);
        assert_eq!(
            observation.through.get(),
            9,
            "the horizon is the settled head"
        );
        assert_eq!(
            observation.sources,
            vec![journal(5), journal(closing)],
            "{} cites the ASK and the fact that settled it",
            state.name()
        );
        assert!(!observation.explanation.is_empty());
    }
}

#[test]
fn pending_or_roomless_results_are_not_observed() {
    assert!(observe(&settled(QuestionResultState::Pending {}, Some(5))).is_none());
    let roomless = Settled {
        record: record(false),
        ..settled(QuestionResultState::TimedOut {}, Some(5))
    };
    assert!(observe(&roomless).is_none());
}

#[test]
fn an_unadmitted_ask_is_cited_as_unavailable() {
    let observation = observe(&settled(QuestionResultState::TimedOut {}, None))
        .expect("a timed-out result observes");
    assert_eq!(
        observation.sources[0],
        RoomSourceView::Unavailable {
            source: RoomSource::Journal {
                fact: SequenceCode::new(3),
            },
            unavailable: RoomSourceUnavailable::Missing {},
        }
    );
}
