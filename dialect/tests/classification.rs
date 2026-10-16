use dialect::{
    Classification, ClassificationContext, EdgeKind, PROJECT_MAX_DEPTH, PROJECT_MIN_DEPTH,
    RefusalCode, WarningCode, classify, classify_with_context, emit,
};

fn document(classification: &Classification) -> &dialect::Document {
    classification
        .document()
        .expect("fixture should produce a document")
}

/// Slug-spelled segments carrying every byte class a segment may use.
fn segments_of_depth(depth: usize) -> Vec<String> {
    (0..depth).map(|segment| format!("seg_{segment}")).collect()
}

/// A project of exactly this many segments.
///
/// Fixtures that merely need *a* valid project call this rather than spelling
/// one out, so a change to the declared depth in `spice-rust.toml` cannot turn them
/// red for a reason they are not about.
fn project_of_depth(depth: usize) -> String {
    segments_of_depth(depth).join(".")
}

/// A project at a declared depth whose first segment carries this spelling.
fn project_spelled(spelling: &str) -> String {
    let mut segments = segments_of_depth(PROJECT_MIN_DEPTH);
    segments[0] = spelling.to_owned();
    segments.join(".")
}

fn classify_project(project: &str) -> Classification {
    classify_with_context(
        "# Goal\n",
        ClassificationContext {
            project: Some(project.to_owned()),
            origin: None,
        },
    )
}

fn accepts_project(project: &str) -> bool {
    classify_project(project).document().is_some()
}

fn refuses_project(project: &str) -> bool {
    classify_project(project)
        .refusals()
        .iter()
        .any(|refusal| refusal.code == RefusalCode::InvalidProject)
}

#[test]
fn canonical_output_is_current_and_carries_caller_identity() {
    let source = "# Cut release\n\n- Build backend\n  Acceptance: binary is reproducible\n  Priority: high\n- Build frontend\n  After: build-backend\n  Flow: todo, verify, review\n";
    let project = project_of_depth(PROJECT_MIN_DEPTH);
    let context = ClassificationContext {
        project: Some(project.clone()),
        origin: Some("ack:1kExample".to_owned()),
    };
    let first = classify_with_context(source, context.clone());
    let canonical = emit(document(&first));
    let second = classify_with_context(&canonical, context);
    assert!(matches!(second, Classification::Current(_)));
    let graph = document(&second);
    assert_eq!(graph.project.as_deref(), Some(project.as_str()));
    assert_eq!(graph.origin.as_deref(), Some("ack:1kExample"));
    assert_eq!(graph.nodes[graph.root].slug, "cut-release");
    assert!(graph.edges.iter().any(|edge| {
        edge.kind == EdgeKind::Explicit
            && graph.nodes[edge.source].slug == "build-frontend"
            && graph.nodes[edge.target].slug == "build-backend"
    }));
}

#[test]
fn convertible_spelling_reaches_a_byte_stable_fixed_point() {
    let source =
        "Release plan\n============\n\n* [x] Package bits\n  **Acceptance:** archive opens\n";
    let first = classify(source);
    let warnings = first.warnings();
    assert!(
        warnings
            .iter()
            .any(|warning| { warning.code == WarningCode::NonCanonicalHeading })
    );
    assert!(
        warnings
            .iter()
            .any(|warning| { warning.code == WarningCode::CheckedStatusDiscarded })
    );
    let once = emit(document(&first));
    let second = classify(&once);
    assert!(matches!(second, Classification::Current(_)));
    let twice = emit(document(&second));
    assert_eq!(once, twice);
    assert_eq!(
        once,
        "# Release plan\n\n- Package bits\n  Acceptance: archive opens\n"
    );
}

#[test]
fn containment_and_qualified_duplicate_titles_are_unambiguous() {
    let source = "# Rollout\n\n## Phase one\n\n- Update docs\n\n## Phase two\n\n- Update docs\n- Ship fleet\n  After: phase-one--update-docs\n";
    let classification = classify(source);
    let graph = document(&classification);
    assert!(graph.node("phase-one--update-docs").is_some());
    assert!(graph.node("phase-two--update-docs").is_some());
    let canonical = emit(graph);
    assert!(matches!(classify(&canonical), Classification::Current(_)));
}

