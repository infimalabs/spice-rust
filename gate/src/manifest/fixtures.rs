use super::*;

const SYSTEM: &str = ::std::include_str!("../../tests/fixtures/gate-manifest/system.toml");
const REPOSITORY: &str = ::std::include_str!("../../tests/fixtures/gate-manifest/repository.toml");
const WORKTREE: &str = ::std::include_str!("../../tests/fixtures/gate-manifest/worktree.toml");

fn inputs() -> PredecessorInputs<'static> {
    PredecessorInputs::new("/installed/0.31.0/spice-rust.toml", SYSTEM)
}

fn migrate(repository: &str, worktree: &str) -> GateManifest {
    migrate_predecessor(
        inputs()
            .repository("/repo/spice-rust.toml", repository)
            .worktree("/git/common/spice/config.toml", worktree),
    )
    .expect("fixture migrates")
}

/// Whether release 0.31.0's battery declared no standard arm for this identity.
///
/// A gate this workspace grew past the predecessor cannot be named by any
/// predecessor document, so it is selected from the compiled registry root and
/// its refusals cannot cite a step that never ran.
fn absent_from_battery(id: GateId) -> bool {
    PREDECESSOR_STANDARD_ARMS
        .into_iter()
        .any(|(candidate, arm)| candidate == id && arm == PredecessorStandardArm::AbsentFromBattery)
}

fn declaration(manifest: &GateManifest, id: GateId) -> &GateDeclaration {
    manifest
        .gates()
        .iter()
        .find(|declaration| declaration.id() == id)
        .expect("complete manifest contains ID")
}

fn error(repository: &str, worktree: &str) -> MigrationError {
    migrate_predecessor(
        inputs()
            .repository("/repo/spice-rust.toml", repository)
            .worktree("/git/common/spice/config.toml", worktree),
    )
    .expect_err("fixture must refuse")
}

#[test]
fn current_manifest_is_exactly_the_ordered_instrument_registry() {
    let manifest = GateManifest::default();
    assert_eq!(manifest.version(), GATE_MANIFEST_VERSION);
    assert_eq!(manifest.gates().len(), 30);
    assert_eq!(
        manifest
            .gates()
            .iter()
            .map(GateDeclaration::id)
            .collect::<Vec<_>>(),
        GateId::ALL
    );
    assert!(manifest.gates().iter().all(GateDeclaration::enabled));
    assert!(
        manifest
            .gates()
            .iter()
            .all(|declaration| declaration.display().is_none())
    );
    assert!(manifest.gates().iter().all(|declaration| {
        declaration.id_provenance().layer() == OriginLayer::Compiled
            && declaration.enabled_provenance().layer() == OriginLayer::Compiled
    }));

    let rendered = manifest.render();
    let reparsed = GateManifest::parse(&rendered).expect("rendered manifest parses");
    assert_eq!(reparsed, manifest);
    assert_eq!(
        rendered
            .lines()
            .filter_map(|line| line.strip_prefix("id = \"")?.strip_suffix('"'))
            .collect::<Vec<_>>(),
        GateId::ALL.map(GateId::as_str)
    );
}

#[test]
fn ordinary_parser_rejects_predecessor_and_executable_declarations() {
    let current = GateManifest::default().render();
    for (candidate, expected) in [
        (
            current.replacen("id = \"merge-integrity\"", "id = \"merge_integrity\"", 1),
            "unknown compiled gate ID",
        ),
        (
            current.replacen(
                "id = \"canvas-web-typecheck\"",
                concat!("id = \"ser", "ve-web-typecheck\""),
                1,
            ),
            "unknown compiled gate ID",
        ),
        (
            current.replacen("id = \"merge-integrity\"\n", "", 1),
            "missing field `id`",
        ),
        (
            current.replacen("enabled = true", "run = [\"python\", \"gate.py\"]", 1),
            "unknown field `run`",
        ),
        (
            current.replacen("enabled = true", "program = \"python\"", 1),
            "unknown field `program`",
        ),
        (
            current.replacen("enabled = true", "metadata = \"undeclared\"", 1),
            "unknown field `metadata`",
        ),
        (
            current.replacen("version = 1", "version = 2", 1),
            "unsupported",
        ),
    ] {
        let message = GateManifest::parse(&candidate)
            .expect_err("candidate must refuse")
            .to_string();
        assert!(message.contains(expected), "{expected:?}: {message}");
    }

    let mut moved: Value = toml::from_str(&current).expect("manifest value");
    let gates = moved
        .get_mut("gates")
        .and_then(Value::as_array_mut)
        .expect("gate array");
    gates.swap(0, 1);
    let moved = toml::to_string_pretty(&moved).expect("moved manifest renders");
    let message = GateManifest::parse(&moved)
        .expect_err("moved order must refuse")
        .to_string();
    assert!(
        message.contains("gates[0].id must be MergeIntegrity"),
        "{message}"
    );
}

