//! Effort-fold runtime behavior kept separate from the integration root.

use super::*;

/// One proven phase whose counters are unlike any other number in this suite.
///
/// The values are distinctive on purpose. A surface that showed a total it
/// derived rather than the one the fold produced would have to reproduce these
/// exact digits by accident to escape notice.
fn proven_usage() -> wire::intent::EffortUsage {
    wire::intent::EffortUsage {
        turns: 12,
        input_tokens: 990_113,
        cached_input_tokens: 990_227,
        cache_write_input_tokens: 990_331,
        output_tokens: 990_449,
        reasoning_output_tokens: Some(990_557),
    }
}

fn launch(model: &str, effort: &str) -> wire::intent::EffortIdentity {
    wire::intent::EffortIdentity {
        vendor: "claude".parse().expect("fixture vendor is a name"),
        model: model.to_owned(),
        effort: effort.to_owned(),
    }
}

fn effort_phase(availability: wire::intent::EffortAvailability) -> wire::intent::EffortPhaseAnswer {
    wire::intent::EffortPhaseAnswer {
        task: "1kKRRsDP".parse().expect("fixture stamp is canonical"),
        phase: "todo".parse().expect("fixture phase is a phase"),
        phase_index: 0,
        opening: seq(41),
        closing: Some(seq(57)),
        renewals: 1,
        opening_boundary: wire::body::SourceBoundary::Proven { heads: Vec::new() },
        closing_boundary: Some(wire::body::SourceBoundary::Proven { heads: Vec::new() }),
        segments: Vec::new(),
        availability,
    }
}

fn effort_answer() -> wire::intent::EffortAnswer {
    wire::intent::EffortAnswer {
        phases: vec![
            effort_phase(wire::intent::EffortAvailability::Available {
                identity: launch("claude-opus-5", "high"),
                usage: proven_usage(),
                wall_ms: 990_661,
            }),
            effort_phase(wire::intent::EffortAvailability::Unavailable {
                reasons: vec![wire::intent::EffortUnavailable {
                    code: wire::intent::EffortUnavailableCode::UntaggedLaunch,
                    detail: "the launch recorded no model".to_owned(),
                }],
            }),
        ],
        aggregates: Vec::new(),
    }
}

fn effort_response(
    session: &Session,
    outcome: Outcome<wire::intent::EffortAnswer>,
) -> app::Admitted<Response<wire::intent::EffortAnswer>> {
    let bytes = support::response(WireVersion::CURRENT, uuid(REQUEST), uuid(EPOCH), outcome);
    session
        .admit_frame(&bytes)
        .expect("effort response is admitted")
}

#[test]
fn an_effort_refusal_replaces_the_held_fold_rather_than_standing_beside_it() {
    let session = session(OPERATOR, EPOCH);
    let mut runtime = CanvasRuntime::new(&acceptance(&session, "operator", EPOCH));
    assert_eq!(runtime.effort(), None, "nothing is held before the read");

    runtime.apply_effort(&effort_response(
        &session,
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(57)),
            body: effort_answer(),
        }),
    ));
    let app::EffortState::Answered(answer) = runtime.effort().expect("the fold is held") else {
        panic!("a reply is held as an answer");
    };
    assert_eq!(answer.phases.len(), 2);

    runtime.apply_effort(&effort_response(
        &session,
        Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new("effort::selection-failed", None, None, None)
                .with_detail("name a task that exists"),
        )),
    ));
    // The counters are gone rather than dimmed: a refusal that left the last
    // good total readable would show a number no read behind it produced.
    let app::EffortState::Withheld(withheld) = runtime.effort().expect("the refusal is held")
    else {
        panic!("a refusal replaces the answer instead of leaving it standing");
    };
    assert_eq!(withheld.class(), "refusal");
    assert_eq!(withheld.reason(), "effort::selection-failed");
    assert_eq!(withheld.detail(), "name a task that exists");

    runtime.apply_effort(&effort_response(
        &session,
        Outcome::Fault(Fault {
            class: FaultClass::Store,
            reason: "effort::fold-failed".to_owned(),
            detail: "the fold store is not readable".to_owned(),
        }),
    ));
    let app::EffortState::Withheld(withheld) = runtime.effort().expect("the fault is held") else {
        panic!("a fault replaces the answer instead of leaving it standing");
    };
    assert_eq!(
        withheld.class(),
        "fault",
        "a fault is a distinct state from a refusal rather than one error word"
    );
    assert_eq!(withheld.detail(), "the fold store is not readable");
}

