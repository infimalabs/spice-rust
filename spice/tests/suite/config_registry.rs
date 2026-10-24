use std::collections::{BTreeMap, BTreeSet};

use gate::CONFIG_KEY_DECLARATIONS;
use spice::config::{Builtin, PathSegment, PolicyClass, Root, ValueKind, descriptor, descriptors};
use toml::Value;
use wire::ConfigSource;

const COMPLETE: &str = r#"
[commands]
"report.inspect" = ["tool", "report", "inspect"]
release = ["tool", "release"]

[wrappers]
spice = "appliance"

[tasks]
hidden_stems = ["oops", "sandbox", "sandbox"]
project_min_depth = 2
project_max_depth = 3

[judge]
enabled = true

[policy]
env_names = ["FIXTURE_INBOX", "FIXTURE_PROOF"]
pre_commit_builtins = false
pre_commit = [
    { label = "Rust gate", run = ["gate/pre-commit", "--fixture"], scopes = { paths = ["src/**", "tests/**"] } },
]
test_engines = ["cargo-workspace"]
package_roots = ["src/fixture"]
generated_paths = ["build/fixture"]
name_cluster_threshold = 4
taste_mining_threshold = 3
test_paths = ["tests/fixture"]

[policy.repo_truth]
docs = ["DOCTRINE.md"]

[policy.file_shape]
source_suffixes = [".rs", ".py"]

[policy.languages]
complexity = [".rs", ".py"]
magic = [".rs"]

[policy.package]
boundary_underscore_pattern = "^[a-z][a-z0-9_]*$"

[policy.commit_message]
allowed_trailers = ["Spice-Origin", "Reviewed-By"]
blocked_trailers = ["Co-Authored-By"]

[taste.zeta]
words = ["delay", "slept"]
message = "Wait on the real event."
enabled = false
scopes = { paths = ["src/**"], extensions = [".rs"], vendors = ["omp"], models = ["gpt-current"], phases = ["pre-commit-success"] }

[taste.alpha]
words = ["finish"]
message = "Observe completion."

[consent]
posture = "queue"

[[consent.rule]]
name = "reads"
tool = "read*"
answer = "allow"
budget = 25

[[consent.rule]]
name = "fallback"
tool = "*"
answer = "deny"

[gate.walk]
skip = ["src/**", "fixtures/"]

[board.next]
horizon = "3h"

[lane]
life = "Steer"
lease = "90s"
vendor = "omp"
deaths = 4
young = "45s"
hold = "1h"
narration = 30

[steer]
expiry = "48h"
resend = ["10m", "30m", "2h"]

[vendor.omp]
binary = "/opt/omp"
model = "gpt-5.6-sol"
effort = "max"
fast_mode = true
auto_compact_window_tokens = 250000

[listen]
window = "10m"

[lock]
busy = 70
taken = 71
full = 72

[lock.alpha]
path = "locks/alpha.lock"

[lock.zeta]
directory = "locks/zeta"
shards = 4

[lock."a.b"]
directory = "locks/a.b"
shards = 5

[lock."a[0]"]
directory = "locks/a[0]"
shards = 6

[serve.pack]
rows = 80

[proof]
image = "registry.example/proof@sha256:4444444444444444444444444444444444444444444444444444444444444444"
"#;

#[cfg(not(windows))]
fn absolute(name: &str) -> String {
    format!("/opt/spice/{name}")
}

#[cfg(windows)]
fn absolute(name: &str) -> String {
    format!("C:/spice/{name}")
}

fn complete() -> String {
    format!(
        "{COMPLETE}container = \"{}\"\nrunner = \"{}\"\n",
        absolute("container"),
        absolute("runner")
    )
}

fn decode(document: &str) -> Result<Root, String> {
    let deserializer = toml::Deserializer::parse(document).map_err(|error| error.to_string())?;
    serde_path_to_error::deserialize(deserializer).map_err(|error| error.to_string())
}

fn encoded(root: &Root) -> Value {
    let document = toml::to_string(root).expect("the typed root encodes");
    toml::from_str(&document).expect("encoded root is TOML")
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum OwnedSegment {
    Key(String),
    Index(usize),
}

fn borrowed(path: &[OwnedSegment]) -> Vec<PathSegment<'_>> {
    path.iter()
        .map(|segment| match segment {
            OwnedSegment::Key(key) => PathSegment::Key(key),
            OwnedSegment::Index(index) => PathSegment::Index(*index),
        })
        .collect()
}