#[test]
fn packaged_fixture_migrates_every_id_in_exact_order() {
    let manifest = migrate_predecessor(inputs()).expect("packaged fixture migrates");
    assert_eq!(
        manifest
            .gates()
            .iter()
            .map(GateDeclaration::id)
            .collect::<Vec<_>>(),
        GateId::ALL
    );
    for declaration in manifest.gates() {
        assert!(declaration.enabled(), "{}", declaration.id());
        assert_eq!(declaration.id_provenance().release(), "0.31.0");
        assert_eq!(declaration.id_provenance().layer(), OriginLayer::System);
        assert_eq!(
            declaration.id_provenance().document(),
            "/installed/0.31.0/spice-rust.toml"
        );
        let mut expected = vec!["policy", "pre_commit_builtins"];
        if !absent_from_battery(declaration.id()) {
            expected.push(declaration.id().predecessor_str());
        }
        assert_eq!(
            declaration.id_provenance().field(),
            expected.as_slice(),
            "{}",
            declaration.id()
        );
    }
}

#[test]
fn an_omitted_builtin_selects_its_compiled_id_from_the_packaged_registry_root() {
    let manifest = migrate_predecessor(PredecessorInputs::new(
        "/installed/0.31.0/spice-rust.toml",
        "[policy.pre_commit_builtins]\n",
    ))
    .expect("omitted builtins select compiled defaults");

    for declaration in manifest.gates() {
        assert!(declaration.enabled(), "{}", declaration.id());
        assert_eq!(declaration.id_provenance().layer(), OriginLayer::System);
        assert_eq!(
            declaration.id_provenance().field(),
            ["policy", "pre_commit_builtins"]
        );
        assert_eq!(
            declaration.enabled_provenance(),
            declaration.id_provenance()
        );
    }
}

#[test]
fn repository_root_false_disables_only_predecessor_builtins() {
    let manifest = migrate("[policy]\npre_commit_builtins = false\n", "");

    for declaration in manifest.gates() {
        let provenance = declaration.enabled_provenance();
        if declaration.id().predecessor_declared() {
            assert!(!declaration.enabled(), "{}", declaration.id());
            assert_eq!(provenance.layer(), OriginLayer::Repository);
            assert_eq!(provenance.field(), ["policy", "pre_commit_builtins"]);
        } else {
            assert!(declaration.enabled(), "{}", declaration.id());
            assert_eq!(provenance.layer(), OriginLayer::System);
        }
    }
}

#[test]
fn noncanonical_name_normalizes_to_an_undeclared_arm_refusal_with_provenance() {
    let refusal = migrate_predecessor(inputs().repository(
        "/repo/spice-rust.toml",
        "[policy.pre_commit_builtins]\nrust_typecheck = false\n",
    ))
    .expect_err("a post-release gate name must refuse after normalization");

    assert_eq!(refusal.kind(), MigrationErrorKind::UndeclaredArm);
    assert_eq!(refusal.layer(), OriginLayer::Repository);
    assert_eq!(refusal.document(), "/repo/spice-rust.toml");
    assert_eq!(
        refusal.field(),
        ["policy", "pre_commit_builtins", "rust_typecheck"]
    );
    assert_eq!(
        refusal.recovery(),
        "remove the entry and let the compiled instrument registry declare rust-typecheck"
    );
    assert!(refusal.to_string().contains("release=0.31.0"));
}

