use gate::{
    BoundName, PolicyContext, PolicyDocument, PolicyMigrationError, RulePolicy, RuleSourceKind,
    ScopeAxis, migrate_predecessor_policy,
};

const OVERLAPPING: &str = r#"
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
routine_ccn = 12

[policy.magic]
examine_threshold = 10
baseline_ref = "HEAD"

[policy.markdown_depth_budget]
extensions = []

[[policy.rules]]
scopes = { paths = ["src/**"] }
multiplier = 2.0
flex = 2.0

[[policy.rules]]
scopes = { paths = ["src/legacy/**"] }

[policy.rules.file_loc]
multiplier = 3.0
min = 35
max = 40
flex = 1.5

[policy.rules.magic]
examine_threshold = 100

[[policy.rules]]
scopes = { paths = ["Docs"], extensions = [".md"] }

[policy.rules.file_loc]
min = 25
max = 25
flex = 1.0

[policy.rules.routine_length]
unlimited = true
"#;

const EVERY_SCOPE: &str = r#"
regex_dialect = "rust-regex-1"
magic_examine_threshold = 10

[limits]
file_loc = 10
file_bytes = 100
routine_ccn = 5
routine_length = 8
commit_message_wrap = 72
repo_truth_doc_chars = 1000

[ratchet]
ratio = 1.5
variation_percent = 5

[[rules]]
id = "path-rule"
source_order = 0
priority = 1
scopes = { paths = ["src/**"] }
[rules.source]
location = "current.toml"
field = "rules.path-rule"
kind = "current"
[rules.bounds.file_loc]
multiplier = 2.0

[[rules]]
id = "extension-rule"
source_order = 1
priority = 1
scopes = { extensions = [".RS"] }
[rules.source]
location = "current.toml"
field = "rules.extension-rule"
kind = "current"
[rules.bounds.file_bytes]
multiplier = 2.0

[[rules]]
id = "vendor-rule"
source_order = 2
priority = 1
scopes = { vendors = ["CODEX"] }
[rules.source]
location = "current.toml"
field = "rules.vendor-rule"
kind = "current"
[rules.bounds.routine_ccn]
multiplier = 2.0

[[rules]]
id = "model-rule"
source_order = 3
priority = 1
scopes = { models = ["GPT 5.5"] }
[rules.source]
location = "current.toml"
field = "rules.model-rule"
kind = "current"
[rules.bounds.routine_length]
multiplier = 2.0

[[rules]]
id = "phase-rule"
source_order = 4
priority = 1
scopes = { phases = ["pre_commit_success"] }
[rules.source]
location = "current.toml"
field = "rules.phase-rule"
kind = "current"
[rules.bounds.commit_message_wrap]
multiplier = 2.0
"#;

