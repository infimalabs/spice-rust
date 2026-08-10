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
    assert_eq!(missing.provenance().len(), 53);
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

include!("config_resolve/registry.rs");
