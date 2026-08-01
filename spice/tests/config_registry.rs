use std::collections::{BTreeMap, BTreeSet};

use spice::config::{Builtin, PathSegment, PolicyClass, Root, ValueKind, descriptor, descriptors};
use toml::Value;
use wire::ConfigSource;

const COMPLETE: &str = r#"
[taste.zeta]
words = ["delay*", "slept"]
message = "Wait on the real event."

[taste.alpha]
words = ["finish*"]
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
review = -75.5
band = 3.0

[lane]
life = "Steer"
lease = "90s"
vendor = "codex"
deaths = 4
young = "45s"
hold = "1h"

[steer]
expiry = "48h"
resend = ["10m", "30m", "2h"]

[vendor.claude]
binary = "/opt/claude"

[vendor.codex]
binary = "/opt/codex"

[listen]
window = "10m"

[lock]
busy = 70
taken = 71
full = 72

[lock.alpha]
shards = 2

[lock.zeta]
shards = 4

[lock."a.b"]
shards = 5

[lock."a[0]"]
shards = 6

[canvas.pack]
rows = 80

[proof]
image = "registry.example/proof@sha256:4444444444444444444444444444444444444444444444444444444444444444"
"#;

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
fn root_is_exactly_the_eleven_typed_domains() {
    let root = decode(COMPLETE).expect("the complete root decodes");
    let round_trip = decode(&toml::to_string_pretty(&root).expect("root renders"))
        .expect("rendered root decodes");
    assert_eq!(round_trip, root);
    assert_eq!(decode("").expect("empty root"), Root::default());

    let Value::Table(table) = encoded(&Root::default()) else {
        panic!("a root must encode as a table");
    };
    assert_eq!(
        table.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "taste", "consent", "gate", "board", "lane", "steer", "vendor", "listen", "lock",
            "canvas", "proof",
        ])
    );

    for forbidden in [
        "journal", "bus", "fold", "ack", "dialect", "tape", "wire", "app", "tasks", "policy",
        "raw", "unknown",
    ] {
        let document = format!("[{forbidden}]\nvalue = 1\n");
        let error = decode(&document).expect_err("an unknown root domain must refuse");
        assert!(error.contains(forbidden), "{forbidden}: {error}");
    }
}

#[test]
fn one_registry_row_covers_every_populated_serde_leaf() {
    let root = decode(COMPLETE).expect("the complete root decodes");
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
            .filter(|entry| entry.builtin().is_some())
            .map(|entry| entry.template().as_str())
            .collect()
    );
    assert_eq!(
        descriptor(&[PathSegment::Key("proof"), PathSegment::Key("image"),])
            .and_then(|entry| entry.builtin()),
        Some(Builtin::String(proof::DEFAULT_IMAGE))
    );
}

#[test]
fn templates_are_unique_single_word_non_overlapping_and_stably_ordered() {
    let expected = [
        "taste.<name>.words",
        "taste.<name>.message",
        "consent.posture",
        "consent.rule[].name",
        "consent.rule[].tool",
        "consent.rule[].answer",
        "consent.rule[].budget",
        "gate.walk.skip",
        "board.next.review",
        "board.next.band",
        "lane.life",
        "lane.lease",
        "lane.vendor",
        "lane.deaths",
        "lane.young",
        "lane.hold",
        "steer.expiry",
        "steer.resend",
        "vendor.<name>.binary",
        "listen.window",
        "lock.busy",
        "lock.taken",
        "lock.full",
        "lock.<name>.shards",
        "canvas.pack.rows",
        "proof.image",
    ];
    assert_eq!(
        descriptors()
            .iter()
            .map(|entry| entry.template().as_str())
            .collect::<Vec<_>>(),
        expected
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
                word.bytes().all(|byte| byte.is_ascii_lowercase()),
                "non-word owned key {word:?} in {template}"
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
        let path = [
            PathSegment::Key("lock"),
            PathSegment::Key(name),
            PathSegment::Key("shards"),
        ];
        assert_eq!(
            descriptor(&path).map(|entry| entry.template().as_str()),
            Some("lock.<name>.shards"),
            "the registry must leave {name:?} to the lock name authority"
        );
    }
}

#[test]
fn layer_policy_kind_and_flag_metadata_are_complete() {
    let local = descriptors()
        .iter()
        .filter(|entry| entry.layers().contains(&ConfigSource::Local))
        .map(|entry| entry.template().as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        local,
        [
            "listen.window",
            "lock.busy",
            "lock.taken",
            "lock.full",
            "lock.<name>.shards",
            "canvas.pack.rows",
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
        assert!(entry.layers().contains(&ConfigSource::Tracked));
        assert!(!entry.layers().contains(&ConfigSource::Builtin));
        assert_eq!(
            entry.policy_class() == PolicyClass::MachineLocal,
            entry.layers().contains(&ConfigSource::Local),
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
