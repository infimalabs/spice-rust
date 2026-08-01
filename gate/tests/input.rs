use std::path::{Path, PathBuf};

use gate::{
    BASELINE_PATH, BaselineError, Contract, ContractError, LONG_LINE_LIMIT, PolicyError, Standing,
};
use serde::Deserialize;

const TRACKED_POLICY: &str = include_str!("../../spice.toml");
const TRACKED_BASELINE: &str = include_str!("../../baseline.toml");
const TRACKED_LONG_LINE_FLOOR: u64 = 70;

#[test]
fn tracked_inputs_load_one_operational_current_contract() {
    let root = workspace_root();
    let contract = Contract::load(&root).expect("tracked current gate inputs load");

    assert_eq!(contract.policy().ignore().render(), ".git/\ntarget/\n");
    assert_eq!(contract.policy().metrics().len(), 1);
    let metric = &contract.policy().metrics()[0];
    assert_eq!(metric.name(), "rust.long-lines");
    assert_eq!(metric.parser().program(), "/usr/bin/awk");
    assert_eq!(
        metric.parser().arguments(),
        [format!(
            "length($0) > {LONG_LINE_LIMIT} {{ debt += 1 }} END {{ print debt + 0 }}"
        )]
    );
    assert_eq!(
        contract
            .baseline()
            .floors()
            .iter()
            .map(|floor| (floor.name(), floor.value()))
            .collect::<Vec<_>>(),
        [
            ("rust.long-lines", TRACKED_LONG_LINE_FLOOR),
            ("rust.mutation-survivors", 0),
        ]
    );
    assert_eq!(
        contract.ratchet().baselines().collect::<Vec<_>>(),
        [("rust.long-lines", TRACKED_LONG_LINE_FLOOR)],
        "mutation debt is current baseline state but not a retired parser metric"
    );
    assert_eq!(contract.tools().len(), 1);
    assert_eq!(contract.tools()[0].name(), "awk");
    assert_eq!(contract.tools()[0].program(), Path::new("/usr/bin/awk"));
    assert_eq!(
        contract.bounds().render(),
        "bounds 1\nowner gate current tools\nruntime-ms 30000\noutput-drain-ms 1000\n\
         reap-ms 1000\ncancellation never\nrestart never\ngrace-ms 250\ncapture 65536\n"
    );

    let run = contract
        .adjudicate(&root)
        .expect("an operational contract reaches a complete verdict");
    assert_eq!(run.receipt().standing(), Standing::Clean);
    assert_eq!(run.measurements().len(), 1);
    assert!(
        run.measurements()[0].value()
            <= contract
                .baseline()
                .floor("rust.long-lines")
                .expect("long-line floor is loaded")
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
    let narrowed = fixture_contract("[gate.walk]\nskip = [\"fixtures/\"]\n", TRACKED_BASELINE)
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
        "version = 1\n\n[[floor]]\nname = \"rust.long-lines\"\nvalue = 1\n\n\
         [[floor]]\nname = \"rust.long-lines\"\nvalue = 2\n\n[[floor]]\n\
         name = \"rust.mutation-survivors\"\nvalue = 0\n",
    );
    assert!(matches!(
        duplicate,
        BaselineError::Duplicate { ref path, ref field, ref name }
            if path == &baseline_path() && field == "floor[1].name" && name == "rust.long-lines"
    ));

    let unsorted = baseline_error(
        "version = 1\n\n[[floor]]\nname = \"rust.mutation-survivors\"\nvalue = 0\n\n\
         [[floor]]\nname = \"rust.long-lines\"\nvalue = 1\n",
    );
    assert!(matches!(
        unsorted,
        BaselineError::Unsorted { ref path, ref field, ref previous, ref name }
            if path == &baseline_path() && field == "floor[1].name"
                && previous == "rust.mutation-survivors" && name == "rust.long-lines"
    ));

    let malformed =
        baseline_error("version = 1\n\n[[floor]]\nname = \"rust.long-lines\"\nvalue = \"many\"\n");
    assert!(matches!(
        malformed,
        BaselineError::Field(ref field)
            if field.path() == baseline_path() && field.field() == "floor[0].value"
    ));

    let missing =
        baseline_error("version = 1\n\n[[floor]]\nname = \"rust.long-lines\"\nvalue = 1\n");
    assert!(matches!(
        missing,
        BaselineError::Missing { ref path, ref field, ref name }
            if path == &baseline_path() && field == "floor"
                && name == "rust.mutation-survivors"
    ));

    let retired = baseline_error(
        "version = 1\n\n[[floor]]\nname = \"rust.kill-rate\"\nvalue = 1\n\n\
         [[floor]]\nname = \"rust.long-lines\"\nvalue = 1\n\n[[floor]]\n\
         name = \"rust.mutation-survivors\"\nvalue = 0\n",
    );
    assert!(matches!(
        retired,
        BaselineError::Retired { ref path, ref field, ref name }
            if path == &baseline_path() && field == "floor[0].name" && name == "rust.kill-rate"
    ));

    let unknown = baseline_error(
        "version = 1\n\n[[floor]]\nname = \"rust.long-lines\"\nvalue = 1\n\n\
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
    let selector = fixture_contract(source, TRACKED_BASELINE)
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

#[derive(Deserialize)]
struct GateDocument {
    gate: gate::Config,
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
    PathBuf::from("fixture/spice.toml")
}

fn baseline_path() -> PathBuf {
    PathBuf::from("fixture").join(BASELINE_PATH)
}