#[test]
fn several_top_level_items_receive_a_synthetic_root() {
    let classification = classify("- First task\n- Second task\n");
    let graph = document(&classification);
    assert_eq!(graph.nodes[graph.root].slug, "document-root");
    assert_eq!(graph.nodes[graph.root].children.len(), 2);
    assert_eq!(emit(graph), "- First task\n- Second task\n");
}

#[test]
fn every_convertible_result_has_a_reason_code() {
    let classification = classify("# Goal\n");
    match classification {
        Classification::Current(_) => {}
        Classification::Convertible { warnings, .. } => assert!(!warnings.is_empty()),
        Classification::Refused { .. } => panic!("valid goal was refused"),
    }
}

#[test]
fn escaped_field_shape_remains_description_prose() {
    let first = classify("# Goal\n\nOwner\\: operator prose\n");
    let canonical = emit(document(&first));
    assert_eq!(canonical, "# Goal\n\nOwner\\: operator prose\n");
    assert!(matches!(classify(&canonical), Classification::Current(_)));
}

#[test]
fn invalid_context_is_a_named_refusal() {
    let classification = classify_with_context(
        "# Goal\n",
        ClassificationContext {
            project: Some("Task Bad".to_owned()),
            origin: Some("memory:old".to_owned()),
        },
    );
    let codes = classification
        .refusals()
        .iter()
        .map(|refusal| refusal.code)
        .collect::<Vec<_>>();
    assert_eq!(
        codes,
        vec![RefusalCode::InvalidProject, RefusalCode::InvalidOrigin]
    );
}

/// Depth is declared law, so the fixture asks the declaration what to try.
///
/// Spelling the depths out here would restate the bound the classifier just
/// stopped restating: changing the declared number in `spice-rust.toml` would turn
/// this red and force exactly the Rust edit the declaration exists to avoid.
#[test]
fn project_depth_follows_the_declared_bounds() {
    for depth in PROJECT_MIN_DEPTH..=PROJECT_MAX_DEPTH {
        let project = project_of_depth(depth);
        assert!(
            accepts_project(&project),
            "depth {depth} is within the declared bounds: {project}",
        );
    }

    for depth in [PROJECT_MIN_DEPTH - 1, PROJECT_MAX_DEPTH + 1] {
        let project = project_of_depth(depth);
        assert!(
            refuses_project(&project),
            "depth {depth} is outside the declared bounds: {project}",
        );
    }
}

#[test]
fn project_segments_must_be_slug_spelled_at_a_declared_depth() {
    for spelling in ["wire-name", "Wire", "wire name", ""] {
        let project = project_spelled(spelling);
        assert!(
            refuses_project(&project),
            "a segment spelled `{spelling}` is not a slug: {project}",
        );
    }
}

#[test]
fn accepted_corpus_converges_after_one_normalization() {
    let fixtures = [
        "- A\n- B\n",
        "Goal\n====\n\n1. First\n2. Second\n",
        "# Goal\n\n- [ ] Work\n  Done when: complete\n  Deadline: 2026-08-01\n",
        "# Goal\n\n```text\n# preserved code\n```\n",
        "# Goal\n\n\\- preserved prose\nAfter\\: preserved prose\n",
    ];
    for fixture in fixtures {
        let first = classify(fixture);
        let once = emit(document(&first));
        let second = classify(&once);
        assert!(
            matches!(second, Classification::Current(_)),
            "canonical output was not current:\n{once}"
        );
        assert_eq!(emit(document(&second)), once);
    }
}

#[test]
fn nested_fenced_annotations_reach_the_same_fixed_point() {
    let source = "# Goal\n\n- Task\n\n  ```text\n  preserved\n    nested\n  ```\n";

    let first = classify(source);
    let once = emit(document(&first));
    let second = classify(&once);

    assert!(
        matches!(second, Classification::Current(_)),
        "nested fence did not become current:\n{once}"
    );
    assert_eq!(emit(document(&second)), once);
}
