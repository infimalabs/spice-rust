use std::path::{Path, PathBuf};

use gate::{
    BASELINE_PATH, BoundName, Contract, ContractError, POLICY_PATH, PolicyError, RuleSettings,
};
use serde::Deserialize;

mod support;

const TRACKED_POLICY: &str = ::std::include_str!("../../spice-rust.toml");
const TRACKED_BASELINE: &str = ::std::include_str!("../../baseline.toml");

#[test]
fn identical_tracked_bytes_yield_identical_execution_identities() {
    let first = fixture_contract(TRACKED_POLICY, TRACKED_BASELINE).expect("first load");
    let second = fixture_contract(TRACKED_POLICY, TRACKED_BASELINE).expect("second load");

    assert_eq!(first.policy().render(), second.policy().render());
    assert_eq!(first.policy().digest(), second.policy().digest());
    assert_eq!(first.gate_set().render(), second.gate_set().render());
    assert_eq!(first.gate_set().digest(), second.gate_set().digest());
    assert_eq!(first.bounds().render(), second.bounds().render());
    assert_eq!(first.bounds().digest(), second.bounds().digest());
    assert_eq!(first.baseline().render(), second.baseline().render());
    assert_eq!(first.baseline().digest(), second.baseline().digest());
    assert_eq!(first.tools(), second.tools());
}

#[test]
fn admitted_walk_intent_moves_policy_without_selecting_an_executable() {
    let compiled = fixture_contract(TRACKED_POLICY, TRACKED_BASELINE).expect("compiled defaults");
    let narrowed = fixture_contract(
        &loadable("[gate.walk]\nskip = [\"fixtures/\"]\n"),
        TRACKED_BASELINE,
    )
    .expect("tracked skip is admitted");

    assert_ne!(compiled.policy().digest(), narrowed.policy().digest());
    assert_eq!(compiled.gate_set().digest(), narrowed.gate_set().digest());
    assert_eq!(compiled.bounds().digest(), narrowed.bounds().digest());
    assert_eq!(compiled.baseline().digest(), narrowed.baseline().digest());
    assert_eq!(compiled.tools(), narrowed.tools());
    assert_eq!(
        narrowed.policy().ignore().render(),
        ".git/\ntarget/\nfixtures/\n"
    );

    let refused = fixture_contract("[gate]\nprogram = \"/bin/true\"\n", TRACKED_BASELINE)
        .expect_err("tracked bytes cannot select an executable");
    let ContractError::Policy(PolicyError::Toml { path, source }) = refused else {
        panic!("unknown executable key lost its typed config refusal: {refused:?}");
    };
    assert_eq!(path, policy_path());
    assert!(source.to_string().contains("program"));
}

#[test]
fn malformed_policy_and_unknown_gate_fields_refuse_before_a_contract_exists() {
    let malformed = fixture_contract("gate = [\n", TRACKED_BASELINE)
        .expect_err("malformed shared policy is refused");
    assert!(matches!(
        malformed,
        ContractError::Policy(PolicyError::Toml { ref path, .. }) if path == &policy_path()
    ));

    let unknown = fixture_contract("[gate.walk]\nskip = []\ntimeout = 1\n", TRACKED_BASELINE)
        .expect_err("unknown gate field is refused");
    let ContractError::Policy(PolicyError::Toml { path, source }) = unknown else {
        panic!("unknown gate key lost its typed config refusal: {unknown:?}");
    };
    assert_eq!(path, policy_path());
    assert!(source.to_string().contains("timeout"));

    let source = "[gate.walk]\nskip = [\"fixtures/**\"]\n";
    let selector = fixture_contract(&loadable(source), TRACKED_BASELINE)
        .expect("documented crossing-star skip is admitted");
    let configured = toml::from_str::<GateDocument>(source)
        .expect("the same tracked bytes decode through the configuration tree");
    assert_eq!(
        selector.policy().ignore().render(),
        ".git/\ntarget/\nfixtures/**\n"
    );
    assert_eq!(
        &selector.policy().ignore().rules()[2..],
        configured.gate.walk().skip().rules(),
        "input policy must retain the domain configuration's exact admitted rules"
    );
}