#[test]
fn boolean_enabled_label_and_recursive_layer_resets_preserve_winning_fields() {
    let manifest = migrate(REPOSITORY, WORKTREE);

    let formatters = declaration(&manifest, GateId::Formatters);
    assert!(!formatters.enabled());
    assert_eq!(formatters.display(), Some("repository formatters"));
    assert_eq!(
        formatters
            .display_provenance()
            .expect("label source")
            .layer(),
        OriginLayer::Repository
    );
    assert_eq!(
        formatters.enabled_provenance().layer(),
        OriginLayer::Worktree
    );
    assert_eq!(
        formatters.enabled_provenance().field(),
        ["policy", "pre_commit_builtins", "formatters", "enabled"]
    );

    let magic = declaration(&manifest, GateId::MagicNumbers);
    assert!(magic.enabled());
    assert_eq!(magic.display(), Some("worktree magic"));
    assert_eq!(magic.id_provenance().layer(), OriginLayer::Worktree);
    assert_eq!(magic.enabled_provenance().layer(), OriginLayer::Worktree);

    let complexity = declaration(&manifest, GateId::Complexity);
    assert!(!complexity.enabled());
    assert_eq!(complexity.display(), None);
    assert_eq!(complexity.id_provenance().layer(), OriginLayer::Worktree);

    let taste = declaration(&manifest, GateId::Taste);
    assert!(taste.enabled());
    assert_eq!(taste.id_provenance().layer(), OriginLayer::System);
}

#[test]
fn scalar_and_table_forms_replace_one_another_at_layer_boundaries() {
    let scalar_to_table = migrate(
        "[policy.pre_commit_builtins]\ncomplexity = false\n",
        "[policy.pre_commit_builtins.complexity]\nlabel = \"restored\"\n",
    );
    let complexity = declaration(&scalar_to_table, GateId::Complexity);
    assert!(complexity.enabled());
    assert_eq!(complexity.display(), Some("restored"));
    assert_eq!(
        complexity.enabled_provenance().layer(),
        OriginLayer::Worktree
    );

    let table_to_scalar = migrate(
        "[policy.pre_commit_builtins.complexity]\nlabel = \"discarded\"\nenabled = false\n",
        "[policy.pre_commit_builtins]\ncomplexity = true\n",
    );
    let complexity = declaration(&table_to_scalar, GateId::Complexity);
    assert!(complexity.enabled());
    assert_eq!(complexity.display(), None);
    assert_eq!(
        complexity.enabled_provenance().layer(),
        OriginLayer::Worktree
    );

    let explicit_true = migrate(
        "[policy.pre_commit_builtins.complexity]\nenabled = true\n",
        "",
    );
    let complexity = declaration(&explicit_true, GateId::Complexity);
    assert!(complexity.enabled());
    assert_eq!(
        complexity.enabled_provenance().field(),
        ["policy", "pre_commit_builtins", "complexity", "enabled"]
    );
}

#[test]
fn mounted_resolution_obeys_layered_command_replacement_and_disable() {
    let executable = error(
        concat!(
            "[commands]\nstandard = [\"old\"]\n",
            "[policy.pre_commit_builtins]\nformatters = \"standard\"\n"
        ),
        "[commands]\nstandard = [\"new\", \"arm\"]\n",
    );
    assert_eq!(executable.kind(), MigrationErrorKind::ExecutableReplacement);
    assert!(executable.to_string().contains("[\"new\", \"arm\"]"));

    let unresolved = error(
        concat!(
            "[commands]\nstandard = [\"old\"]\n",
            "[policy.pre_commit_builtins]\nformatters = \"standard\"\n"
        ),
        "[commands]\nstandard = false\n",
    );
    assert_eq!(unresolved.kind(), MigrationErrorKind::UnresolvedMount);
    assert_eq!(unresolved.layer(), OriginLayer::Repository);
    assert_eq!(
        unresolved.field(),
        ["policy", "pre_commit_builtins", "formatters"]
    );
}
