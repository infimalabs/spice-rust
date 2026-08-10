const TRACKED_ALL_DESCRIPTOR_DOCUMENT: &str = r#"
[commands]
release = ["tool", "release"]

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
taste_mining_threshold = 3
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

    let source = include_str!("../../src/config/resolve.rs");
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
