use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use gate::metrics::{
    EvidenceState, LatchBinding, MagicFinding, MetricEvidence, MetricId, MetricKey,
    MetricMeasurement, MetricParity, MetricReport, MetricStateBinding, MetricUnit, ParseError,
    ParserEvidence, ParserId, ParserMode, PredecessorMetricObservation, PredecessorMetricResult,
    ResolvedMetricRule, RuleBinding, STRUCTURAL_ARMS, STRUCTURAL_METRICS, StructuralArmEvidence,
    compare_predecessor_metric, measure_file_shape, measure_repository_document, parse_lizard_csv,
    resolve_magic_metric, resolve_metric_rule, scan_magic_regressions, scan_markdown_link_case,
};
use gate::state::{ActorInputs, CurrentState, LatchKind, PredecessorSources, RenameMap};
use gate::{
    GateId, PREDECESSOR_GATE_RELEASE, PolicyContext, RulePolicy, migrate_predecessor_policy,
};

const EXISTING_MAGIC_VALUE: u64 = 75;
const FIXTURE_BASE_LIMIT: u64 = 1_000;
const FIXTURE_RATCHET_LIMIT: u64 = 1_500;
const NEW_MAGIC_VALUE: u64 = 100;
const PYTHON_MAGIC_VALUE: u64 = 50;
const STATE_CLEAR_VALUE: u64 = 20;
const STATE_HELD_VALUE: u64 = 30;
const STATE_LATCH_VALUE: u64 = 42;
const STATE_TEST_CLOCK: f64 = 10_000.0;

static NEXT_STATE_FIXTURE: AtomicU64 = AtomicU64::new(0);

const SCOPED_POLICY: &str = r#"
[policy.limits]
file_loc = 10
file_bytes = 100
routine_ccn = 5
routine_length = 8
commit_message_wrap = 72
repo_truth_doc_chars = 1000

[policy.flex]
ratio = 2.0
jitter_percent = 7

[policy.magic]
examine_threshold = 10
baseline_ref = "HEAD"

[policy.markdown_depth_budget]
extensions = [".mdoc"]
stem_pattern = "[A-Z_]+"

[[policy.rules]]
scopes = { paths = ["src/**"] }
multiplier = 2.0
flex = 2.0

[[policy.rules]]
scopes = { paths = ["src/strict/**"] }
[policy.rules.magic]
examine_threshold = 100
"#;

#[test]
fn declarations_freeze_ids_units_parsers_scopes_and_order() {
    assert_eq!(
        STRUCTURAL_ARMS.map(|arm| arm.id()),
        [
            GateId::FileShape,
            GateId::Complexity,
            GateId::MagicNumbers,
            GateId::MarkdownLinks,
        ]
    );
    assert_eq!(STRUCTURAL_METRICS.map(|metric| metric.id()), MetricId::ALL);
    assert_eq!(STRUCTURAL_METRICS[0].unit(), MetricUnit::SourceLines);
    assert_eq!(
        STRUCTURAL_METRICS[2].key(),
        gate::metrics::KeyShape::PathFunction
    );
    assert_eq!(
        STRUCTURAL_METRICS[4].owner(),
        GateId::RepoDocs,
        "repo-doc owns document-character enforcement"
    );
    let complexity = STRUCTURAL_ARMS[1];
    assert_eq!(complexity.parser().mode(), ParserMode::External);
    let lizard = complexity
        .parser()
        .tool()
        .expect("complexity declares lizard");
    assert_eq!(lizard.program(), "lizard");
    assert_eq!(lizard.version_arguments(), ["--version"]);
    assert_eq!(lizard.exact_version(), "1.23.0");
    assert_eq!(lizard.analysis_arguments(), ["--csv"]);
    assert!(
        STRUCTURAL_ARMS[0]
            .scope()
            .exclusions()
            .contains(&"repository-truth documents")
    );
}

