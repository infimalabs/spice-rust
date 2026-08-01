use dialect::{Classification, RefusalCode, classify};

fn assert_refusal(source: &str, code: RefusalCode, message: &str) {
    let Classification::Refused { refusals, .. } = classify(source) else {
        panic!("fixture should be refused");
    };
    assert!(
        refusals
            .iter()
            .any(|refusal| refusal.code == code && refusal.message == message),
        "missing {code}: {message}; got {refusals:?}"
    );
}

#[test]
fn empty_and_structureless_documents_refuse_exactly() {
    assert_refusal(
        " \n\t\n",
        RefusalCode::EmptyDocument,
        "task document is empty",
    );
    assert_refusal(
        "prose only\n",
        RefusalCode::NoNodes,
        "task document has no nodes (content but no structure)",
    );
}

#[test]
fn malformed_and_unknown_dependencies_are_distinct() {
    assert_refusal(
        "# Goal\nAfter: missing!\n",
        RefusalCode::MalformedDependency,
        "malformed After target: missing! (line 2)",
    );
    assert_refusal(
        "# Goal\nAfter: missing\n",
        RefusalCode::UnknownDependency,
        "unknown After target: missing (line 2)",
    );
    assert_refusal(
        "# Goal\nAfter: goal,\n",
        RefusalCode::MalformedDependency,
        "malformed After target: empty target (line 2)",
    );
}

#[test]
fn duplicate_identity_refuses_with_source_lines() {
    assert_refusal(
        "# Goal\n\n- Same task\n- Same task\n",
        RefusalCode::DuplicateIdentity,
        "duplicate identity: same-task (lines 3, 4)",
    );
}

#[test]
fn unsupported_field_refuses_instead_of_becoming_silent_metadata() {
    assert_refusal(
        "# Goal\nOwner: mei\n",
        RefusalCode::UnsupportedField,
        "unsupported field: Owner (line 2)",
    );
}

#[test]
fn priority_and_flow_use_closed_vocabularies() {
    assert_refusal(
        "# Goal\nPriority: urgent\n",
        RefusalCode::InvalidPriority,
        "invalid priority: urgent (line 2)",
    );
    assert_refusal(
        "# Goal\nFlow: todo, deploy\n",
        RefusalCode::InvalidFlowPhase,
        "invalid flow phase: deploy (line 2)",
    );
}

#[test]
fn explicit_dependency_cycles_refuse() {
    let Classification::Refused { refusals, .. } =
        classify("# Goal\n\n- Alpha\n  After: beta\n- Beta\n  After: alpha\n")
    else {
        panic!("cycle should be refused");
    };
    assert!(refusals.iter().any(|refusal| {
        refusal.code == RefusalCode::DependencyCycle
            && refusal.message.starts_with("dependency cycle at ")
    }));
}

#[test]
fn title_requires_an_ascii_identity_word() {
    assert_refusal(
        "# !!!\n",
        RefusalCode::TitleWithoutAsciiWords,
        "title has no ASCII words: !!! (line 1)",
    );
}

#[test]
fn bare_reference_to_qualified_duplicates_is_ambiguous() {
    assert_refusal(
        "# Goal\n\n## One\n\n- Shared\n\n## Two\n\n- Shared\n- Consumer\n  After: shared\n",
        RefusalCode::AmbiguousDependency,
        "ambiguous After target: shared (line 11; use a qualified slug)",
    );
}
