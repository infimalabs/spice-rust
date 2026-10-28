use super::*;

#[test]
fn research_coverage_belongs_only_to_open_design_passes() {
    let questions = vec!["Which boundary owns cancellation?".to_owned()];
    let mut workflow = wire::WorkflowHead {
        flow: ["plan", "todo", "review", "design"]
            .map(|phase| phase.parse().expect("fixture phase"))
            .to_vec(),
        phase: "plan".parse().expect("fixture phase"),
        status: wire::WorkflowStatus::Pending(wire::WorkflowReview::Unassigned),
        deferral: None,
        generation: 0,
    };
    for intent in [
        wire::TaskIntent::Design,
        wire::TaskIntent::OpportunisticDesign,
    ] {
        for phase in ["plan", "todo", "review"] {
            workflow.phase = phase.parse().expect("fixture phase");
            assert_eq!(
                design_coverage(Some(intent), &workflow, &questions, None),
                None,
                "{intent} in {phase} has no research pass"
            );
        }
        workflow.phase = "design".parse().expect("fixture phase");
        assert_eq!(
            design_coverage(Some(intent), &workflow, &questions, None),
            Some(DesignCoverage {
                covered: Vec::new(),
                missing: vec![wire::rooms::RoomQuestion::new(1).expect("first question")],
            }),
            "an open {intent} pass retains its current question"
        );
    }
}