#[test]
fn measured_parser_evidence_binds_mode_tool_version_and_executable_identity() {
    let policy = migrate_predecessor_policy("fixtures/metrics.toml", SCOPED_POLICY)
        .expect("predecessor policy migrates once");
    let complexity_rule = resolve_metric_rule(
        &policy,
        MetricId::RoutineCcn,
        &PolicyContext::new().with_path("src/app.rs"),
    )
    .expect("complexity rule resolves");
    let complexity_key = MetricKey::function("src/app.rs", "run").unwrap();

    for parser in [
        ParserEvidence::Native {
            parser: ParserId::LizardCsvV1,
        },
        ParserEvidence::External {
            parser: ParserId::LizardCsvV1,
            tool: "python-lizard".to_owned(),
            version: "1.23.0".to_owned(),
            executable_digest: "sha256:fixture".to_owned(),
        },
        ParserEvidence::External {
            parser: ParserId::LizardCsvV1,
            tool: "lizard".to_owned(),
            version: "1.24.0".to_owned(),
            executable_digest: "sha256:fixture".to_owned(),
        },
        ParserEvidence::External {
            parser: ParserId::LizardCsvV1,
            tool: "lizard".to_owned(),
            version: "1.23.0".to_owned(),
            executable_digest: String::new(),
        },
    ] {
        assert_parser_refused_by_both(
            MetricId::RoutineCcn,
            complexity_key.clone(),
            5,
            "routine_ccn",
            parser,
            &complexity_rule,
        );
    }

    let file_rule = resolve_metric_rule(
        &policy,
        MetricId::FileLines,
        &PolicyContext::new().with_path("src/app.rs"),
    )
    .expect("file rule resolves");
    assert_parser_refused_by_both(
        MetricId::FileLines,
        MetricKey::path("src/app.rs").unwrap(),
        12,
        "file_loc",
        ParserEvidence::External {
            parser: ParserId::Utf8FileShapeV1,
            tool: "lizard".to_owned(),
            version: "1.23.0".to_owned(),
            executable_digest: "sha256:fixture".to_owned(),
        },
        &file_rule,
    );

    let pinned = ParserEvidence::External {
        parser: ParserId::LizardCsvV1,
        tool: "lizard".to_owned(),
        version: "1.23.0".to_owned(),
        executable_digest: "sha256:fixture".to_owned(),
    };
    MetricEvidence::new(
        MetricId::RoutineCcn,
        complexity_key.clone(),
        EvidenceState::measured(bounded_measurement_with_parser(
            MetricId::RoutineCcn,
            5,
            "routine_ccn",
            pinned.clone(),
        )),
    )
    .expect("the exact declared lizard evidence is measured");
    MetricStateBinding::new(
        MetricId::RoutineCcn,
        complexity_key,
        5,
        pinned,
        complexity_rule,
    )
    .expect("the exact declared lizard evidence enters state binding");
}

#[test]
fn native_file_and_document_parsers_preserve_predecessor_counts() {
    let source = "alpha\r\nbeta\u{2028}gamma\n".as_bytes();
    let shape = measure_file_shape("src/app.rs", source).expect("UTF-8 source measures");
    assert_eq!(shape.path(), "src/app.rs");
    assert_eq!(shape.lines(), 3);
    assert_eq!(shape.bytes(), u64::try_from(source.len()).unwrap());
    for separator in [
        "\n", "\r", "\u{000b}", "\u{000c}", "\u{001c}", "\u{001d}", "\u{001e}", "\u{0085}",
        "\u{2028}", "\u{2029}",
    ] {
        let text = format!("left{separator}right{separator}");
        assert_eq!(
            measure_file_shape("src/separators.rs", text.as_bytes())
                .expect("every Python line separator measures")
                .lines(),
            2,
            "separator {separator:?}"
        );
    }
    let invalid = measure_file_shape("src/invalid.rs", b"a\xffb\nc")
        .expect_err("the predecessor excludes invalid UTF-8 source blobs");
    assert!(matches!(
        invalid,
        ParseError::Unsupported {
            ref source_field,
            ref recovery,
            ..
        } if source_field == "policy.file_shape.source_suffixes"
            && recovery.contains("encode it as UTF-8")
    ));
    assert!(matches!(
        invalid.into_evidence_state(),
        EvidenceState::Unsupported { owning_release, .. }
            if owning_release == PREDECESSOR_GATE_RELEASE
    ));
    assert_eq!(
        measure_file_shape("src/empty.rs", b"")
            .expect("empty text is measured, not omitted")
            .lines(),
        0
    );

    let document = measure_repository_document("README.md", b"a\xffb")
        .expect("invalid UTF-8 is decoded with replacement");
    assert_eq!(document.characters(), 3);
    assert!(matches!(
        measure_file_shape("src/image.rs", b"a\0b"),
        Err(ParseError::Unsupported { source_field, .. })
            if source_field == "policy.file_shape.source_suffixes"
    ));
    assert!(matches!(
        measure_repository_document("README.md", b"a\0b"),
        Err(ParseError::Unsupported { source_field, .. })
            if source_field == "policy.repo_truth.docs"
    ));
}

#[test]
fn lizard_csv_is_strict_per_function_and_never_drops_bad_rows() {
    let stdout = concat!(
        "15,5,123,2,17,\"run@1-17@/repo/src/app.rs\",",
        "\"/repo/src/app.rs\",\"run,worker\",signature,1,17\n"
    );
    let records = parse_lizard_csv(
        Path::new("/repo"),
        &["src/app.rs".to_owned()],
        stdout.as_bytes(),
    )
    .expect("pinned lizard row parses");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].path(), "src/app.rs");
    assert_eq!(records[0].function(), "run,worker");
    assert_eq!(
        (records[0].ccn(), records[0].length(), records[0].nloc()),
        (5, 17, 15)
    );

    let short = b"15,5,123,2,17,location,src/app.rs\n";
    assert!(matches!(
        parse_lizard_csv(Path::new("/repo"), &["src/app.rs".to_owned()], short),
        Err(ParseError::Malformed { reason, .. }) if reason.contains("expected 11 fields")
    ));
    let duplicate = format!("{stdout}{stdout}");
    assert!(matches!(
        parse_lizard_csv(
            Path::new("/repo"),
            &["src/app.rs".to_owned()],
            duplicate.as_bytes()
        ),
        Err(ParseError::Collision { .. })
    ));
}

