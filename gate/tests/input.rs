use std::path::{Path, PathBuf};

use gate::{
    BASELINE_PATH, BaselineError, BoundName, Contract, ContractError, LONG_LINE_LIMIT,
    PINNED_CARGO_MUTANTS, POLICY_PATH, PolicyContext, PolicyError, RuleSettings, Standing,
};
use serde::Deserialize;

mod support;

use support::contract_repository;

const TRACKED_POLICY: &str = include_str!("../../spice-rust.toml");
const TRACKED_BASELINE: &str = include_str!("../../baseline.toml");
const TRACKED_FILE_LOC_LIMIT: u64 = 1_000;
const TRACKED_FILE_LOC_RATCHET: u64 = 1_500;

#[test]
fn tracked_inputs_load_one_operational_current_contract() {
    let repository = contract_repository("input-current-contract");
    let root = &repository.root;
    let contract = Contract::load(root).expect("tracked current gate inputs load");

    the_measured_policy_is_current(&contract);
    assert_eq!(contract.kill().tool().identity(), PINNED_CARGO_MUTANTS);
    assert!(contract.kill().tool().cargo_program().is_absolute());
    assert!(contract.kill().tool().mutants_program().is_absolute());
    assert_eq!(contract.kill().battery().attempts(), 3);
    assert_eq!(contract.kill().battery().seed(), 41);
    assert_eq!(contract.kill().battery().targets().len(), 4);
    assert_eq!(
        contract.kill().bounds().deadlines().runtime().as_secs(),
        300
    );
    assert_eq!(contract.kill().bounds().grace().as_secs(), 1);
    assert_eq!(contract.kill().bounds().capture(), 640 * 1024);
    assert_eq!(contract.kill().floor().numerator(), 0);
    assert_eq!(contract.kill().floor().denominator(), 1);
    the_kill_declaration_is_current(&contract.gate_set().render());
    // Written out rather than rendered from the constants, so a bound that moves
    // has to be restated here deliberately. The runtime figure in particular is
    // measured rather than chosen: see RUNTIME_DEADLINE in gate/src/input.rs for
    // the cold canvas-build samples it clears and by how much.
    assert_eq!(
        contract.bounds().render(),
        "bounds 1\nowner gate current tools\nruntime-ms 120000\noutput-drain-ms 1000\n\
         reap-ms 1000\ncancellation never\nrestart never\ngrace-ms 250\ncapture 65536\n"
    );

    // Selected test execution compiles a sealed source tree with repository
    // environment removed. The tracked inputs above remain authoritative in
    // that tree, while live-index adjudication belongs only to a real checkout.
    if !root.join(".git").exists() {
        return;
    }

    let run = contract
        .adjudicate(root)
        .expect("an operational contract reaches a complete verdict");
    // This adjudicates an owned repository against its captured HEAD, so the
    // result is independent of whether Cargo compiled the test from the live
    // worktree or from an immutable selected source without Git metadata.
    assert_eq!(
        run.receipt().standing(),
        Standing::Clean,
        "{}",
        gate::report::refusal_detail(root, &contract, &run.report())
    );
    assert!(run.measurements().is_empty());
    assert!(run.receipt().long_lines().is_empty());
    repository.remove();
}

/// Native long-line admission, durable mutation state, and the Camp profile.
fn the_measured_policy_is_current(contract: &Contract) {
    assert!(
        contract
            .policy()
            .render()
            .starts_with("jurisdiction tracked-index\n"),
        "the live Contract must render its sole tracked-index path source"
    );
    assert_eq!(contract.policy().ignore().render(), ".git/\ntarget/\n");
    assert!(contract.policy().metrics().is_empty());
    assert_eq!(
        contract
            .baseline()
            .floors()
            .iter()
            .map(|floor| (floor.name(), floor.value()))
            .collect::<Vec<_>>(),
        [("rust.mutation-survivors", 0)]
    );
    assert!(contract.ratchet().baselines().next().is_none());
    assert_eq!(contract.tools().len(), 1);
    assert_eq!(contract.tools()[0].name(), "awk");
    assert_eq!(contract.tools()[0].program(), Path::new("/usr/bin/awk"));
    assert_eq!(contract.camp().bounds().len(), 1);
    assert_eq!(contract.camp().bounds()[0].limit(), LONG_LINE_LIMIT as u64);
    assert_eq!(contract.camp().bounds()[0].loci().len(), 4);
}

