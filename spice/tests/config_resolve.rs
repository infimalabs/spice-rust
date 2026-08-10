use std::path::Path;

use lane::life::Life;
use spice::config::{
    Builtin, Document, FlagError, FlagOverride, FlagValue, Inputs, IssueKind, PathSegment,
    Resolved, Root, descriptor, resolve,
};
use wire::{ConfigSource, Name};

#[cfg(not(windows))]
fn absolute(name: &str) -> String {
    format!("/opt/spice/{name}")
}

#[cfg(windows)]
fn absolute(name: &str) -> String {
    format!("C:/spice/{name}")
}

fn documents<'a>(tracked: Option<&'a str>, local: Option<&'a str>) -> Inputs<'a> {
    Inputs::new(
        tracked.map(|text| Document::new("spice-rust.toml", text)),
        local.map(|text| Document::new(".spice-rs/local.toml", text)),
        &[],
    )
}

fn path_source(resolved: &Resolved, expected: &str) -> ConfigSource {
    resolved
        .provenance()
        .iter()
        .find(|row| row.path().to_string() == expected)
        .unwrap_or_else(|| panic!("missing provenance for {expected}"))
        .source()
}

fn path_builtin(resolved: &Resolved, expected: &str) -> Option<Builtin> {
    resolved
        .provenance()
        .iter()
        .find(|row| row.path().to_string() == expected)
        .unwrap_or_else(|| panic!("missing provenance for {expected}"))
        .builtin()
}