#[test]
fn magic_delta_matches_supported_comparisons_and_refuses_ast_substitution() {
    let baseline = format!("if (value > {EXISTING_MAGIC_VALUE}) {{}}\n");
    let current = format!(
        concat!(
            "if (value > {existing}) {{}}\n",
            "if (count == 5) {{}}\n",
            "let named = 99;\n",
            "if (size <= {new}) {{}}\n",
            "// if (commented > 500) {{}}\n",
            "let markup = `if (copy > 600) {{}}`;\n",
        ),
        existing = EXISTING_MAGIC_VALUE,
        new = NEW_MAGIC_VALUE
    );
    let findings = scan_magic_regressions("src/app.rs", &current, Some(&baseline), 10)
        .expect("C-grammar source scans");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].line(), 4);
    assert_eq!(findings[0].literal(), "100");
    let separated = scan_magic_regressions(
        "src/app.rs",
        &format!("ignored\u{2028}x > {NEW_MAGIC_VALUE}"),
        None,
        10,
    )
    .expect("Python line separators retain predecessor line numbers");
    assert_eq!(separated[0].line(), 2);
    let unclosed_comment =
        scan_magic_regressions("src/app.rs", &format!("/* x > {NEW_MAGIC_VALUE}"), None, 10)
            .expect("predecessor regex leaves an unclosed block-comment marker intact");
    assert_eq!(unclosed_comment[0].literal(), "100");
    let pair = format!("{EXISTING_MAGIC_VALUE} < {NEW_MAGIC_VALUE}");
    let pair = scan_magic_regressions("src/app.rs", &pair, None, 10)
        .expect("one comparison follows predecessor non-overlapping matching");
    assert_eq!(
        pair.iter().map(MagicFinding::literal).collect::<Vec<_>>(),
        [EXISTING_MAGIC_VALUE.to_string()]
    );
    let chain = format!("{PYTHON_MAGIC_VALUE} < {EXISTING_MAGIC_VALUE} < {NEW_MAGIC_VALUE}");
    let chain = scan_magic_regressions("src/app.rs", &chain, None, 10)
        .expect("comparison chains follow predecessor non-overlapping matching");
    assert_eq!(
        chain.iter().map(MagicFinding::literal).collect::<Vec<_>>(),
        [
            PYTHON_MAGIC_VALUE.to_string(),
            EXISTING_MAGIC_VALUE.to_string()
        ]
    );
    let consumed = format!("5 < {NEW_MAGIC_VALUE}");
    assert!(
        scan_magic_regressions("src/app.rs", &consumed, None, 10)
            .expect("a below-threshold match still consumes its comparison")
            .is_empty()
    );

    assert!(matches!(
        scan_magic_regressions(
            "src/app.py",
            &format!("if x > {PYTHON_MAGIC_VALUE}: pass"),
            None,
            10
        ),
        Err(ParseError::Unsupported { source_field, recovery, .. })
            if source_field == "policy.languages.magic"
                && recovery.contains("exact native parser")
    ));
    assert!(matches!(
        scan_magic_regressions(
            "src/app.txt",
            &format!("x > {PYTHON_MAGIC_VALUE}"),
            None,
            10
        ),
        Err(ParseError::NotApplicable { .. })
    ));
}

#[test]
fn markdown_links_use_projected_inventory_and_exact_ascii_case() {
    let tracked = vec![
        "README.md".to_owned(),
        "Docs/Guide.md".to_owned(),
        "images/logo.png".to_owned(),
    ];
    let text = concat!(
        "[Guide](docs/guide.md)\n",
        "[External](https://example.com)\n",
        "[Anchor](#local)\n",
    );
    let findings =
        scan_markdown_link_case("README.md", text, &tracked).expect("tracked Markdown scans");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].resolved_path(), "docs/guide.md");
    assert_eq!(findings[0].expected_path(), "Docs/Guide.md");
    let separated =
        scan_markdown_link_case("README.md", "plain\u{2028}[Guide](docs/guide.md)", &tracked)
            .expect("Python line separators retain predecessor link line numbers");
    assert_eq!(separated[0].line(), 2);

    assert!(matches!(
        scan_markdown_link_case("README.md", "[x](Döcs.md)", &tracked),
        Err(ParseError::Unsupported { source_field, .. })
            if source_field == "markdown link target"
    ));
    let collision = vec!["Docs/Guide.md".to_owned(), "docs/guide.md".to_owned()];
    assert!(matches!(
        scan_markdown_link_case("README.md", "", &collision),
        Err(ParseError::Collision { .. })
    ));
}