fn leaves(value: &Value) -> Vec<(Vec<OwnedSegment>, Value)> {
    fn walk(
        path: &mut Vec<OwnedSegment>,
        value: &Value,
        output: &mut Vec<(Vec<OwnedSegment>, Value)>,
    ) {
        match value {
            Value::Table(table) => {
                for (field, child) in table {
                    path.push(OwnedSegment::Key(field.clone()));
                    walk(path, child, output);
                    path.pop();
                }
            }
            Value::Array(values) if values.iter().all(Value::is_table) && !values.is_empty() => {
                for (index, child) in values.iter().enumerate() {
                    path.push(OwnedSegment::Index(index));
                    walk(path, child, output);
                    path.pop();
                }
            }
            Value::Array(values) if values.is_empty() && descriptor(&borrowed(path)).is_none() => {}
            _ => output.push((path.clone(), value.clone())),
        }
    }

    let mut output = Vec::new();
    walk(&mut Vec::new(), value, &mut output);
    output
}

fn builtin_matches(builtin: Builtin, value: &Value) -> bool {
    match (builtin, value) {
        (Builtin::String(expected), Value::String(actual)) => actual == expected,
        (Builtin::Strings(expected), Value::Array(actual)) => {
            actual.len() == expected.len()
                && actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| actual.as_str() == Some(expected))
        }
        (Builtin::UnsignedInteger(expected), Value::Integer(actual)) => {
            u64::try_from(*actual) == Ok(expected)
        }
        (Builtin::Float(expected), Value::Float(actual)) => actual == &expected,
        (Builtin::Boolean(expected), Value::Boolean(actual)) => actual == &expected,
        _ => false,
    }
}

fn sample_path(template: &str) -> Vec<OwnedSegment> {
    let mut path = Vec::new();
    for segment in template.split('.') {
        if segment == "<name>" {
            path.push(OwnedSegment::Key("alpha".to_owned()));
        } else if let Some(key) = segment.strip_suffix("[]") {
            path.push(OwnedSegment::Key(key.to_owned()));
            path.push(OwnedSegment::Index(0));
        } else {
            path.push(OwnedSegment::Key(segment.to_owned()));
        }
    }
    path
}

#[test]
fn shared_metadata_directly_refuses_invalid_names_depths_and_children() {
    for (document, expected) in [
        ("[policy]\nenv_names = [\"\"]\n", "nonempty"),
        (
            "[policy]\nenv_names = [\"FIXTURE_PROOF\", \"FIXTURE_PROOF\"]\n",
            "unique",
        ),
        ("[tasks]\nproject_min_depth = 0\n", "positive"),
        ("[tasks]\nproject_max_depth = 0\n", "positive"),
        (
            "[tasks]\nproject_min_depth = 4\nproject_max_depth = 3\n",
            "less than or equal",
        ),
        ("[tasks]\nunknown = true\n", "unknown"),
        ("[wrappers]\nunknown = \"appliance\"\n", "unknown"),
        ("[wrappers]\nspice = \"ambient\"\n", "unknown variant"),
        ("[rtk]\nexecutable = \"\"\n", "non-empty executable"),
        ("[rtk]\nexecutable = \"path/rtk\"\n", "basename or absolute"),
        ("[rtk]\nunknown = \"rtk\"\n", "unknown"),
        ("[policy]\nunknown = true\n", "unknown"),
    ] {
        let error = decode(document).expect_err("invalid shared metadata must refuse");
        assert!(error.contains(expected), "{document:?}: {error}");
    }
}