#[test]
fn migration_assigns_stable_ids_and_preserves_selection_and_bound_order() {
    let policy = migrate_predecessor_policy("fixtures/policy.toml", OVERLAPPING)
        .expect("legacy policy migrates");

    assert_eq!(policy.document().rules().len(), 3);
    assert_eq!(policy.document().ratchet().variation_percent(), 7);
    assert_eq!(
        policy.document().rules()[1].id().as_str(),
        "legacy-rule:fixtures%2fpolicy.toml:policy.rules%5b2%5d"
    );
    assert_eq!(policy.document().rules()[1].source_order(), 1);
    assert_eq!(
        policy.document().rules()[1].source().kind(),
        RuleSourceKind::PredecessorList
    );

    let legacy = policy
        .resolve_bound(
            BoundName::FileLoc,
            &PolicyContext::new().with_path("src/legacy/app.rs"),
        )
        .expect("specific bound resolves");
    assert_eq!(legacy.limit(), 35);
    assert_eq!(legacy.ratchet_limit(), 52);
    assert!(!legacy.unlimited());
    let evidence = legacy.selection().expect("specific rule selected");
    assert_eq!(
        evidence.rule_id().as_str(),
        "legacy-rule:fixtures%2fpolicy.toml:policy.rules%5b2%5d"
    );
    assert_eq!(evidence.metric(), "file_loc");
    assert_eq!(evidence.priority(), 1);
    assert_eq!(evidence.source_order(), 1);
    assert_eq!(evidence.source().field(), "policy.rules[2]");
    assert!(evidence.match_basis().contains("src/legacy/**"));
    assert_eq!(evidence.specificity().constrained_axes(), 1);

    let flat = policy
        .resolve_bound(
            BoundName::FileBytes,
            &PolicyContext::new().with_path("src/app.rs"),
        )
        .expect("flat rule resolves");
    assert_eq!((flat.limit(), flat.ratchet_limit()), (200, 400));
    assert_eq!(flat.selection().expect("flat selection").source_order(), 0);

    let global = policy
        .resolve_bound(
            BoundName::FileBytes,
            &PolicyContext::new().with_path("other/app.rs"),
        )
        .expect("global bound resolves");
    assert_eq!((global.limit(), global.ratchet_limit()), (100, 200));
    assert!(global.selection().is_none());

    let global_override = policy
        .resolve_bound(
            BoundName::RoutineCcn,
            &PolicyContext::new().with_path("other/app.rs"),
        )
        .expect("global per-bound ratchet resolves");
    assert_eq!(
        (global_override.limit(), global_override.ratchet_limit()),
        (5, 12)
    );
    assert!(global_override.selection().is_none());

    let unlimited = policy
        .resolve_bound(
            BoundName::RoutineLength,
            &PolicyContext::new().with_path("Docs/guide.md"),
        )
        .expect("per-bound unlimited resolves");
    assert!(unlimited.unlimited());
    assert_eq!(
        unlimited
            .selection()
            .expect("unlimited selection")
            .source_order(),
        2
    );

    let magic = policy
        .resolve_magic(&PolicyContext::new().with_path("src/legacy/app.rs"))
        .expect("magic resolves");
    assert_eq!(magic.metric(), "magic.examine_threshold");
    assert_eq!(magic.examine_threshold(), 100);
    assert_eq!(magic.selection().expect("magic rule").source_order(), 1);

    let global_magic = policy
        .resolve_magic(&PolicyContext::new().with_path("other/app.rs"))
        .expect("global magic resolves");
    assert_eq!(global_magic.examine_threshold(), 10);
    assert!(global_magic.selection().is_none());
}

#[test]
fn generated_markdown_rules_retain_regex_one_letter_depth_and_priority() {
    let policy = migrate_predecessor_policy(
        "tracked/spice.toml",
        r#"
        [policy.limits]
        repo_truth_doc_chars = 1000

        [policy.markdown_depth_budget]
        extensions = [".mdoc"]
        stem_pattern = "[A-Z_]+"

        [policy.markdown_depth]
        base_chars = 7000
        max_bounded_chars = 15000
        "#,
    )
    .expect("generated policy migrates");

    assert_eq!(policy.document().rules().len(), 4);
    assert!(
        policy
            .document()
            .rules()
            .iter()
            .all(|rule| rule.priority() == 0)
    );
    assert!(
        policy
            .document()
            .rules()
            .iter()
            .all(|rule| { rule.source().kind() == RuleSourceKind::GeneratedMarkdownDepth })
    );

    let root = resolve_doc(&policy, "DOCS_MAIN.mdoc");
    assert_eq!((root.limit(), root.ratchet_limit()), (7000, 10500));
    assert!(
        root.selection()
            .expect("generated root selection")
            .match_basis()
            .contains("dialect=rust-regex-1")
    );

    let nested = resolve_doc(&policy, "docs/DOCS_MAIN.mdoc");
    assert_eq!(nested.limit(), 14000);
    let deep = resolve_doc(&policy, "docs/reference/DOCS_MAIN.mdoc");
    assert_eq!(deep.limit(), 15000);
    let unbounded = resolve_doc(&policy, "docs/reference/generated/DOCS_MAIN.mdoc");
    assert!(unbounded.unlimited());

    for path in ["A.mdoc", "guide.mdoc", "DOCS_MAIN.md"] {
        let fallback = resolve_doc(&policy, path);
        assert_eq!(fallback.limit(), 1000, "{path}");
        assert!(fallback.selection().is_none(), "{path}");
    }
}

#[test]
fn repository_document_selection_unifies_explicit_paths_and_generated_depth_rules() {
    let policy = migrate_predecessor_policy(
        "tracked/spice.toml",
        r#"
        [policy.repo_truth]
        docs = ["NOTICE.txt", "./NOTICE.txt"]

        [policy.markdown_depth_budget]
        extensions = [".md"]
        "#,
    )
    .expect("repository-document policy migrates");

    assert_eq!(policy.document().repo_truth_docs(), ["NOTICE.txt"]);
    assert!(
        policy
            .selects_repository_document("NOTICE.txt")
            .expect("explicit path resolves")
    );
    assert!(
        policy
            .selects_repository_document("docs/guide.md")
            .expect("generated markdown path resolves")
    );
    assert!(
        !policy
            .selects_repository_document("src/lib.rs")
            .expect("ordinary path resolves")
    );
}