#[test]
fn policy_binding_preserves_regex_rules_magic_and_markdown_depth() {
    let policy = migrate_predecessor_policy("fixtures/metrics.toml", SCOPED_POLICY)
        .expect("predecessor policy migrates once");
    let file = resolve_metric_rule(
        &policy,
        MetricId::FileLines,
        &PolicyContext::new().with_path("src/app.rs"),
    )
    .expect("scoped file rule resolves");
    assert_eq!((file.base_limit(), file.ratchet_limit()), (20, 40));
    assert_eq!(file.variation_percent(), 7);
    assert!(file.binding().rule_id().is_some());
    assert!(file.binding().match_basis().contains("src/**"));

    let magic = resolve_magic_metric(
        &policy,
        &PolicyContext::new().with_path("src/strict/app.rs"),
    )
    .expect("scoped magic rule resolves");
    assert_eq!(magic.threshold(), 100);

    let root = resolve_metric_rule(
        &policy,
        MetricId::RepositoryDocumentCharacters,
        &PolicyContext::new().with_path("DOCS_MAIN.mdoc"),
    )
    .expect("root generated Markdown rule resolves");
    assert!(
        root.binding()
            .match_basis()
            .contains("dialect=rust-regex-1")
    );
    let nested = resolve_metric_rule(
        &policy,
        MetricId::RepositoryDocumentCharacters,
        &PolicyContext::new().with_path("docs/DOCS_MAIN.mdoc"),
    )
    .expect("nested generated Markdown rule resolves");
    let deep = resolve_metric_rule(
        &policy,
        MetricId::RepositoryDocumentCharacters,
        &PolicyContext::new().with_path("docs/reference/DOCS_MAIN.mdoc"),
    )
    .expect("deep generated Markdown rule resolves");
    let unlimited = resolve_metric_rule(
        &policy,
        MetricId::RepositoryDocumentCharacters,
        &PolicyContext::new().with_path("docs/reference/generated/DOCS_MAIN.mdoc"),
    )
    .expect("very deep generated Markdown rule resolves");
    assert_eq!(
        (root.base_limit(), nested.base_limit(), deep.base_limit()),
        (10_000, 20_000, 30_000)
    );
    assert!(unlimited.unlimited());

    let wrong_stem = resolve_metric_rule(
        &policy,
        MetricId::RepositoryDocumentCharacters,
        &PolicyContext::new().with_path("guide.mdoc"),
    )
    .expect("nonmatching stem uses global policy");
    assert_eq!(wrong_stem.base_limit(), 1000);
    assert!(wrong_stem.binding().rule_id().is_none());
}

#[test]
fn missing_rule_context_refuses_instead_of_selecting_a_global_default() {
    let policy = migrate_predecessor_policy("fixtures/metrics.toml", SCOPED_POLICY)
        .expect("predecessor policy migrates once");
    let error = resolve_metric_rule(&policy, MetricId::FileLines, &PolicyContext::new())
        .expect_err("path-constrained rule requires path context");
    assert!(error.to_string().contains("requires path context"));
}

#[test]
fn bounded_metrics_cross_current_state_with_every_latch_transition_and_actor_variation() {
    let policy = migrate_predecessor_policy("fixtures/metrics.toml", SCOPED_POLICY)
        .expect("predecessor policy migrates once");
    let fixture = MetricStateFixture::new();

    let latched = reconcile_file_state(&fixture, &policy, STATE_LATCH_VALUE, STATE_TEST_CLOCK);
    let latch = measured_latch(&latched);
    assert_eq!(latch.transition(), "latched");
    assert_eq!(latch.configured_ratchet_limit(), 40);
    assert_eq!(latch.variation_percent(), 7);
    assert_eq!(latch.ratchet_limit(), 41);
    assert_eq!(latch.active_limit(), 41);
    assert_eq!(latch.actor_source(), Some("active-actor"));
    assert_eq!(latch.actor(), Some("actor-a"));
    assert!(latch.over_limit());
    assert!(latch.over_base());
    assert!(latch.ratchet_breach());

    let held = reconcile_file_state(&fixture, &policy, STATE_HELD_VALUE, STATE_TEST_CLOCK + 1.0);
    let latch = measured_latch(&held);
    assert_eq!(latch.transition(), "held");
    assert_eq!(latch.active_limit(), STATE_CLEAR_VALUE);
    assert!(latch.over_limit());
    assert!(latch.over_base());
    assert!(!latch.ratchet_breach());

    let healed = reconcile_file_state(&fixture, &policy, STATE_CLEAR_VALUE, STATE_TEST_CLOCK + 2.0);
    let latch = measured_latch(&healed);
    assert_eq!(latch.transition(), "healed");
    assert!(!latch.over_limit());
    assert!(!latch.over_base());

    let clear = reconcile_file_state(&fixture, &policy, STATE_CLEAR_VALUE, STATE_TEST_CLOCK + 3.0);
    let latch = measured_latch(&clear);
    assert_eq!(latch.transition(), "clear");
    assert_eq!(latch.active_limit(), 41);
}

