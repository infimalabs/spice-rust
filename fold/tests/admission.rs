#[allow(dead_code)]
#[path = "../build/admission.rs"]
mod admission;

use admission::{AnswerContent, FactRegistration, FoldRegistration, RingRegistration, Source};

const ROWS: &[(&str, &str, &str)] = &[
    ("SteerInbox", "steer::inbox", "Pending directives per lane."),
    (
        "ConsentQueue",
        "consent::queue",
        "Pending consent and remaining budgets.",
    ),
    (
        "LaneCanary",
        "lane::canary",
        "Armed windows, listening meters, and alarms.",
    ),
    (
        "TasteActive",
        "taste::active",
        "Active tastes and evidence ages.",
    ),
    (
        "CanvasVerdicts",
        "canvas::verdicts",
        "Commit and finding verdict history.",
    ),
    (
        "CanvasMagnitude",
        "canvas::magnitude",
        "Effort, pressure, residue, and consent rates.",
    ),
    ("CanvasFlow", "canvas::flow", "Tasks moving through phases."),
    (
        "CanvasTopology",
        "canvas::topology",
        "Lanes, worktrees, occupant lineage, and possession.",
    ),
    (
        "CanvasChronology",
        "canvas::chronology",
        "Compaction, queues, human delay, trust, recovery, and lifecycle transitions in time.",
    ),
    (
        "ReleaseHistory",
        "release::history",
        "Sealed and published artifacts.",
    ),
];

const FIXTURE_SOURCE: Source = Source::Fact {
    variant: "FixtureKind",
    kind: "fixture::kind",
};
const FIXTURE_CONTENT: AnswerContent = AnswerContent::Fact {
    variant: "FixtureKind",
    kind: "fixture::kind",
};

fn complete_folds(
    target_sources: &'static [Source],
    target_content: &'static [AnswerContent],
) -> Vec<FoldRegistration> {
    ROWS.iter()
        .map(|(variant, name, answer)| FoldRegistration {
            variant,
            name,
            answer,
            content: if *name == "steer::inbox" {
                target_content
            } else {
                &[FIXTURE_CONTENT]
            },
            sources: if *name == "steer::inbox" {
                target_sources
            } else {
                &[FIXTURE_SOURCE]
            },
            beyond: "Answer",
        })
        .collect()
}

fn ring() -> [RingRegistration; 1] {
    [RingRegistration {
        variant: "BusRefusals",
        name: "bus::refusals",
        recovery: "None",
    }]
}

#[test]
fn generation_refuses_an_unconsumed_fact_and_names_it() {
    let facts = [
        FactRegistration {
            variant: "FixtureKind".to_owned(),
            kind: "fixture::kind".to_owned(),
            producer: Some("CommandDoor".to_owned()),
        },
        FactRegistration {
            variant: "OrphanKind".to_owned(),
            kind: "orphan::kind".to_owned(),
            producer: Some("CommandDoor".to_owned()),
        },
    ];
    let folds = complete_folds(
        &[],
        &[AnswerContent::Head {
            variant: "Witness",
            name: "witness",
        }],
    );

    let refusal = admission::validate(&facts, &folds, &ring())
        .expect_err("a fact no fold consumes must stop generation");

    assert_eq!(
        refusal.to_string(),
        "fact kind orphan::kind has no consuming fold"
    );
}

#[test]
fn generation_refuses_a_fact_without_a_producer_and_names_it() {
    let facts = [FactRegistration {
        variant: "OrphanKind".to_owned(),
        kind: "orphan::kind".to_owned(),
        producer: None,
    }];
    let folds = complete_folds(
        &[Source::Fact {
            variant: "OrphanKind",
            kind: "orphan::kind",
        }],
        &[AnswerContent::Fact {
            variant: "OrphanKind",
            kind: "orphan::kind",
        }],
    );

    let refusal = admission::validate(&facts, &folds, &ring())
        .expect_err("a fact no producer can mint must stop generation");

    assert_eq!(
        refusal.to_string(),
        "fact kind orphan::kind has no registered producer class"
    );
}

#[test]
fn generation_refuses_answer_content_no_admitted_kind_can_supply() {
    let facts = [FactRegistration {
        variant: "FixtureKind".to_owned(),
        kind: "fixture::kind".to_owned(),
        producer: Some("CommandDoor".to_owned()),
    }];
    let folds = complete_folds(
        &[FIXTURE_SOURCE],
        &[AnswerContent::Fact {
            variant: "OrphanContent",
            kind: "orphan::content",
        }],
    );

    let refusal = admission::validate(&facts, &folds, &ring())
        .expect_err("unsupplied answer content must stop generation");

    assert_eq!(
        refusal.to_string(),
        "fold steer::inbox answer names fact content orphan::content, but no admitted kind can supply it"
    );
}