/// Every environment name and target the rendered kill declaration carries.
///
/// Written out one line at a time rather than compared whole, so a name that
/// disappears is reported as the name it was rather than as a differing blob.
fn the_kill_declaration_is_current(declaration: &str) {
    assert!(declaration.contains("kill identity cargo-mutants 27.1.0\n"));
    assert!(declaration.contains("kill environment clear\n"));
    assert!(declaration.contains("kill environment AR /usr/bin/ar\n"));
    assert!(declaration.contains("kill environment CARGO "));
    assert!(declaration.contains(
        "kill environment CARGO_BUILD_BUILD_DIR selected-snapshot/target/mutation-build\n"
    ));
    assert!(declaration.contains("kill environment CARGO_INCREMENTAL 0\n"));
    assert!(
        declaration.contains(
            "kill environment CARGO_TARGET_DIR selected-snapshot/target/mutation-target\n"
        )
    );
    assert!(declaration.contains("kill environment CC /usr/bin/cc\n"));
    assert!(declaration.contains("kill environment HOME "));
    assert!(declaration.contains(":/usr/bin:/bin\nkill environment RUSTFLAGS "));
    assert!(declaration.contains("kill environment RUSTFLAGS -C linker=/usr/bin/cc\n"));
    assert!(declaration.contains("kill environment RUSTC_WORKSPACE_WRAPPER \n"));
    assert!(declaration.contains("kill environment RUSTC_WRAPPER \n"));
    assert!(declaration.contains("kill environment SPICE_MUTATION_SEED compiled-seed\n"));
    assert!(declaration.contains("kill environment SPICE_MUTATION_ATTEMPT one-based-attempt\n"));
    let target_lines = declaration
        .lines()
        .filter(|line| line.starts_with("kill target "))
        .collect::<Vec<_>>();
    assert_eq!(
        target_lines,
        [
            "kill target gate/src/camp.rs",
            "kill target gate/src/input.rs",
            "kill target gate/src/kill.rs",
            "kill target gate/src/verdict.rs",
        ]
    );
}