#[test]
fn state_binding_preserves_function_keys_and_unlimited_document_rules() {
    let policy = migrate_predecessor_policy("fixtures/metrics.toml", SCOPED_POLICY)
        .expect("predecessor policy migrates once");
    let function_rule = resolve_metric_rule(
        &policy,
        MetricId::RoutineCcn,
        &PolicyContext::new().with_path("src/app.rs"),
    )
    .expect("function rule resolves");
    let parser_error = MetricStateBinding::new(
        MetricId::RoutineCcn,
        MetricKey::function("src/app.rs", "run").unwrap(),
        5,
        ParserEvidence::Native {
            parser: ParserId::Utf8FileShapeV1,
        },
        function_rule.clone(),
    )
    .expect_err("a foreign parser cannot enter bounded state");
    assert!(parser_error.to_string().contains("parser evidence"));
    let function = MetricStateBinding::new(
        MetricId::RoutineCcn,
        MetricKey::function("src/app.rs", "run").unwrap(),
        5,
        ParserEvidence::External {
            parser: ParserId::LizardCsvV1,
            tool: "lizard".to_owned(),
            version: "1.23.0".to_owned(),
            executable_digest: "sha256:fixture".to_owned(),
        },
        function_rule,
    )
    .expect("function measurement binds");
    assert_eq!(function.entry().kind(), LatchKind::ComplexityCcn);
    assert_eq!(function.entry().key().repo_path(), "src/app.rs");
    assert_eq!(function.entry().key().function_name(), Some("run"));
    function
        .observation(ActorInputs::new("src/app.rs").unwrap())
        .expect("function observation crosses current state");

    let document_path = "docs/reference/generated/DOCS_MAIN.mdoc";
    let document_rule = resolve_metric_rule(
        &policy,
        MetricId::RepositoryDocumentCharacters,
        &PolicyContext::new().with_path(document_path),
    )
    .expect("unlimited document rule resolves");
    let binding = MetricStateBinding::new(
        MetricId::RepositoryDocumentCharacters,
        MetricKey::path(document_path).unwrap(),
        FIXTURE_BASE_LIMIT,
        ParserEvidence::Native {
            parser: ParserId::Utf8DocumentCharactersV1,
        },
        document_rule,
    )
    .expect("document measurement binds");
    let fixture = MetricStateFixture::new();
    let row = reconcile_binding(&fixture, &binding, document_path, STATE_TEST_CLOCK);
    let latch = measured_latch(&row);
    assert!(latch.unlimited());
    assert_eq!(latch.variation_percent(), 7);
    assert!(!latch.over_limit());
    assert!(!latch.over_base());
    assert!(!latch.ratchet_breach());
}

#[test]
fn state_binding_preserves_the_current_u64_variation_domain() {
    const ABOVE_U32: u64 = 4_294_967_296;
    let policy = migrate_predecessor_policy(
        "fixtures/large-variation.toml",
        "[policy.flex]\nratio = 2.0\njitter_percent = 4294967296\n",
    )
    .expect("large predecessor variation migrates");
    let rule = resolve_metric_rule(
        &policy,
        MetricId::FileLines,
        &PolicyContext::new().with_path("src/lib.rs"),
    )
    .expect("large variation rule resolves");
    assert_eq!(rule.variation_percent(), ABOVE_U32);
    let binding = MetricStateBinding::new(
        MetricId::FileLines,
        MetricKey::path("src/lib.rs").expect("metric path"),
        1,
        ParserEvidence::Native {
            parser: ParserId::Utf8FileShapeV1,
        },
        rule,
    )
    .expect("the complete policy percentage binds to current state");
    let observation = binding
        .observation(
            ActorInputs::new("src/lib.rs")
                .expect("actor path")
                .with_active_actor("actor-a"),
        )
        .expect("large variation observation");

    let fixture = MetricStateFixture::new();
    let mut state = CurrentState::load(&fixture.destination, &fixture.sources)
        .expect("load current metric state");
    state
        .begin_run(&RenameMap::default(), ["src/lib.rs"], STATE_TEST_CLOCK)
        .expect("begin metric-state run");
    let decisions = state
        .reconcile([observation])
        .expect("reconcile large variation observation");
    assert_eq!(decisions[0].bound().variation_percent(), ABOVE_U32);
    let evidence = binding
        .evidence(&decisions[0])
        .expect("large variation decision binds to metric evidence");
    assert_eq!(measured_latch(&evidence).variation_percent(), ABOVE_U32);
    let json = serde_json::to_value(&evidence).expect("metric evidence serializes");
    assert_eq!(
        json["state"]["measurement"]["latch"]["variation_percent"],
        ABOVE_U32
    );
}

