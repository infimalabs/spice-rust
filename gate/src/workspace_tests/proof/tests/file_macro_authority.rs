use super::*;
use crate::{
    ActionKey, ActionKeyAuthority, ActionReuseClass, ActionSpec, ActionSpecFields, Selector,
    compile_explicit_action_input, derive_authority_bound_action_key, test_repository,
};
use wire::{Digest, Domain};

#[test]
fn opaque_expansion_file_macro_authority_refuses_before_a_shard_can_be_planned() {
    let fixture = Fixture::new("opaque-expansion-file-macro");
    fs::write(
        fixture.root.join("fixture/src/lib.rs"),
        concat!(
            "macro_rules! outer {\n",
            "    ($macro_name:ident) => {\n",
            "        pub const VALUE: &str = $macro_name!(\"../../file-macro-authority.txt\");\n",
            "    };\n",
            "}\n",
            "outer!(include_str);\n",
        ),
    )
    .expect("write opaque expansion file-macro source");

    let error = fixture
        .try_plan(true)
        .expect_err("opaque file-macro expansion authority cannot produce Cargo shards");
    assert!(
        error.to_string().contains(
            "cannot prove expansion-site file-macro namespace for macro path containing $macro_name"
        ),
        "unexpected refusal: {error}",
    );
}

#[test]
fn transitive_compiler_namespace_input_reaches_the_cargo_shard() {
    let fixture = Fixture::new("transitive-file-macro-authority");
    fs::write(
        fixture.root.join("fixture/src/lib.rs"),
        concat!(
            "#![doc = core::include_str!(\"../../file-macro-authority.txt\")]\n",
            "#![cfg_attr(any(), doc = core::include_bytes!(\"../../file-macro-authority.txt\"))]\n",
            "use ::std as standard;\n",
            "use standard as core;\n",
            "pub const AUTHORITY: u8 = 1;\n",
        ),
    )
    .expect("write transitive file-macro source");
    test_repository::initialize(&fixture.root);
    test_repository::stage_all(&fixture.root);
    test_repository::commit(&fixture.root, "seed inner attribute authority");
    let original = fixture.plan(true);
    assert!(
        original
            .typechecks()
            .iter()
            .find(|shard| shard.package() == "fixture")
            .expect("fixture typecheck shard is planned")
            .input_paths()
            .contains(&"file-macro-authority.txt".to_owned())
    );
    let authority = fixture.root.join("file-macro-authority.txt");
    fs::remove_file(&authority).expect("remove file-macro input");
    let error = fixture
        .try_plan(true)
        .expect_err("a removed compiler input must refuse Cargo planning");
    assert!(
        error
            .to_string()
            .contains("is not readable selected-tree authority"),
        "unexpected refusal: {error}",
    );
}

#[test]
fn inner_attribute_input_moves_the_manifest_and_action_identity() {
    let fixture = Fixture::new("inner-attribute-manifest");
    fs::write(
        fixture.root.join("fixture/src/lib.rs"),
        "#![doc = include_str!(\"../../file-macro-authority.txt\")]\npub const VALUE: u8 = 1;\n",
    )
    .expect("write inner-attribute source");
    test_repository::initialize(&fixture.root);
    test_repository::stage_all(&fixture.root);
    test_repository::commit(&fixture.root, "seed inner attribute manifest");

    let original = closure_identity(&fixture, Selector::revision("HEAD"));
    assert_eq!(
        closure_identity(&fixture, Selector::revision("HEAD")),
        original,
        "an unchanged selected closure must retain its manifest and action identity",
    );

    fs::write(
        fixture.root.join("file-macro-authority.txt"),
        "changed file authority\n",
    )
    .expect("change inner-attribute input");
    test_repository::stage(&fixture.root, &["file-macro-authority.txt"]);
    let changed = closure_identity(&fixture, Selector::staged());
    assert_ne!(
        changed.0, original.0,
        "input content must move the manifest"
    );
    assert_ne!(
        changed.1, original.1,
        "input content must move the action key"
    );
}

fn closure_identity(fixture: &Fixture, selector: Selector) -> (Digest, ActionKey) {
    let plan = fixture.plan(true);
    let shard = plan
        .typechecks()
        .iter()
        .find(|shard| shard.package() == "fixture")
        .expect("fixture typecheck shard is planned");
    let tree = selector
        .materialize(&fixture.root)
        .expect("materialize selected Cargo shard tree");
    let manifest = compile_explicit_action_input(&tree, shard.input_paths().to_vec(), Vec::new())
        .expect("compile Cargo shard input manifest");
    let manifest = manifest.digest().expect("digest Cargo shard manifest");
    let spec = ActionSpec::new(ActionSpecFields {
        action_id: shard.action_id().to_owned(),
        runner: "cargo-gate-shard".to_owned(),
        runner_version: 1,
        declaration: "cargo-proof-v1".to_owned(),
        argv: shard.argv().to_vec(),
        cwd: ".".to_owned(),
        environment_names: Vec::new(),
        tools: Vec::new(),
        input_strategy: "explicit-selected-tree-v1".to_owned(),
        reuse: ActionReuseClass::Cacheable,
    })
    .expect("construct Cargo shard action specification");
    let authority = ActionKeyAuthority {
        input_manifest: manifest.clone(),
        policy: authority_digest(b"policy"),
        gate_set: authority_digest(b"gate-set"),
        tools: authority_digest(b"tools"),
        environment: authority_digest(b"environment"),
        baseline: None,
    };
    let action = derive_authority_bound_action_key(&spec, &authority)
        .expect("derive authority-bound Cargo action key");
    (manifest, action)
}

fn authority_digest(bytes: &[u8]) -> Digest {
    Digest::compute(Domain::GateActionKey, bytes)
}