/// The tracked contract carries the bound the file-shape arm now measures against.
///
/// These three claims were asserted while the arm was still unavailable, because
/// the sentence it reported named them and had previously blamed the contract for
/// its own missing acquisition. The arm acquires now, so they stopped describing a
/// refusal and started describing its inputs: this scope is selected by
/// `file_shape.source_suffixes`, the tracked contract resolves a bound for a path
/// the arm really measures, and the actor seed moves that bound. Each is still
/// measured against the tracked document rather than restated, since an arm
/// reading a bound nobody declared would measure every path against a default.
/// A zero variation would leave the elastic limit no headroom to vary, which is
/// the same as having no seed at all.
#[test]
fn the_tracked_contract_resolves_the_file_shape_bound_the_arm_measures_against() {
    let contract = Contract::load(&workspace_root()).expect("tracked current gate inputs load");
    let policy = contract.rule_policy();
    assert!(
        policy
            .document()
            .file_classes()
            .file_shape()
            .iter()
            .any(|suffix| suffix == ".rs"),
        "the measured path is outside the class the arm selects its scope from"
    );

    let measured = PolicyContext::new().with_path("gate/src/metrics/acquire.rs");
    let bound = policy
        .resolve_bound(BoundName::FileLoc, &measured)
        .expect("the tracked contract resolves the file-shape bound for a measured path");
    assert_eq!(bound.limit(), TRACKED_FILE_LOC_LIMIT);
    assert_eq!(bound.ratchet_limit(), TRACKED_FILE_LOC_RATCHET);
    assert!(!bound.unlimited());

    let unpathed = policy
        .resolve_bound(BoundName::FileLoc, &PolicyContext::new())
        .expect("no declared file_loc rule leaves the global bound owed to any context");
    assert_eq!(
        unpathed, bound,
        "no declared rule narrows this path, so the arm measures it against the global bound"
    );
    assert!(
        policy.document().ratchet().variation_percent() > 0,
        "a zero variation would leave the actor seed no headroom to vary"
    );
}

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
fn every_invalid_baseline_refuses_whole_and_names_its_exact_field() {
    let duplicate = baseline_error(
        "version = 1\n\n[[floor]]\nname = \"rust.mutation-survivors\"\nvalue = 0\n\n\
         [[floor]]\nname = \"rust.mutation-survivors\"\nvalue = 0\n",
    );
    assert!(matches!(
        duplicate,
        BaselineError::Duplicate { ref path, ref field, ref name }
            if path == &baseline_path()
                && field == "floor[1].name" && name == "rust.mutation-survivors"
    ));

    let malformed = baseline_error(
        "version = 1\n\n[[floor]]\nname = \"rust.mutation-survivors\"\nvalue = \"many\"\n",
    );
    assert!(matches!(
        malformed,
        BaselineError::Field(ref field)
            if field.path() == baseline_path() && field.field() == "floor[0].value"
    ));

    let missing = baseline_error("version = 1\nfloor = []\n");
    assert!(matches!(
        missing,
        BaselineError::Missing { ref path, ref field, ref name }
            if path == &baseline_path() && field == "floor"
                && name == "rust.mutation-survivors"
    ));

    let retired = baseline_error(
        "version = 1\n\n[[floor]]\nname = \"rust.long-lines\"\nvalue = 1\n\n\
         [[floor]]\nname = \"rust.mutation-survivors\"\nvalue = 0\n",
    );
    assert!(matches!(
        retired,
        BaselineError::Retired { ref path, ref field, ref name, owner: 1 }
            if path == &baseline_path()
                && field == "floor[0].name" && name == "rust.long-lines"
    ));

    let unknown = baseline_error(
        "version = 1\n\n[[floor]]\nname = \"rust.mutation-survivors\"\nvalue = 0\n\n\
         [[floor]]\nname = \"rust.unknown\"\nvalue = 0\n",
    );
    assert!(matches!(
        unknown,
        BaselineError::Unknown { ref path, ref field, ref name }
            if path == &baseline_path() && field == "floor[1].name" && name == "rust.unknown"
    ));

    let version = baseline_error(&TRACKED_BASELINE.replacen("version = 1", "version = 2", 1));
    assert!(matches!(
        version,
        BaselineError::UnsupportedVersion { ref path, ref field, version: 2 }
            if path == &baseline_path() && field == "version"
    ));

    let alternate = baseline_error(&(TRACKED_BASELINE.to_owned() + "\n"));
    assert!(matches!(
        alternate,
        BaselineError::NonCanonical { ref path, ref field, .. }
            if path == &baseline_path() && field == "document"
    ));

    let not_toml = baseline_error("version = [\n");
    assert!(matches!(
        not_toml,
        BaselineError::Toml { ref path, .. } if path == &baseline_path()
    ));

    let fractional_without_denominator =
        baseline_error("version = 1\n\n[[floor]]\nname = \"rust.mutation-survivors\"\nvalue = 2\n");
    assert!(matches!(
        fractional_without_denominator,
        BaselineError::MutationRate { ref path, ref field, value: 2 }
            if path == &baseline_path()
                && field == "floor[rust.mutation-survivors].value"
    ));
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

fn baseline_error(text: &str) -> BaselineError {
    match fixture_contract(TRACKED_POLICY, text).expect_err("baseline must refuse") {
        ContractError::Baseline(error) => error,
        other => panic!("baseline refusal changed class: {other:?}"),
    }
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