#[test]
fn state_binding_refuses_a_decision_from_another_variation_policy() {
    const ABOVE_U32: u64 = 4_294_967_296;
    let large_policy = migrate_predecessor_policy(
        "fixtures/large-variation.toml",
        "[policy.flex]\nratio = 2.0\njitter_percent = 4294967296\n",
    )
    .expect("large predecessor variation migrates");
    let small_policy = migrate_predecessor_policy(
        "fixtures/small-variation.toml",
        "[policy.flex]\nratio = 2.0\njitter_percent = 7\n",
    )
    .expect("small predecessor variation migrates");
    let context = PolicyContext::new().with_path("src/lib.rs");
    let large_rule = resolve_metric_rule(&large_policy, MetricId::FileLines, &context)
        .expect("large variation rule resolves");
    let small_rule = resolve_metric_rule(&small_policy, MetricId::FileLines, &context)
        .expect("small variation rule resolves");
    assert_eq!(large_rule.variation_percent(), ABOVE_U32);
    assert_eq!(small_rule.variation_percent(), 7);

    let key = MetricKey::path("src/lib.rs").expect("metric path");
    let parser = ParserEvidence::Native {
        parser: ParserId::Utf8FileShapeV1,
    };
    let large_binding = MetricStateBinding::new(
        MetricId::FileLines,
        key.clone(),
        1,
        parser.clone(),
        large_rule,
    )
    .expect("large variation binding");
    let small_binding = MetricStateBinding::new(MetricId::FileLines, key, 1, parser, small_rule)
        .expect("small variation binding");
    let observation = small_binding
        .observation(
            ActorInputs::new("src/lib.rs")
                .expect("actor path")
                .with_active_actor("actor-a"),
        )
        .expect("small variation observation");

    let fixture = MetricStateFixture::new();
    let mut state = CurrentState::load(&fixture.destination, &fixture.sources)
        .expect("load current metric state");
    state
        .begin_run(&RenameMap::default(), ["src/lib.rs"], STATE_TEST_CLOCK)
        .expect("begin metric-state run");
    let decisions = state
        .reconcile([observation])
        .expect("reconcile small variation observation");
    let error = large_binding
        .evidence(&decisions[0])
        .expect_err("another variation policy cannot supply metric evidence");
    assert!(error.to_string().contains("resolved rule"));
}

#[test]
fn evidence_vocabulary_is_total_and_release_owned() {
    let states = [
        EvidenceState::Unavailable {
            reason: "lizard executable absent".to_owned(),
        },
        EvidenceState::predecessor_unsupported(
            "policy.languages.magic",
            "Python AST classification is not compiled",
            "remove .py or compile an exact parser",
        ),
        EvidenceState::Unmeasured {
            reason: "selected bytes were not acquired".to_owned(),
        },
        EvidenceState::Timeout {
            milliseconds: 30_000,
            reason: "bounded lizard process exceeded runtime".to_owned(),
        },
        EvidenceState::Failed {
            reason: "lizard exited without complete CSV".to_owned(),
            exit_code: Some(2),
        },
        EvidenceState::NotApplicable {
            reason: "no tracked Markdown sources".to_owned(),
        },
    ];
    for metric in MetricId::ALL {
        for (index, state) in states.iter().cloned().enumerate() {
            let row = MetricEvidence::new(metric, MetricKey::Arm, state)
                .expect("every metric admits every explicit nonmeasurement state");
            assert_eq!(row.state().is_failure(), index < 5);
            assert_eq!(row.state().is_skip(), index == 5);
        }
        let row = measured_evidence(metric);
        assert!(matches!(row.state(), EvidenceState::Measured { .. }));
    }

    let converted = ParseError::Unsupported {
        source_field: "policy.languages.magic".to_owned(),
        reason: "parser class is absent".to_owned(),
        recovery: "compile the parser".to_owned(),
    }
    .into_evidence_state();
    assert!(matches!(
        converted,
        EvidenceState::Unsupported { owning_release, .. } if owning_release == "0.31.0"
    ));
}

#[test]
fn parity_proves_supported_values_and_exact_release_owned_refusals() {
    let key = MetricKey::path("src/app.rs").unwrap();
    let current = MetricEvidence::new(
        MetricId::FileLines,
        key.clone(),
        EvidenceState::measured(bounded_measurement(MetricId::FileLines, 12, "file_loc")),
    )
    .unwrap();
    let predecessor = PredecessorMetricObservation::new(
        MetricId::FileLines,
        key,
        PredecessorMetricResult::Measured(12),
    );
    assert!(matches!(
        compare_predecessor_metric(&predecessor, &current).unwrap(),
        MetricParity::Equivalent {
            value: Some(12),
            ..
        }
    ));

    let key = MetricKey::path("src/app.py").unwrap();
    let refused = MetricEvidence::new(
        MetricId::MagicNumberRegressions,
        key.clone(),
        EvidenceState::predecessor_unsupported(
            "policy.languages.magic",
            "Python AST classification is not compiled",
            "remove .py or compile an exact parser",
        ),
    )
    .unwrap();
    let predecessor = PredecessorMetricObservation::new(
        MetricId::MagicNumberRegressions,
        key,
        PredecessorMetricResult::Measured(1),
    );
    assert!(matches!(
        compare_predecessor_metric(&predecessor, &refused).unwrap(),
        MetricParity::Refused {
            source_field,
            owning_release,
            ..
        } if source_field == "policy.languages.magic" && owning_release == "0.31.0"
    ));
}