/// The tracked document inventory is the one the contract compiles.
///
/// `AGENTS.md` is packaged, so a contract that never reads the declaration keeps
/// naming it and can produce none of the three other inventories below. The
/// declared documents deliberately carry no markdown suffix, because the
/// packaged markdown-depth rules select every `.md` path on their own and would
/// hide whether the declaration was read at all.
#[test]
fn the_tracked_repository_document_inventory_reaches_the_compiled_rule_policy() {
    let packaged = fixture_contract(TRACKED_POLICY, TRACKED_BASELINE)
        .expect("the tracked workspace declares no repository documents");
    assert_eq!(
        packaged.rule_policy().document().repo_truth_docs(),
        ["AGENTS.md"],
        "a repository that declares nothing measures the packaged inventory"
    );

    let declared = fixture_contract(
        &declaring("docs = [\"./NOTICE.txt\", \"charter/CHARTER.txt\"]"),
        TRACKED_BASELINE,
    )
    .expect("a declared repository-document inventory is admitted");
    assert_eq!(
        declared.rule_policy().document().repo_truth_docs(),
        ["NOTICE.txt", "charter/CHARTER.txt"],
        "the declared inventory is the compiled one, normalized and ordered"
    );
    assert!(
        declared
            .rule_policy()
            .selects_repository_document("NOTICE.txt")
            .expect("a declared document resolves"),
        "a declared document no packaged rule can reach was left unselected"
    );

    let none = fixture_contract(&declaring("docs = []"), TRACKED_BASELINE)
        .expect("declaring no repository documents is admitted");
    assert!(
        none.rule_policy().document().repo_truth_docs().is_empty(),
        "declaring an empty inventory is not the same as declaring nothing"
    );

    let refused = fixture_contract(&declaring("docs = [\"../escape.txt\"]"), TRACKED_BASELINE)
        .expect_err("a document outside the repository is refused");
    let ContractError::Policy(PolicyError::Rules {
        path,
        source: refusal,
    }) = refused
    else {
        panic!("an escaping document lost its typed tracked refusal: {refused:?}");
    };
    assert_eq!(path, policy_path());
    assert_eq!(refusal.source(), POLICY_PATH);
    assert_eq!(refusal.field(), "policy.repo_truth.docs");
}

/// The tracked workspace policy plus one `[policy.repo_truth]` declaration.
fn declaring(docs: &str) -> String {
    format!("{TRACKED_POLICY}\n[policy.repo_truth]\n{docs}\n")
}

/// Every `[policy]` subtable the rule-policy migration reads is tracked.
///
/// The contract compiles its rule policy from the tracked document alone, so
/// each of these subtables decides part of it. Each carries a value distinct
/// from the packaged default *and* from every other value here, because six
/// surfaces that all arrive as the same number cannot show that six of them
/// arrived.
#[test]
fn every_migrated_policy_subtable_reaches_the_compiled_rule_policy() {
    let declared = fixture_contract(
        &format!(
            "{TRACKED_POLICY}
[policy.limits]
file_loc = 1234
[policy.flex]
ratio = 2.25
jitter_percent = 7
[policy.magic]
examine_threshold = 33
[policy.markdown_depth]
base_chars = 100
max_bounded_chars = 300
[policy.markdown_depth_budget]
extensions = [\".rst\"]
[[policy.rules]]
routine_length = {{ max = 55 }}
"
        ),
        TRACKED_BASELINE,
    )
    .expect("every migrated policy subtable is admitted from the tracked document");
    let document = declared.rule_policy().document();

    assert_eq!(document.limits().get(BoundName::FileLoc), 1234);
    assert!((document.ratchet().ratio() - 2.25).abs() < f64::EPSILON);
    assert_eq!(document.ratchet().variation_percent(), 7);
    assert_eq!(document.magic_examine_threshold(), 33);

    let generated: Vec<&str> = document
        .rules()
        .iter()
        .map(|rule| rule.id().as_str())
        .collect();
    assert!(
        generated.iter().any(|id| id.contains("rst")),
        "the declared suffix selects the generated depth rules: {generated:?}"
    );
    assert!(
        !generated.iter().any(|id| id.contains("md")),
        "declaring a suffix names the whole set, so .md is no longer generated: {generated:?}"
    );
    assert_eq!(
        document
            .rules()
            .iter()
            .filter_map(|rule| rule.bounds().get(&BoundName::RoutineLength))
            .filter_map(RuleSettings::maximum)
            .collect::<Vec<_>>(),
        [55],
        "the one declared row is compiled beside the generated ones: {generated:?}"
    );
    assert_eq!(
        generated.len(),
        5,
        "300 bounded over a 100 base is three bounded depths plus the unbounded \
         rule, plus the one declared row: {generated:?}"
    );

    let refused = fixture_contract(
        &format!("{TRACKED_POLICY}\n[policy.magic]\nexamine_threshhold = 33\n"),
        TRACKED_BASELINE,
    )
    .expect_err("a misspelled key inside a migrated subtable is refused, not ignored");
    let ContractError::Policy(PolicyError::Rules { source, .. }) = refused else {
        panic!("a misspelled migrated key lost its typed tracked refusal: {refused:?}");
    };
    assert_eq!(source.field(), "policy.magic.examine_threshhold");
}

#[derive(Deserialize)]
struct GateDocument {
    gate: gate::Config,
}

/// One tracked policy document that can load: a fixture body carrying the
/// file-class selections migration requires of every document.
fn loadable(body: &str) -> String {
    format!("{}{body}", gate::NARROWEST_FILE_CLASSES)
}

fn fixture_contract(policy: &str, baseline: &str) -> Result<Contract, ContractError> {
    Contract::from_tracked(
        &workspace_root(),
        &policy_path(),
        policy,
        &baseline_path(),
        baseline,
    )
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("gate is a direct workspace member")
        .to_path_buf()
}

fn policy_path() -> PathBuf {
    PathBuf::from("fixture/spice-rust.toml")
}

fn baseline_path() -> PathBuf {
    PathBuf::from("fixture").join(BASELINE_PATH)
}