#[test]
fn pre_commit_commands_admit_only_the_current_nonempty_shape() {
    for document in [
        "[policy]\npre_commit = [{ label = \"\", run = [\"gate/pre-commit\"] }]\n",
        "[policy]\npre_commit = [{ label = \"  \", run = [\"gate/pre-commit\"] }]\n",
        "[policy]\npre_commit = [{ label = \"Rust gate\", run = [] }]\n",
        "[policy]\npre_commit = [{ label = \"Rust gate\", run = [\"\"] }]\n",
        "[policy]\npre_commit = [{ label = \"Rust gate\", run = [\"gate/pre-commit\"], scopes = { paths = [\"\"] } }]\n",
        "[policy]\npre_commit = [{ label = \"Rust gate\", run = [\"gate/pre-commit\"], scopes = { paths = [\"src/**\", \"src/**\"] } }]\n",
        "[policy]\npre_commit = [{ label = \"Rust gate\", run = [\"gate/pre-commit\"], scopes = { unknown = [\"src/**\"] } }]\n",
        "[policy]\npre_commit = [{ label = \"Rust gate\", run = [\"gate/pre-commit\"], scopes = { paths = [\"../src/**\"] } }]\n",
    ] {
        assert!(decode(document).is_err(), "{document:?} must refuse");
    }

    assert!(decode("[policy]\npre_commit_builtins = false\n").is_err());
}

#[test]
fn arbitrary_pre_commit_commands_round_trip_in_authored_order() {
    let document = r#"
[policy]
pre_commit_builtins = true
pre_commit = [
    { label = "first", run = ["tool", "one"] },
    { label = "second", run = ["tool", "two"], scopes = { paths = ["src/**"] } },
    { label = "third", run = ["tool", "three", "--exact"] },
]
"#;
    let root = decode(document).expect("an arbitrary ordered list decodes");
    let round_trip = decode(&toml::to_string(&root).expect("ordered policy serializes"))
        .expect("serialized ordered policy decodes");
    assert_eq!(round_trip, root);
    assert_eq!(
        root.policy()
            .pre_commit()
            .iter()
            .map(|step| {
                (
                    step.label(),
                    step.run().join("\0"),
                    step.scopes().paths().join("\0"),
                )
            })
            .collect::<Vec<_>>(),
        [
            ("first", "tool\0one".to_owned(), String::new()),
            ("second", "tool\0two".to_owned(), "src/**".to_owned(),),
            ("third", "tool\0three\0--exact".to_owned(), String::new()),
        ]
    );
}

#[test]
fn one_registry_row_covers_every_populated_serde_leaf() {
    let root = decode(&complete()).expect("the complete root decodes");
    let mut coverage = BTreeMap::<&str, usize>::new();

    for (path, _) in leaves(&encoded(&root)) {
        let path_segments = borrowed(&path);
        let matches = descriptors()
            .iter()
            .filter(|entry| entry.template().matches(&path_segments))
            .collect::<Vec<_>>();
        assert_eq!(matches.len(), 1, "{path:?} matched {matches:?}");
        *coverage.entry(matches[0].template().as_str()).or_default() += 1;
    }

    assert_eq!(coverage.len(), descriptors().len());
    for entry in descriptors() {
        assert!(
            coverage.contains_key(entry.template().as_str()),
            "fixture omitted {}",
            entry.template().as_str()
        );
    }
}

#[test]
fn registry_builtins_are_the_typed_root_defaults() {
    let mut seen = BTreeSet::new();
    for (path, value) in leaves(&encoded(&Root::default())) {
        let entry =
            descriptor(&borrowed(&path)).unwrap_or_else(|| panic!("unregistered default {path:?}"));
        let builtin = entry
            .builtin()
            .unwrap_or_else(|| panic!("default {path:?} has no builtin metadata"));
        assert!(
            builtin_matches(builtin, &value),
            "{path:?}: {builtin:?} != {value:?}"
        );
        assert!(seen.insert(entry.template().as_str()));
    }

    assert_eq!(
        seen,
        descriptors()
            .iter()
            .filter(|entry| {
                entry
                    .builtin()
                    .is_some_and(|builtin| builtin != Builtin::Absent)
            })
            .map(|entry| entry.template().as_str())
            .collect()
    );
}