#[test]
fn current_named_rules_cover_every_scope_dimension_with_provenance() {
    let document: PolicyDocument = toml::from_str(EVERY_SCOPE).expect("current policy decodes");
    let encoded = serde_json::to_value(&document).expect("current policy serializes");
    let decoded = serde_json::from_value(encoded).expect("current policy round trips");
    let policy = RulePolicy::compile(decoded).expect("current policy compiles");

    let cases = [
        (
            BoundName::FileLoc,
            PolicyContext::new().with_path("src/lib.rs"),
            "path-rule",
            "paths=",
        ),
        (
            BoundName::FileBytes,
            PolicyContext::new().with_path("src/lib.rs"),
            "extension-rule",
            "extension=",
        ),
        (
            BoundName::RoutineCcn,
            PolicyContext::new().with_vendor("Codex"),
            "vendor-rule",
            "vendor=",
        ),
        (
            BoundName::RoutineLength,
            PolicyContext::new().with_model("gpt 5.5"),
            "model-rule",
            "model=",
        ),
        (
            BoundName::CommitMessageWrap,
            PolicyContext::new().with_phase("pre-commit-success"),
            "phase-rule",
            "phase=",
        ),
    ];
    for (metric, context, id, basis) in cases {
        let resolved = policy
            .resolve_bound(metric, &context)
            .expect("scope resolves");
        let evidence = resolved.selection().expect("scope selects a rule");
        assert_eq!(evidence.rule_id().as_str(), id);
        assert_eq!(evidence.metric(), metric.as_str());
        assert!(evidence.match_basis().contains(basis));
        assert_eq!(evidence.source().kind(), RuleSourceKind::Current);
    }

    let explicit_extension = policy
        .resolve_bound(
            BoundName::FileBytes,
            &PolicyContext::new().with_extension(" .RS "),
        )
        .expect("explicit extension normalizes");
    assert_eq!(
        explicit_extension
            .selection()
            .expect("extension rule")
            .rule_id()
            .as_str(),
        "extension-rule"
    );
}

#[test]
fn missing_candidate_context_refuses_instead_of_falling_back() {
    let policy = current_policy(EVERY_SCOPE);
    let error = policy
        .resolve_bound(BoundName::RoutineCcn, &PolicyContext::new())
        .expect_err("missing vendor must refuse");

    assert_eq!(error.code(), "policy::context-missing");
    assert_eq!(error.metric(), "routine_ccn");
    assert_eq!(
        error.rule().map(ToString::to_string).as_deref(),
        Some("vendor-rule")
    );
    assert_eq!(error.dimension(), Some(ScopeAxis::Vendors));
    assert_eq!(
        error.recovery(),
        "supply PolicyContext::with_vendor before resolving routine_ccn"
    );
}