#[test]
fn reports_require_every_arm_metric_and_paired_scope_key() {
    let arms = vec![
        not_applicable_arm(GateId::MarkdownLinks),
        not_applicable_arm(GateId::FileShape),
        not_applicable_arm(GateId::MagicNumbers),
        not_applicable_arm(GateId::Complexity),
    ];
    let documents = vec![
        MetricEvidence::new(
            MetricId::RepositoryDocumentCharacters,
            MetricKey::Arm,
            EvidenceState::NotApplicable {
                reason: "no repository-truth documents".to_owned(),
            },
        )
        .unwrap(),
    ];
    let report = MetricReport::new(arms, documents).expect("complete report admits");
    assert_eq!(
        report
            .arms()
            .iter()
            .map(|arm| arm.gate_id())
            .collect::<Vec<_>>(),
        STRUCTURAL_ARMS.map(|arm| arm.id())
    );
    assert_eq!(report.canonical_json(), report.canonical_json());

    let line = MetricEvidence::new(
        MetricId::FileLines,
        MetricKey::path("src/a.rs").unwrap(),
        EvidenceState::measured(bounded_measurement(MetricId::FileLines, 12, "file_loc")),
    )
    .unwrap();
    let bytes = MetricEvidence::new(
        MetricId::FileBytes,
        MetricKey::path("src/b.rs").unwrap(),
        EvidenceState::measured(bounded_measurement(MetricId::FileBytes, 100, "file_bytes")),
    )
    .unwrap();
    assert!(
        StructuralArmEvidence::new(GateId::FileShape, vec![line, bytes])
            .expect_err("file metrics must cover identical path keys")
            .to_string()
            .contains("metric keys differ")
    );
}

fn not_applicable_arm(gate: GateId) -> StructuralArmEvidence {
    let declaration = STRUCTURAL_ARMS
        .into_iter()
        .find(|arm| arm.id() == gate)
        .unwrap();
    let evidence = declaration
        .metrics()
        .iter()
        .map(|metric| {
            MetricEvidence::new(
                *metric,
                MetricKey::Arm,
                EvidenceState::NotApplicable {
                    reason: "declared scope has no selected input".to_owned(),
                },
            )
            .unwrap()
        })
        .collect();
    StructuralArmEvidence::new(gate, evidence).unwrap()
}

fn bounded_measurement(metric: MetricId, value: u64, field: &str) -> MetricMeasurement {
    let parser = match metric {
        MetricId::FileLines | MetricId::FileBytes => ParserId::Utf8FileShapeV1,
        MetricId::RoutineCcn | MetricId::RoutineLength => ParserId::LizardCsvV1,
        MetricId::RepositoryDocumentCharacters => ParserId::Utf8DocumentCharactersV1,
        _ => unreachable!("fixture helper only builds bounded measurements"),
    };
    let parser = if parser == ParserId::LizardCsvV1 {
        ParserEvidence::External {
            parser,
            tool: "lizard".to_owned(),
            version: "1.23.0".to_owned(),
            executable_digest: "sha256:fixture".to_owned(),
        }
    } else {
        ParserEvidence::Native { parser }
    };
    bounded_measurement_with_parser(metric, value, field, parser)
}

fn bounded_measurement_with_parser(
    metric: MetricId,
    value: u64,
    field: &str,
    parser: ParserEvidence,
) -> MetricMeasurement {
    let kind = match metric {
        MetricId::FileLines => "file_loc",
        MetricId::FileBytes => "file_bytes",
        MetricId::RoutineCcn => "complexity_ccn",
        MetricId::RoutineLength => "complexity_length",
        MetricId::RepositoryDocumentCharacters => "repo_doc_chars",
        _ => unreachable!("fixture helper only builds bounded measurements"),
    };
    MetricMeasurement::new(value, parser)
        .with_rule(
            RuleBinding::new(
                field,
                None::<String>,
                "compiled policy globals",
                "no named rule selected",
            )
            .unwrap(),
        )
        .with_latch(
            LatchBinding::new(
                kind,
                FIXTURE_BASE_LIMIT,
                FIXTURE_RATCHET_LIMIT,
                0,
                FIXTURE_RATCHET_LIMIT,
                FIXTURE_RATCHET_LIMIT,
                false,
                false,
                false,
                false,
                "clear",
                None::<String>,
                None::<String>,
            )
            .unwrap(),
        )
}