/// Every leaf the registry publishes, in the order a reader meets it.
///
/// Order is part of the contract: help output and provenance reports walk the
/// registry as declared, so a row that moves is a visible change even when the
/// set is unchanged.
const EXPECTED_TEMPLATES: &[&str] = &[
    "commands.<name>",
    "wrappers.spice",
    "rtk.executable",
    "tasks.hidden_stems",
    "tasks.project_min_depth",
    "tasks.project_max_depth",
    "judge.enabled",
    "policy.env_names",
    "policy.pre_commit_builtins",
    "policy.pre_commit[].label",
    "policy.pre_commit[].run",
    "policy.pre_commit[].scopes.paths",
    "policy.test_engines",
    "policy.commit_message.allowed_trailers",
    "policy.commit_message.blocked_trailers",
    "policy.repo_truth.docs",
    "policy.file_shape.source_suffixes",
    "policy.languages.complexity",
    "policy.languages.magic",
    "policy.package_roots",
    "policy.generated_paths",
    "policy.name_cluster_threshold",
    "policy.taste_mining_threshold",
    "policy.test_paths",
    "policy.package.boundary_underscore_pattern",
    "policy.admission",
    "taste.<name>.words",
    "taste.<name>.message",
    "taste.<name>.enabled",
    "taste.<name>.scopes.paths",
    "taste.<name>.scopes.extensions",
    "taste.<name>.scopes.vendors",
    "taste.<name>.scopes.models",
    "taste.<name>.scopes.phases",
    "consent.posture",
    "consent.rule[].name",
    "consent.rule[].tool",
    "consent.rule[].answer",
    "consent.rule[].budget",
    "gate.walk.skip",
    "board.next.horizon",
    "lane.life",
    "lane.lease",
    "lane.runtime",
    "lane.vendor",
    "lane.deaths",
    "lane.young",
    "lane.hold",
    "lane.narration",
    "lane.wake_order",
    "steer.expiry",
    "steer.resend",
    "vendor.omp.binary",
    "vendor.omp.model",
    "vendor.omp.effort",
    "vendor.omp.fast_mode",
    "vendor.omp.lsp",
    "vendor.omp.auto_compact_window_tokens",
    "listen.window",
    "lock.busy",
    "lock.taken",
    "lock.full",
    "lock.<name>.path",
    "lock.<name>.directory",
    "lock.<name>.shards",
    "serve.pack.rows",
    "say.backend",
    "say.voices",
    "say.words_per_minute",
    "proof.image",
    "proof.container",
    "proof.runner",
];

#[test]
fn templates_are_unique_canonical_non_overlapping_and_stably_ordered() {
    assert_eq!(
        descriptors()
            .iter()
            .map(|entry| entry.template().as_str())
            .collect::<Vec<_>>(),
        EXPECTED_TEMPLATES
    );

    let mut unique = BTreeSet::new();
    for entry in descriptors() {
        let template = entry.template().as_str();
        assert!(unique.insert(template), "duplicate template {template}");
        assert_eq!(
            entry.template().is_dynamic(),
            template.contains("<name>") || template.contains("[]"),
            "template shape drifted for {template}"
        );
        for segment in template.split('.') {
            let word = segment
                .strip_suffix("[]")
                .filter(|word| !word.is_empty())
                .unwrap_or(segment);
            if word == "<name>" {
                continue;
            }
            assert!(
                word.split('_').all(|part| {
                    !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_lowercase())
                }),
                "non-canonical owned key {word:?} in {template}"
            );
        }

        let concrete = sample_path(template);
        let concrete_segments = borrowed(&concrete);
        assert_eq!(
            descriptors()
                .iter()
                .filter(|candidate| candidate.template().matches(&concrete_segments))
                .count(),
            1,
            "overlap at {concrete:?}"
        );
    }

    for name in ["a.b", "a[0]", ""] {
        for field in ["path", "directory", "shards"] {
            let path = [
                PathSegment::Key("lock"),
                PathSegment::Key(name),
                PathSegment::Key(field),
            ];
            assert_eq!(
                descriptor(&path).map(|entry| entry.template().as_str()),
                Some(match field {
                    "path" => "lock.<name>.path",
                    "directory" => "lock.<name>.directory",
                    "shards" => "lock.<name>.shards",
                    _ => unreachable!(),
                }),
                "the registry must leave {name:?} to the lock name authority"
            );
        }
    }
}