#[test]
fn resolver_order_is_priority_then_specificity_then_earlier_source() {
    let policy = current_policy(
        r#"
        regex_dialect = "rust-regex-1"
        magic_examine_threshold = 10
        [limits]
        file_loc = 10
        file_bytes = 100
        routine_ccn = 5
        routine_length = 8
        commit_message_wrap = 72
        repo_truth_doc_chars = 1000
        [ratchet]
        ratio = 1.0
        variation_percent = 5

        [[rules]]
        id = "priority-broad"
        source_order = 0
        priority = 2
        scopes = { paths = ["src/**"] }
        [rules.source]
        location = "current.toml"
        field = "rules.priority-broad"
        kind = "current"
        [rules.bounds.file_loc]
        multiplier = 2.0

        [[rules]]
        id = "specific-low"
        source_order = 1
        priority = 1
        scopes = { paths = ["src/legacy/app.rs"] }
        [rules.source]
        location = "current.toml"
        field = "rules.specific-low"
        kind = "current"
        [rules.bounds.file_loc]
        multiplier = 3.0

        [[rules]]
        id = "specific-first"
        source_order = 2
        priority = 1
        scopes = { paths = ["src/**"] }
        [rules.source]
        location = "generated"
        field = "generated.first"
        kind = "generated-markdown-depth"
        [rules.bounds.file_bytes]
        multiplier = 2.0

        [[rules]]
        id = "specific-second"
        source_order = 3
        priority = 1
        scopes = { paths = ["src/**"] }
        [rules.source]
        location = "generated"
        field = "generated.second"
        kind = "generated-markdown-depth"
        [rules.bounds.file_bytes]
        multiplier = 3.0

        [[rules]]
        id = "specific-exact"
        source_order = 4
        priority = 1
        scopes = { paths = ["docs/guide.rs"] }
        [rules.source]
        location = "current.toml"
        field = "rules.specific-exact"
        kind = "current"
        [rules.bounds.routine_length]
        multiplier = 3.0

        [[rules]]
        id = "specific-glob"
        source_order = 5
        priority = 1
        scopes = { paths = ["docs/**"] }
        [rules.source]
        location = "current.toml"
        field = "rules.specific-glob"
        kind = "current"
        [rules.bounds.routine_length]
        multiplier = 2.0
        "#,
    );

    let priority = policy
        .resolve_bound(
            BoundName::FileLoc,
            &PolicyContext::new().with_path("src/legacy/app.rs"),
        )
        .expect("priority resolves");
    assert_eq!(
        priority
            .selection()
            .expect("priority selection")
            .rule_id()
            .as_str(),
        "priority-broad"
    );

    let source = policy
        .resolve_bound(
            BoundName::FileBytes,
            &PolicyContext::new().with_path("src/app.rs"),
        )
        .expect("source order resolves");
    assert_eq!(source.limit(), 200);
    assert_eq!(
        source
            .selection()
            .expect("source selection")
            .rule_id()
            .as_str(),
        "specific-first"
    );

    let specific = policy
        .resolve_bound(
            BoundName::RoutineLength,
            &PolicyContext::new().with_path("docs/guide.rs"),
        )
        .expect("specificity resolves");
    assert_eq!(
        specific
            .selection()
            .expect("specific selection")
            .rule_id()
            .as_str(),
        "specific-exact"
    );
}

#[test]
fn predecessor_path_matching_keeps_glob_crossing_and_root_double_star() {
    let policy = migrate_predecessor_policy(
        "path-policy.toml",
        r#"
        [policy.markdown_depth_budget]
        extensions = []

        [[policy.rules]]
        scopes = { paths = ["src/*.rs"] }
        [policy.rules.file_loc]
        multiplier = 2.0

        [[policy.rules]]
        scopes = { paths = ["**/*.cs"] }
        [policy.rules.magic]
        examine_threshold = 77

        [[policy.rules]]
        scopes = { paths = ["Docs/"] }
        [policy.rules.routine_ccn]
        multiplier = 2.0
        "#,
    )
    .expect("path policy migrates");

    assert!(
        policy
            .resolve_bound(
                BoundName::FileLoc,
                &PolicyContext::new().with_path("src/deep/app.rs"),
            )
            .expect("cross-directory star resolves")
            .selection()
            .is_some()
    );
    assert!(
        policy
            .resolve_bound(
                BoundName::FileLoc,
                &PolicyContext::new().with_path("other/src/deep/app.rs"),
            )
            .expect("glob remains anchored at the repository root")
            .selection()
            .is_none()
    );
    assert_eq!(
        policy
            .resolve_magic(&PolicyContext::new().with_path("Widget.cs"))
            .expect("leading double-star resolves at root")
            .examine_threshold(),
        77
    );
    assert!(
        policy
            .resolve_bound(
                BoundName::RoutineCcn,
                &PolicyContext::new().with_path("Docs/reference/page.md"),
            )
            .expect("trailing-slash subtree resolves")
            .selection()
            .is_some()
    );
}