#[test]
fn a_changed_capability_clears_the_held_effort_fold() {
    let session = session(OPERATOR, EPOCH);
    let mut runtime = CanvasRuntime::new(&acceptance(&session, "operator", EPOCH));
    runtime.apply_effort(&effort_response(
        &session,
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(57)),
            body: effort_answer(),
        }),
    ));
    assert!(runtime.effort().is_some(), "the fold is held in its scope");

    let next = Session::open(uuid(NEXT_CAPABILITY), uuid(NEXT_EPOCH));
    assert!(runtime.reconnect(&acceptance(&next, "operator", NEXT_EPOCH)));
    assert_eq!(
        runtime.effort(),
        None,
        "effort is scoped exactly like the board, the roster and consent"
    );
}

#[test]
fn an_effort_answer_from_another_scope_moves_nothing() {
    let session = session(OPERATOR, EPOCH);
    let mut runtime = CanvasRuntime::new(&acceptance(&session, "operator", EPOCH));
    // The same epoch under another capability: the frame is decodable here,
    // so what stops it is the scope check rather than an earlier refusal.
    let elsewhere = Session::open(uuid(SECRET_LANE_CAPABILITY), uuid(EPOCH));

    runtime.apply_effort(&effort_response(
        &elsewhere,
        Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(seq(57)),
            body: effort_answer(),
        }),
    ));

    assert_eq!(runtime.effort(), None, "another scope's fold is not held");
    assert!(
        runtime.events().is_empty(),
        "and it leaves no chronology row either"
    );
}

#[test]
fn grouping_crosses_no_part_of_the_launch_tuple() {
    // Each pair differs in exactly one of the three parts, so a tag that
    // dropped or folded any one of them collides here instead of quietly
    // grouping two launches that the fold kept apart.
    let base = launch("claude-opus-5", "high");
    let model = launch("claude-opus-4-8", "high");
    let effort = launch("claude-opus-5", "low");
    let vendor = wire::intent::EffortIdentity {
        vendor: "codex".parse().expect("fixture vendor is a name"),
        ..base.clone()
    };

    let tag = app::effort_identity_tag;
    assert_eq!(tag(&base), "claude/claude-opus-5/high");
    assert_ne!(tag(&base), tag(&model), "the model is part of the tuple");
    assert_ne!(tag(&base), tag(&effort), "the effort is part of the tuple");
    assert_ne!(tag(&base), tag(&vendor), "the vendor is part of the tuple");
    assert_eq!(tag(&base), tag(&launch("claude-opus-5", "high")));
}

#[test]
fn a_complete_zero_is_distinct_from_every_phase_that_spent_something() {
    let nothing = wire::intent::EffortUsage::default();
    assert!(
        app::effort_is_zero(&nothing, 0),
        "a phase that spent nothing reads as a real zero"
    );
    assert!(
        app::effort_is_zero(
            &wire::intent::EffortUsage {
                reasoning_output_tokens: None,
                ..nothing.clone()
            },
            0,
        ),
        "an unreported reasoning counter reports nothing spent"
    );

    // One counter at a time, so a conjunction missing a term fails on the term
    // it dropped rather than passing on the five it kept.
    let spent = [
        wire::intent::EffortUsage {
            turns: 1,
            ..nothing.clone()
        },
        wire::intent::EffortUsage {
            input_tokens: 1,
            ..nothing.clone()
        },
        wire::intent::EffortUsage {
            cached_input_tokens: 1,
            ..nothing.clone()
        },
        wire::intent::EffortUsage {
            cache_write_input_tokens: 1,
            ..nothing.clone()
        },
        wire::intent::EffortUsage {
            output_tokens: 1,
            ..nothing.clone()
        },
        wire::intent::EffortUsage {
            reasoning_output_tokens: Some(1),
            ..nothing.clone()
        },
    ];
    for usage in &spent {
        assert!(
            !app::effort_is_zero(usage, 0),
            "{usage:?} spent something and is not a complete zero"
        );
    }
    assert!(
        !app::effort_is_zero(&nothing, 1),
        "wall time alone is enough to make a phase non-zero"
    );
}