/// Gate admits exactly the keys the resolver can carry.
///
/// A key Gate admits but the resolver cannot place makes the whole document
/// refuse to load, and a resolver row Gate does not admit is a setting Gate
/// would report as unknown.
#[test]
fn gate_key_admission_uses_the_resolvers_exact_packaged_registry() {
    let resolver = descriptors()
        .iter()
        .map(|entry| entry.template().as_str())
        .collect::<BTreeSet<_>>();
    let admitted = CONFIG_KEY_DECLARATIONS
        .iter()
        .map(|entry| entry.template())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        resolver, admitted,
        "gate admission and the resolver disagree"
    );

    for gate in CONFIG_KEY_DECLARATIONS {
        let entry = descriptors()
            .iter()
            .find(|entry| entry.template().as_str() == gate.template())
            .unwrap_or_else(|| panic!("resolver row for {}", gate.template()));
        assert_eq!(gate.layers(), entry.layers(), "{}", gate.template());
    }
}

#[test]
fn layer_policy_kind_and_flag_metadata_are_complete() {
    let worktree = descriptors()
        .iter()
        .filter(|entry| entry.layers().contains(&ConfigSource::Worktree))
        .map(|entry| entry.template().as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        worktree,
        [
            "vendor.omp.model",
            "vendor.omp.effort",
            "say.voices",
            "proof.runner",
        ]
    );

    let local = descriptors()
        .iter()
        .filter(|entry| entry.layers().contains(&ConfigSource::Local))
        .map(|entry| entry.template().as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        local,
        [
            "rtk.executable",
            "lane.wake_order",
            "vendor.omp.model",
            "vendor.omp.effort",
            "listen.window",
            "lock.busy",
            "lock.taken",
            "lock.full",
            "lock.<name>.path",
            "lock.<name>.directory",
            "lock.<name>.shards",
            "serve.pack.rows",
            "say.backend",
            "say.voices",
            "say.words_per_minute",
            "proof.container",
        ]
    );

    let flags = descriptors()
        .iter()
        .filter_map(|entry| entry.flag().map(|flag| (entry.template().as_str(), flag)))
        .collect::<Vec<_>>();
    assert_eq!(flags.len(), 2);
    assert_eq!(flags[0].0, "lane.life");
    assert_eq!(flags[0].1.command(), ["lane", "add"]);
    assert_eq!(flags[0].1.flag(), "--life");
    assert_eq!(flags[1].0, "lane.vendor");
    assert_eq!(flags[1].1.command(), ["lane", "add"]);
    assert_eq!(flags[1].1.flag(), "--vendor");

    for entry in descriptors() {
        let worktree_preference = entry.policy_class() == PolicyClass::WorktreeLocal;
        assert_eq!(
            entry.layers().contains(&ConfigSource::Tracked),
            entry.policy_class() == PolicyClass::Policy,
            "only tracked policy is tracked: {}",
            entry.template().as_str()
        );
        assert!(!entry.layers().contains(&ConfigSource::Builtin));
        assert_eq!(
            entry.policy_class() == PolicyClass::MachineLocal,
            entry.layers().contains(&ConfigSource::Local) && !worktree_preference,
            "{}",
            entry.template().as_str()
        );
        assert_eq!(
            entry.policy_class() == PolicyClass::WorktreeLocal,
            entry.layers().contains(&ConfigSource::Worktree),
            "{}",
            entry.template().as_str()
        );
        assert_eq!(
            entry.layers().contains(&ConfigSource::Flag),
            entry.flag().is_some(),
            "{}",
            entry.template().as_str()
        );

        let compatible = matches!(
            (entry.kind(), entry.builtin()),
            (_, None)
                | (_, Some(Builtin::Absent))
                | (ValueKind::Boolean, Some(Builtin::Boolean(_)))
                | (
                    ValueKind::String | ValueKind::Duration | ValueKind::Answer | ValueKind::Life,
                    Some(Builtin::String(_)),
                )
                | (
                    ValueKind::StringList | ValueKind::DurationList,
                    Some(Builtin::Strings(_)),
                )
                | (
                    ValueKind::UnsignedInteger,
                    Some(Builtin::UnsignedInteger(_)),
                )
                | (ValueKind::Float, Some(Builtin::Float(_)))
        );
        assert!(
            compatible,
            "incompatible builtin for {}",
            entry.template().as_str()
        );
    }
}