#[test]
fn migration_diagnostics_name_the_exact_source_field() {
    let cases = [
        (
            concat!(
                "[policy.markdown_depth_budget]\nextensions = []\n",
                "[[policy.rules]]\nscopes = { paths = [\"src\"] }\n",
                "[policy.rules.file_loc]\nmultiplier = -1.0\n"
            ),
            "policy.rules[1].file_loc.multiplier",
            "finite number >= 0.0",
        ),
        (
            concat!(
                "[policy.markdown_depth_budget]\nextensions = []\n",
                "[[policy.rules]]\nscopes = { paths = [\"src\"] }\n",
                "[policy.rules.file_loc]\nmin = 20\nmax = 10\n"
            ),
            "policy.rules[1].file_loc",
            "min must be <= max",
        ),
        (
            concat!(
                "[policy.markdown_depth_budget]\nextensions = []\n",
                "[[policy.rules]]\nscopes = { drivers = [\"codex\"] }\n",
                "unlimited = true\n"
            ),
            "policy.rules[1].scopes.drivers",
            "support only paths and extensions",
        ),
        (
            concat!(
                "[policy.markdown_depth_budget]\nextensions = []\n",
                "[[policy.rules]]\nscopes = { paths = [] }\n",
                "unlimited = true\n"
            ),
            "policy.rules[1].scopes.paths",
            "non-empty list",
        ),
        (
            "[policy.markdown_depth_budget]\nstem_pattern = \"(?<=DOC)S\"\n",
            "policy.markdown_depth_budget.stem_pattern",
            "unsupported by rust-regex-1",
        ),
        (
            "[policy.markdown_depth_budget]\nextensions = [\".tar.gz\"]\n",
            "policy.markdown_depth_budget.extensions[1]",
            "one ASCII final suffix",
        ),
    ];
    for (text, field, reason) in cases {
        let error = migrate_predecessor_policy("fixture.toml", text)
            .expect_err("malformed predecessor input must refuse");
        assert_migration_error(&error, field, reason);
    }
}

#[test]
fn disabled_markdown_generation_still_validates_the_stem_dialect() {
    for text in [
        concat!(
            "[policy.markdown_depth_budget]\nextensions = []\n",
            "stem_pattern = \"(?<=DOC)S\"\n"
        ),
        concat!(
            "[policy.markdown_depth_budget]\nextensions = []\n",
            "stem_pattern = \"[\"\n"
        ),
    ] {
        let error = migrate_predecessor_policy("disabled.toml", text)
            .expect_err("disabled unsupported stem pattern must refuse");
        assert_eq!(error.field(), "policy.markdown_depth_budget.stem_pattern");
        assert!(error.reason().contains("unsupported by rust-regex-1"));
        assert!(
            error
                .reason()
                .contains("rewrite it in Rust regex 1.x syntax")
        );
    }

    let policy = migrate_predecessor_policy(
        "disabled.toml",
        concat!(
            "[policy.markdown_depth_budget]\nextensions = []\n",
            "stem_pattern = \"[A-Z_]+\"\n"
        ),
    )
    .expect("disabled supported stem pattern migrates");
    assert!(policy.document().rules().is_empty());
}

#[test]
fn normalized_duplicate_selectors_and_anonymous_current_rules_are_refused() {
    let duplicate = migrate_predecessor_policy(
        "duplicate.toml",
        r#"
        [policy.markdown_depth_budget]
        extensions = []

        [[policy.rules]]
        scopes = { paths = ["./src\\legacy"] }
        unlimited = true

        [[policy.rules]]
        scopes = { paths = ["src/legacy"] }
        multiplier = 2.0
        "#,
    )
    .expect_err("normalized duplicate selector must refuse");
    assert_eq!(duplicate.source(), "duplicate.toml");
    assert_eq!(duplicate.field(), "policy.rules[2].scopes", "{duplicate}");
    assert!(
        duplicate.reason().contains("duplicates earlier selector"),
        "{duplicate}"
    );

    let anonymous = EVERY_SCOPE.replacen("id = \"path-rule\"\n", "", 1);
    let error = toml::from_str::<PolicyDocument>(&anonymous)
        .expect_err("current runtime schema requires a rule ID");
    assert!(error.to_string().contains("missing field `id`"), "{error}");

    let malformed_phase = EVERY_SCOPE.replace("pre_commit_success", "post-commit");
    let document = toml::from_str(&malformed_phase).expect("current policy decodes");
    let error = RulePolicy::compile(document).expect_err("unsupported phase must refuse");
    assert_eq!(error.field(), "rule[phase-rule].scopes.phases");
}

fn current_policy(text: &str) -> RulePolicy {
    let document = toml::from_str(text).expect("current policy decodes");
    RulePolicy::compile(document).expect("current policy compiles")
}

fn resolve_doc(policy: &RulePolicy, path: &str) -> gate::ResolvedBound {
    policy
        .resolve_bound(
            BoundName::RepoTruthDocChars,
            &PolicyContext::new().with_path(path),
        )
        .expect("document bound resolves")
}

fn assert_migration_error(error: &PolicyMigrationError, field: &str, reason: &str) {
    assert_eq!(error.source(), "fixture.toml");
    assert_eq!(error.field(), field, "{error}");
    assert!(error.reason().contains(reason), "{error}");
}