fn flag(path: &[PathSegment<'_>], value: FlagValue) -> FlagOverride {
    let mapping = descriptor(path)
        .and_then(|entry| entry.flag())
        .expect("fixture path has a command mapping");
    FlagOverride::new(mapping, value).expect("fixture value matches its mapping")
}

#[test]
fn missing_and_empty_documents_are_empty_intent() {
    let missing = resolve(Inputs::empty()).expect("compiled defaults resolve");
    let empty = resolve(documents(Some(""), Some(""))).expect("empty documents resolve");

    assert_eq!(missing, empty);
    assert_eq!(missing.root(), &spice::config::Root::default());
    assert!(missing.root().proof().container().is_none());
    assert!(missing.root().proof().runner().is_none());
    assert_eq!(missing.provenance().len(), 52);
    assert!(
        missing
            .provenance()
            .iter()
            .all(|row| row.source() == ConfigSource::Builtin && row.builtin().is_none())
    );
    assert_eq!(
        missing
            .provenance()
            .iter()
            .rev()
            .take(3)
            .map(|row| row.path().to_string())
            .collect::<Vec<_>>(),
        ["proof.runner", "proof.container", "proof.image"]
    );
    let default_toml = toml::to_string(missing.root()).expect("default TOML renders");
    assert!(!default_toml.contains("container"));
    assert!(!default_toml.contains("runner"));
    let default_json = serde_json::to_value(missing.root()).expect("default JSON renders");
    assert!(default_json["proof"].get("container").is_none());
    assert!(default_json["proof"].get("runner").is_none());
}

#[test]
fn speech_backends_resolve_with_local_only_external_argv() {
    let disabled = resolve(documents(Some("[say]\nbackend = 'disabled'\n"), None))
        .expect("tracked disabled speech resolves");
    assert_eq!(
        disabled.root().say().backend(),
        canvas::SpeechBackend::Disabled
    );
    assert!(disabled.root().say().command().is_none());
    assert_eq!(path_source(&disabled, "say.backend"), ConfigSource::Tracked);

    let system = resolve(documents(
        Some("[say]\nvoice = 'Samantha'\nwords_per_minute = 205\n"),
        None,
    ))
    .expect("tracked system speech resolves");
    assert_eq!(system.root().say().voice(), "Samantha");
    assert_eq!(system.root().say().words_per_minute(), 205);

    let external = resolve(documents(
        Some("[say]\nbackend = 'external'\n"),
        Some(concat!(
            "[say]\n",
            "command = ['espeak-ng', '--stdout', '-s', '{words_per_minute}']\n",
            "content_type = 'audio/wav'\n",
        )),
    ))
    .expect("tracked selection and local command resolve together");
    assert_eq!(
        external
            .root()
            .say()
            .resolved_command()
            .expect("external argv"),
        ["espeak-ng", "--stdout", "-s", "175"]
    );
    assert_eq!(path_source(&external, "say.command"), ConfigSource::Local);
}

#[test]
fn speech_configuration_refuses_untrusted_or_malformed_execution() {
    let tracked_command = resolve(documents(
        Some("[say]\nbackend = 'external'\ncommand = ['espeak-ng']\n"),
        None,
    ))
    .expect_err("tracked documents cannot install executable argv");
    assert!(tracked_command.issues().iter().any(|issue| {
        issue
            .path()
            .is_some_and(|path| path.to_string() == "say.command")
            && issue.source() == ConfigSource::Tracked
            && issue.kind() == IssueKind::Policy
    }));

    for (document, path) in [
        ("[say]\nbackend = 'mystery'\n", "say.backend"),
        ("[say]\nvoice = ''\n", "say.voice"),
        (
            "[say]\ncontent_type = 'audio/wav; charset=utf-8'\n",
            "say.content_type",
        ),
        ("[say]\nwords_per_minute = 0\n", "say.words_per_minute"),
    ] {
        let error = resolve(documents(Some(document), None))
            .expect_err("malformed speech configuration must be refused");
        assert!(
            error.issues().iter().any(|issue| {
                issue
                    .path()
                    .is_some_and(|candidate| candidate.to_string() == path)
                    && issue.kind() == IssueKind::Malformed
            }),
            "missing malformed issue for {path}: {:?}",
            error.issues()
        );
    }
}

#[test]
fn current_metadata_resolves_and_removed_spellings_refuse() {
    let tracked = concat!(
        "[tasks]\nstems = ['rim']\nhidden_stems = ['sandbox']\n",
        "[policy]\nenv_names = ['FIXTURE_INBOX']\n",
        "[taste.polling]\nwords = ['poll*']\nmessage = 'use events'\n",
    );
    let local = "[lock.worker]\ndirectory = 'locks/worker'\nshards = 2\n";

    let current = resolve(documents(Some(tracked), Some(local)))
        .expect("current resolution admits typed shared metadata");
    assert_eq!(current.root().tasks().stems(), ["rim"]);
    assert_eq!(
        current.root().tasks().hidden_stems(),
        ["oops", "maxim_proposal", "sandbox"]
    );
    assert_eq!(
        current.root().tasks().project_min_depth(),
        dialect::PROJECT_MIN_DEPTH as u64
    );
    assert_eq!(
        current.root().tasks().project_max_depth(),
        dialect::PROJECT_MAX_DEPTH as u64
    );
    assert_eq!(current.root().policy().env_names(), ["FIXTURE_INBOX"]);
    assert_eq!(current.root().taste().bag().tastes().len(), 1);
    assert_eq!(current.root().lock().named().len(), 1);
    assert_eq!(
        path_source(&current, "taste.polling.words"),
        ConfigSource::Tracked
    );
    assert_eq!(
        path_source(&current, "lock.worker.shards"),
        ConfigSource::Local
    );

    for (document, path) in [
        ("[tasks]\nbase_stems = ['task']\n", "tasks.base_stems"),
        (
            "[policy]\npredecessor_only = ['removed']\n",
            "policy.predecessor_only",
        ),
        ("[unknown]\nvalue = 1\n", "unknown"),
    ] {
        let error = resolve(documents(Some(document), None))
            .expect_err("removed or unknown tracked intent must refuse");
        assert!(error.issues().iter().any(|issue| {
            issue.kind() == IssueKind::Unknown
                && issue.path().is_some_and(|found| found.to_string() == path)
        }));
    }
}

#[test]
fn every_two_layer_precedence_pair_is_deterministic() {
    let tracked = resolve(documents(Some("[lane]\nvendor = \"codex\"\n"), None))
        .expect("tracked shadows builtin");
    assert_eq!(tracked.root().lane().vendor().as_str(), "codex");
    assert_eq!(path_source(&tracked, "lane.vendor"), ConfigSource::Tracked);
    assert_eq!(
        path_builtin(&tracked, "lane.vendor"),
        Some(Builtin::String("claude"))
    );

    let local = resolve(documents(None, Some("[listen]\nwindow = \"2m\"\n")))
        .expect("local shadows builtin");
    assert_eq!(local.root().listen().window_word().to_string(), "2m");
    assert_eq!(path_source(&local, "listen.window"), ConfigSource::Local);

    let vendor_flag = flag(
        &[PathSegment::Key("lane"), PathSegment::Key("vendor")],
        FlagValue::Name("gemini".parse::<Name>().expect("valid vendor")),
    );
    let only_flag = resolve(Inputs::new(None, None, std::slice::from_ref(&vendor_flag)))
        .expect("flag shadows builtin");
    assert_eq!(only_flag.root().lane().vendor().as_str(), "gemini");
    assert_eq!(path_source(&only_flag, "lane.vendor"), ConfigSource::Flag);
    assert_eq!(
        path_builtin(&only_flag, "lane.vendor"),
        Some(Builtin::String("claude"))
    );

    let tracked_local = resolve(documents(
        Some("[listen]\nwindow = \"2m\"\n"),
        Some("[listen]\nwindow = \"3m\"\n"),
    ))
    .expect("local shadows tracked");
    assert_eq!(
        tracked_local.root().listen().window_word().to_string(),
        "3m"
    );
    assert_eq!(
        path_source(&tracked_local, "listen.window"),
        ConfigSource::Local
    );

    let tracked_flag = resolve(Inputs::new(
        Some(Document::new(
            "spice-rust.toml",
            "[lane]\nvendor = \"codex\"\n",
        )),
        None,
        std::slice::from_ref(&vendor_flag),
    ))
    .expect("flag shadows tracked");
    assert_eq!(tracked_flag.root().lane().vendor().as_str(), "gemini");
    assert_eq!(
        path_source(&tracked_flag, "lane.vendor"),
        ConfigSource::Flag
    );

    let local_and_flag = resolve(Inputs::new(
        None,
        Some(Document::new(
            ".spice-rs/local.toml",
            "[listen]\nwindow = \"4m\"\n",
        )),
        &[vendor_flag],
    ))
    .expect("local and flag own disjoint admitted leaves");
    assert_eq!(
        path_source(&local_and_flag, "listen.window"),
        ConfigSource::Local
    );
    assert_eq!(
        path_source(&local_and_flag, "lane.vendor"),
        ConfigSource::Flag
    );
}

#[test]
fn shared_metadata_is_tracked_only_and_enforces_names_and_depths() {
    let resolved = resolve(documents(
        Some(
            r#"
[tasks]
stems = ["app", "board"]
hidden_stems = ["sandbox", "oops", "sandbox"]
project_min_depth = 2
project_max_depth = 3

[policy]
env_names = ["FIXTURE_INBOX", "FIXTURE_PROOF"]
"#,
        ),
        None,
    ))
    .expect("shared tracked metadata resolves");

    assert_eq!(resolved.root().tasks().stems(), ["app", "board"]);
    assert_eq!(
        resolved.root().tasks().hidden_stems(),
        ["oops", "maxim_proposal", "sandbox"]
    );
    assert_eq!(resolved.root().tasks().project_min_depth(), 2);
    assert_eq!(resolved.root().tasks().project_max_depth(), 3);
    assert_eq!(
        resolved.root().policy().env_names(),
        ["FIXTURE_INBOX", "FIXTURE_PROOF"]
    );
    for path in [
        "tasks.stems",
        "tasks.hidden_stems",
        "tasks.project_min_depth",
        "tasks.project_max_depth",
        "policy.env_names",
    ] {
        assert_eq!(path_source(&resolved, path), ConfigSource::Tracked);
    }
    assert_eq!(
        path_builtin(&resolved, "tasks.hidden_stems"),
        Some(Builtin::Strings(&["oops", "maxim_proposal"]))
    );

    let error = resolve(documents(
        Some(
            r#"
[tasks]
stems = ["", "app", "app"]
project_min_depth = 4
project_max_depth = 3

[policy]
env_names = ["FIXTURE_INBOX", "FIXTURE_INBOX"]
"#,
        ),
        Some(
            r#"
[tasks]
stems = ["local"]
project_min_depth = 1
project_max_depth = 2

[policy]
env_names = ["LOCAL_NAME"]
"#,
        ),
    ))
    .expect_err("invalid and local shared metadata refuses");

    for (source, kind, path) in [
        (ConfigSource::Tracked, IssueKind::Malformed, "tasks.stems"),
        (
            ConfigSource::Tracked,
            IssueKind::Malformed,
            "tasks.project_min_depth",
        ),
        (
            ConfigSource::Tracked,
            IssueKind::Malformed,
            "policy.env_names",
        ),
        (ConfigSource::Local, IssueKind::Policy, "tasks.stems"),
        (
            ConfigSource::Local,
            IssueKind::Policy,
            "tasks.project_min_depth",
        ),
        (
            ConfigSource::Local,
            IssueKind::Policy,
            "tasks.project_max_depth",
        ),
        (ConfigSource::Local, IssueKind::Policy, "policy.env_names"),
    ] {
        assert!(
            error.issues().iter().any(|issue| {
                issue.source() == source
                    && issue.kind() == kind
                    && issue
                        .path()
                        .is_some_and(|actual| actual.to_string() == path)
            }),
            "missing {source:?} {kind:?} at {path}: {:#?}",
            error.issues()
        );
    }
}

#[test]
fn hidden_stems_refuse_each_malformed_or_public_colliding_value_at_its_index() {
    for (document, index, expected) in [
        ("[tasks]\nhidden_stems = ['']\n", 0, "must match"),
        ("[tasks]\nhidden_stems = ['.oops']\n", 0, "must match"),
        ("[tasks]\nhidden_stems = ['bad-stem']\n", 0, "must match"),
        ("[tasks]\nhidden_stems = ['Bad']\n", 0, "must match"),
        ("[tasks]\nhidden_stems = ['two.parts']\n", 0, "must match"),
        (
            "[tasks]\nhidden_stems = ['sandbox', 'bad-stem']\n",
            1,
            "must match",
        ),
        (
            "[tasks]\nstems = ['rim']\nhidden_stems = ['rim']\n",
            0,
            "conflicts with public stem",
        ),
        (
            "[tasks]\nhidden_stems = ['task']\n",
            0,
            "conflicts with public stem",
        ),
    ] {
        let error = resolve(documents(Some(document), None))
            .expect_err("malformed hidden metadata must refuse");
        let issue = error
            .issues()
            .iter()
            .find(|issue| {
                issue.kind() == IssueKind::Malformed
                    && issue.path().is_some_and(|path| {
                        path.to_string() == format!("tasks.hidden_stems[{index}]")
                    })
            })
            .unwrap_or_else(|| panic!("missing indexed issue for {document:?}: {error}"));
        assert!(
            issue.message().contains(expected),
            "{document:?}: {issue:?}"
        );
    }
}

#[test]
fn proof_executable_paths_are_optional_local_values_with_exact_rendering() {
    let container = absolute("bin/../container");
    let runner = absolute("runner");

    for (include_container, include_runner) in [(true, false), (false, true), (true, true)] {
        let mut local = String::from("[proof]\n");
        if include_container {
            local.push_str(&format!("container = \"{container}\"\n"));
        }
        if include_runner {
            local.push_str(&format!("runner = \"{runner}\"\n"));
        }
        let resolved =
            resolve(documents(None, Some(&local))).expect("absolute local paths resolve");

        assert_eq!(
            resolved.root().proof().container().map(AsRef::as_ref),
            include_container.then_some(Path::new(&container))
        );
        assert_eq!(
            resolved.root().proof().runner().map(AsRef::as_ref),
            include_runner.then_some(Path::new(&runner))
        );
        for (path, present) in [
            ("proof.container", include_container),
            ("proof.runner", include_runner),
        ] {
            assert_eq!(
                path_source(&resolved, path),
                if present {
                    ConfigSource::Local
                } else {
                    ConfigSource::Builtin
                }
            );
            assert_eq!(
                path_builtin(&resolved, path),
                present.then_some(Builtin::Absent)
            );
        }

        let rendered = toml::to_string_pretty(resolved.root()).expect("effective TOML renders");
        let toml_round_trip: Root = toml::from_str(&rendered).expect("effective TOML decodes");
        assert_eq!(&toml_round_trip, resolved.root());
        let json = serde_json::to_value(resolved.root()).expect("effective JSON renders");
        assert_eq!(
            json["proof"]
                .get("container")
                .and_then(|value| value.as_str()),
            include_container.then_some(container.as_str())
        );
        assert_eq!(
            json["proof"].get("runner").and_then(|value| value.as_str()),
            include_runner.then_some(runner.as_str())
        );
        let json_round_trip: Root =
            serde_json::from_value(json).expect("effective JSON decodes without null paths");
        assert_eq!(&json_round_trip, resolved.root());
    }

    let source = include_str!("../../proof/src/config.rs");
    for forbidden in [
        "std::fs",
        "std::env",
        "std::process",
        "canonicalize",
        "metadata(",
        "Command::",
    ] {
        assert!(
            !source.contains(forbidden),
            "proof configuration unexpectedly references {forbidden}"
        );
    }
}

#[test]
fn proof_executable_paths_refuse_every_non_local_or_non_absolute_route() {
    let container = absolute("container");
    let runner = absolute("runner");
    let tracked = format!("[proof]\ncontainer = \"{container}\"\nrunner = \"{runner}\"\n");
    let local = "[proof]\ncontainer = \"container\"\nrunner = \"\"\nextra = \"ignored\"\n";
    let error = resolve(documents(Some(&tracked), Some(local)))
        .expect_err("tracked, relative, empty, and unknown proof paths refuse together");

    assert_eq!(
        error
            .issues()
            .iter()
            .map(|issue| (
                issue.path().map(ToString::to_string),
                issue.source(),
                issue.kind()
            ))
            .collect::<Vec<_>>(),
        [
            (
                Some("proof.container".to_owned()),
                ConfigSource::Tracked,
                IssueKind::Policy,
            ),
            (
                Some("proof.container".to_owned()),
                ConfigSource::Local,
                IssueKind::Malformed,
            ),
            (
                Some("proof.extra".to_owned()),
                ConfigSource::Local,
                IssueKind::Unknown,
            ),
            (
                Some("proof.runner".to_owned()),
                ConfigSource::Tracked,
                IssueKind::Policy,
            ),
            (
                Some("proof.runner".to_owned()),
                ConfigSource::Local,
                IssueKind::Malformed,
            ),
        ]
    );
    assert!(
        error
            .issues()
            .iter()
            .filter(|issue| issue.kind() == IssueKind::Policy)
            .all(|issue| issue.message().contains("Git-common .spice-rs/config.toml"))
    );
    for leaf in ["container", "runner"] {
        let entry = descriptor(&[PathSegment::Key("proof"), PathSegment::Key(leaf)])
            .expect("proof executable descriptor");
        assert_eq!(entry.layers(), [ConfigSource::Local]);
        assert!(entry.flag().is_none());
    }
}

const TRACKED_ALL_DESCRIPTOR_DOCUMENT: &str = r#"
[taste.alpha]
words = ["done*"]
message = "Done."

[tasks]
stems = ["app", "board"]
hidden_stems = ["sandbox"]
project_min_depth = 2
project_max_depth = 3

[judge]
enabled = true

[policy]
env_names = ["FIXTURE_INBOX", "FIXTURE_PROOF"]
pre_commit_builtins = true
pre_commit = [
    { label = "Rust gate", run = ["gate/pre-commit"], scopes = { paths = ["src/**"] } },
]
package_roots = ["src/fixture"]
generated_paths = ["build/fixture"]
name_cluster_threshold = 4
test_paths = ["tests/fixture"]
admission = ["fixture-load", "--offline"]
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
allowed_trailers = ["Spice-Origin"]
blocked_trailers = ["Co-Authored-By"]

[consent]
posture = "deny"

[[consent.rule]]
name = "reads"
tool = "read*"
answer = "allow"
budget = 2

[gate.walk]
skip = ["target/**"]

[board.next]
band = 3

[lane]
life = "Steer"
lease = "20m"
vendor = "codex"
deaths = 4
young = "40s"
hold = "40m"

[steer]
expiry = "12h"
resend = ["2m", "4m", "6m"]

[vendor.claude]
binary = "/opt/claude"
model = "opus"
effort = "xhigh"
auto_compact_window_tokens = 123456

[vendor.codex]
binary = "/opt/codex"
model = "gpt-current"
effort = "max"
personality = "pragmatic"
fast_mode = true

[listen]
window = "2m"

[lock]
busy = 70
taken = 71
full = 72

[lock.alpha]
directory = "locks/alpha"
shards = 2

[lock.single]
path = "locks/single.lock"

[canvas.pack]
rows = 50

[say]
backend = "external"
voice = "Samantha"
content_type = "audio/wav"
words_per_minute = 190

[proof]
image = "sha256:4444444444444444444444444444444444444444444444444444444444444444"
"#;

#[test]
fn tracked_document_can_address_every_registry_descriptor() {
    let local = format!(
        concat!(
            "[say]\ncommand = ['espeak-ng', '--stdout']\n\n",
            "[proof]\ncontainer = \"{}\"\nrunner = \"{}\"\n",
        ),
        absolute("container"),
        absolute("runner")
    );
    let resolved = resolve(documents(
        Some(TRACKED_ALL_DESCRIPTOR_DOCUMENT),
        Some(&local),
    ))
    .expect("tracked and local descriptors resolve together");

    for row in resolved.provenance() {
        let path = row.path().to_string();
        let expected = if matches!(
            path.as_str(),
            "say.command" | "proof.container" | "proof.runner"
        ) {
            ConfigSource::Local
        } else {
            ConfigSource::Tracked
        };
        assert_eq!(row.source(), expected, "{path}");
    }
    for template in spice::config::descriptors() {
        assert!(
            resolved.provenance().iter().any(|row| {
                let segments = row.path().segments().collect::<Vec<_>>();
                template.template().matches(&segments)
            }),
            "missing concrete provenance for {}",
            template.template().as_str()
        );
    }
}

#[test]
fn all_four_layers_use_one_leaf_order() {
    let life = flag(
        &[PathSegment::Key("lane"), PathSegment::Key("life")],
        FlagValue::Life(Life::Drain),
    );
    let vendor = flag(
        &[PathSegment::Key("lane"), PathSegment::Key("vendor")],
        FlagValue::Name("gemini".parse().expect("valid vendor")),
    );
    let flags = [life, vendor];
    let resolved = resolve(Inputs::new(
        Some(Document::new(
            "spice-rust.toml",
            concat!(
                "[lane]\nlife = \"Steer\"\nvendor = \"codex\"\n",
                "[listen]\nwindow = \"2m\"\n[lock]\nbusy = 70\n",
            ),
        )),
        Some(Document::new(
            ".spice-rs/local.toml",
            "[listen]\nwindow = \"3m\"\n[lock]\nbusy = 71\n",
        )),
        &flags,
    ))
    .expect("all four layers resolve");

    assert_eq!(resolved.root().lane().life(), Life::Drain);
    assert_eq!(resolved.root().lane().vendor().as_str(), "gemini");
    assert_eq!(resolved.root().listen().window_word().to_string(), "3m");
    assert_eq!(resolved.root().lock().exits().busy().get(), 71);
    assert_eq!(path_source(&resolved, "lane.life"), ConfigSource::Flag);
    assert_eq!(path_source(&resolved, "lane.vendor"), ConfigSource::Flag);
    assert_eq!(path_source(&resolved, "listen.window"), ConfigSource::Local);
    assert_eq!(path_source(&resolved, "lock.busy"), ConfigSource::Local);
    assert_eq!(path_source(&resolved, "proof.image"), ConfigSource::Builtin);
}

#[test]
fn tables_merge_recursively_and_dynamic_lock_rows_keep_typed_paths() {
    let resolved = resolve(documents(
        Some(concat!(
            "[lock]\nbusy = 70\ntaken = 71\nfull = 72\n",
            "[lock.alpha]\ndirectory = 'locks/alpha'\nshards = 2\n",
            "[lock.\"a.b\"]\ndirectory = 'locks/a.b'\nshards = 3\n",
        )),
        Some("[lock.alpha]\nshards = 4\n[lock.zeta]\ndirectory = 'locks/zeta'\nshards = 5\n"),
    ))
    .expect("dynamic lock tables merge");

    assert_eq!(resolved.root().lock().exits().busy().get(), 70);
    assert_eq!(
        resolved
            .root()
            .lock()
            .named()
            .map(|(name, target)| (name.as_str(), target.shards().get()))
            .collect::<Vec<_>>(),
        [("a.b", 3), ("alpha", 4), ("zeta", 5)]
    );
    assert_eq!(
        path_source(&resolved, "lock.\"a.b\".shards"),
        ConfigSource::Tracked
    );
    assert_eq!(
        path_source(&resolved, "lock.alpha.shards"),
        ConfigSource::Local
    );
    assert_eq!(
        path_source(&resolved, "lock.zeta.shards"),
        ConfigSource::Local
    );
    assert_eq!(path_builtin(&resolved, "lock.alpha.shards"), None);
}

#[test]
fn one_layer_can_atomically_swap_cross_validated_lock_values() {
    let resolved = resolve(documents(
        Some("[lock]\nbusy = 70\ntaken = 71\nfull = 72\n"),
        Some("[lock]\nbusy = 71\ntaken = 70\nfull = 73\n"),
    ))
    .expect("the complete local lock subtree validates as one layer");

    let exits = resolved.root().lock().exits();
    assert_eq!(
        (exits.busy().get(), exits.taken().get(), exits.full().get()),
        (71, 70, 73)
    );
    for path in ["lock.busy", "lock.taken", "lock.full"] {
        assert_eq!(path_source(&resolved, path), ConfigSource::Local);
    }
}

#[test]
fn lists_replace_as_units_and_taste_rows_retain_declared_order() {
    let resolved = resolve(documents(
        Some(
            r#"
[steer]
resend = ["2m", "4m", "6m"]

[gate.walk]
skip = ["target/**"]

[taste.zeta]
words = ["wait*"]
message = "Wait."

[taste.alpha]
words = ["done*"]
message = "Done."
"#,
        ),
        None,
    ))
    .expect("ordered lists and tastes resolve");

    assert_eq!(
        resolved
            .root()
            .steer()
            .resend()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["2m", "4m", "6m"]
    );
    assert_eq!(
        resolved
            .root()
            .gate()
            .walk()
            .skip()
            .rules()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["target/**"]
    );
    assert_eq!(
        resolved
            .root()
            .taste()
            .bag()
            .tastes()
            .iter()
            .map(|taste| taste.name().as_str())
            .collect::<Vec<_>>(),
        ["zeta", "alpha"]
    );
    assert_eq!(
        path_source(&resolved, "steer.resend"),
        ConfigSource::Tracked
    );
}

#[test]
fn diagnostics_accumulate_unknown_policy_and_malformed_values() {
    let error = resolve(documents(
        Some(
            r#"
[tasks]
owner = "bootstrap"

[policy]
mode = "bootstrap"

[lane]
lease = "soon"
deaths = 0
mystery = 1

[board.next]
band = -1

[proof]
image = "latest"

[canvas.pack]
rows = "many"
"#,
        ),
        Some(
            r#"
[lane]
life = "Drain"

[listen]
window = "never"

[lock.alpha]
directory = "locks/alpha"
shards = 0

[lock.beta]
directory = "locks/beta"
shards = 2
mystery = 3

[canvas.pack]
rows = 0

[bogus]
value = 1
"#,
        ),
    ))
    .expect_err("every independent issue refuses the result");

    let rows = error
        .issues()
        .iter()
        .map(|issue| {
            (
                issue.path().map(ToString::to_string),
                issue.source(),
                issue.kind(),
                issue.message(),
            )
        })
        .collect::<Vec<_>>();
    for (path, source, kind) in [
        (
            "board.next.band",
            ConfigSource::Tracked,
            IssueKind::Malformed,
        ),
        ("bogus", ConfigSource::Local, IssueKind::Unknown),
        (
            "canvas.pack.rows",
            ConfigSource::Tracked,
            IssueKind::Malformed,
        ),
        (
            "canvas.pack.rows",
            ConfigSource::Local,
            IssueKind::Malformed,
        ),
        ("lane.deaths", ConfigSource::Tracked, IssueKind::Malformed),
        ("lane.lease", ConfigSource::Tracked, IssueKind::Malformed),
        ("lane.life", ConfigSource::Local, IssueKind::Policy),
        ("lane.mystery", ConfigSource::Tracked, IssueKind::Unknown),
        ("listen.window", ConfigSource::Local, IssueKind::Malformed),
        ("lock.alpha", ConfigSource::Local, IssueKind::Malformed),
        ("lock.beta.mystery", ConfigSource::Local, IssueKind::Unknown),
        ("policy.mode", ConfigSource::Tracked, IssueKind::Unknown),
        ("proof.image", ConfigSource::Tracked, IssueKind::Malformed),
        ("tasks.owner", ConfigSource::Tracked, IssueKind::Unknown),
    ] {
        assert!(
            rows.iter()
                .any(|row| { row.0.as_deref() == Some(path) && row.1 == source && row.2 == kind }),
            "missing {source:?} {kind:?} issue at {path}: {rows:#?}"
        );
    }
    let local_policy = error
        .issues()
        .iter()
        .find(|issue| {
            issue
                .path()
                .is_some_and(|path| path.to_string() == "lane.life")
        })
        .expect("local policy issue");
    assert!(local_policy.message().contains("tracked spice-rust.toml"));
    assert!(error.issues().windows(2).all(|pair| {
        pair[0].path().map(ToString::to_string) <= pair[1].path().map(ToString::to_string)
    }));
}

#[test]
fn syntax_positions_and_other_layer_diagnostics_are_both_returned() {
    let error = resolve(Inputs::new(
        Some(Document::new(
            "config/spice-rust.toml",
            "# caf\u{e9}\n[lane]\nlease = [\n",
        )),
        Some(Document::new(
            ".spice-rs/local.toml",
            "[unknown]\nvalue = 1\n",
        )),
        &[],
    ))
    .expect_err("syntax and unknown table refuse");

    let syntax = error
        .issues()
        .iter()
        .find(|issue| issue.kind() == IssueKind::Syntax)
        .expect("syntax issue");
    assert_eq!(syntax.document(), "config/spice-rust.toml");
    assert_eq!(syntax.source(), ConfigSource::Tracked);
    assert!(syntax.line().is_some_and(|line| line >= 3));
    assert!(syntax.column().is_some());
    assert!(error.issues().iter().any(|issue| {
        issue.kind() == IssueKind::Unknown
            && issue.source() == ConfigSource::Local
            && issue
                .path()
                .is_some_and(|path| path.to_string() == "unknown")
    }));
}

#[test]
fn flag_values_cannot_cross_descriptor_types_or_duplicate_mappings() {
    let life_mapping = descriptor(&[PathSegment::Key("lane"), PathSegment::Key("life")])
        .and_then(|entry| entry.flag())
        .expect("life flag mapping");
    assert_eq!(
        FlagOverride::new(
            life_mapping,
            FlagValue::Name("codex".parse().expect("valid name"))
        ),
        Err(FlagError::WrongValue {
            expected: spice::config::ValueKind::Life,
            actual: spice::config::ValueKind::String,
        })
    );

    let life =
        FlagOverride::new(life_mapping, FlagValue::Life(Life::Steer)).expect("typed life flag");
    let flags = [life.clone(), life];
    let error = resolve(Inputs::new(None, None, &flags)).expect_err("duplicate flag refuses");
    assert!(error.issues().iter().any(|issue| {
        issue.kind() == IssueKind::DuplicateFlag
            && issue
                .path()
                .is_some_and(|path| path.to_string() == "lane.life")
    }));
}

#[test]
fn equivalent_repeated_resolutions_are_identical_and_the_resolver_has_no_io_surface() {
    let tracked =
        "[listen]\nwindow = \"2m\"\n[lock.alpha]\ndirectory = 'locks/alpha'\nshards = 2\n";
    let local = "[listen]\nwindow = \"3m\"\n";
    let first = resolve(documents(Some(tracked), Some(local))).expect("first resolution");
    let second = resolve(documents(Some(tracked), Some(local))).expect("second resolution");
    assert_eq!(first, second);

    let source = include_str!("../src/config/resolve.rs");
    for forbidden in [
        "std::fs",
        "std::env",
        "std::process",
        "std::net",
        "journal::",
        "SystemTime",
        "Instant::now",
        "tokio::",
        "reqwest::",
    ] {
        assert!(
            !source.contains(forbidden),
            "pure resolver unexpectedly references {forbidden}"
        );
    }
}