fn assert_parser_refused_by_both(
    metric: MetricId,
    key: MetricKey,
    value: u64,
    field: &str,
    parser: ParserEvidence,
    rule: &ResolvedMetricRule,
) {
    let evidence_error = MetricEvidence::new(
        metric,
        key.clone(),
        EvidenceState::measured(bounded_measurement_with_parser(
            metric,
            value,
            field,
            parser.clone(),
        )),
    )
    .expect_err("mismatched parser evidence cannot become metric evidence");
    assert!(evidence_error.to_string().contains("parser evidence"));

    let binding_error = MetricStateBinding::new(metric, key, value, parser, rule.clone())
        .expect_err("mismatched parser evidence cannot enter state binding");
    assert!(binding_error.to_string().contains("parser evidence"));
}

fn measured_evidence(metric: MetricId) -> MetricEvidence {
    let key = match metric {
        MetricId::RoutineCcn | MetricId::RoutineLength => {
            MetricKey::function("src/app.rs", "run").unwrap()
        }
        _ => MetricKey::path("src/app.rs").unwrap(),
    };
    let measurement = match metric {
        MetricId::FileLines => bounded_measurement(metric, 12, "file_loc"),
        MetricId::FileBytes => bounded_measurement(metric, 100, "file_bytes"),
        MetricId::RoutineCcn => bounded_measurement(metric, 5, "routine_ccn"),
        MetricId::RoutineLength => bounded_measurement(metric, 17, "routine_length"),
        MetricId::RepositoryDocumentCharacters => {
            bounded_measurement(metric, 100, "repo_truth_doc_chars")
        }
        MetricId::MagicNumberRegressions => MetricMeasurement::new(
            1,
            ParserEvidence::Native {
                parser: ParserId::CGrammarMagicDeltaV1,
            },
        )
        .with_rule(
            RuleBinding::new(
                "magic.examine_threshold",
                None::<String>,
                "compiled policy globals",
                "no named rule selected",
            )
            .unwrap(),
        )
        .with_threshold(10),
        MetricId::MarkdownLinkCaseMismatches => MetricMeasurement::new(
            1,
            ParserEvidence::Native {
                parser: ParserId::MarkdownInlineLinkCaseV1,
            },
        ),
    };
    MetricEvidence::new(metric, key, EvidenceState::measured(measurement)).unwrap()
}

struct MetricStateFixture {
    root: PathBuf,
    destination: PathBuf,
    sources: PredecessorSources,
}

impl MetricStateFixture {
    fn new() -> Self {
        let serial = NEXT_STATE_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("gate-metric-state-{}-{serial}", std::process::id()));
        fs::create_dir_all(&root).expect("create metric-state fixture root");
        let sources = PredecessorSources::new(
            &root.join("predecessor-worktree"),
            &root.join("predecessor-shared"),
        );
        let destination = root.join("current/gate-state.json");
        Self {
            root,
            destination,
            sources,
        }
    }
}

impl Drop for MetricStateFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn reconcile_file_state(
    fixture: &MetricStateFixture,
    policy: &RulePolicy,
    value: u64,
    now: f64,
) -> MetricEvidence {
    let rule = resolve_metric_rule(
        policy,
        MetricId::FileLines,
        &PolicyContext::new().with_path("src/app.rs"),
    )
    .expect("file rule resolves");
    let binding = MetricStateBinding::new(
        MetricId::FileLines,
        MetricKey::path("src/app.rs").unwrap(),
        value,
        ParserEvidence::Native {
            parser: ParserId::Utf8FileShapeV1,
        },
        rule,
    )
    .expect("file measurement binds");
    reconcile_binding(fixture, &binding, "src/app.rs", now)
}

fn reconcile_binding(
    fixture: &MetricStateFixture,
    binding: &MetricStateBinding,
    path: &str,
    now: f64,
) -> MetricEvidence {
    let actors = ActorInputs::new(path)
        .expect("actor inputs")
        .with_active_actor("actor-a");
    let observation = binding.observation(actors).expect("state observation");
    let mut state = CurrentState::load(&fixture.destination, &fixture.sources)
        .expect("load current metric state");
    state
        .begin_run(&RenameMap::default(), [path], now)
        .expect("begin metric-state run");
    let decisions = state
        .reconcile([observation])
        .expect("reconcile metric state");
    let evidence = binding
        .evidence(&decisions[0])
        .expect("bind reconciled evidence");
    state
        .finish()
        .expect("finish metric-state run")
        .accept()
        .expect("persist accepted metric state");
    evidence
}

fn measured_latch(evidence: &MetricEvidence) -> &LatchBinding {
    let EvidenceState::Measured { measurement } = evidence.state() else {
        panic!("fixture produced nonmeasurement evidence")
    };
    measurement.latch().expect("bounded evidence carries latch")
}
