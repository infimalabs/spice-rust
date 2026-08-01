use lane::life::Life;
use spice::config::{
    Builtin, Document, FlagError, FlagOverride, FlagValue, Inputs, IssueKind, PathSegment,
    Resolved, descriptor, resolve,
};
use wire::{ConfigSource, Name};

fn documents<'a>(tracked: Option<&'a str>, local: Option<&'a str>) -> Inputs<'a> {
    Inputs::new(
        tracked.map(|text| Document::new("spice.toml", text)),
        local.map(|text| Document::new(".spice/local.toml", text)),
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
    assert_eq!(missing.provenance().len(), 18);
    assert!(
        missing
            .provenance()
            .iter()
            .all(|row| row.source() == ConfigSource::Builtin && row.builtin().is_none())
    );
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
        Some(Document::new("spice.toml", "[lane]\nvendor = \"codex\"\n")),
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
            ".spice/local.toml",
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
fn tracked_document_can_address_every_registry_descriptor() {
    let resolved = resolve(documents(
        Some(
            r#"
[taste.alpha]
words = ["done*"]
message = "Done."

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
review = -80.0
band = 3.0

[lane]
life = "Steer"
lease = "20m"
vendor = "codex"
deaths = 4
young = "40s"
hold = "40m"

[steer]
expiry = "12h"
resend = ["2m"]

[vendor.codex]
binary = "/opt/codex"

[listen]
window = "2m"

[lock]
busy = 70
taken = 71
full = 72

[lock.alpha]
shards = 2

[canvas.pack]
rows = 50

[proof]
image = "sha256:4444444444444444444444444444444444444444444444444444444444444444"
"#,
        ),
        None,
    ))
    .expect("all tracked descriptors resolve together");

    assert!(
        resolved
            .provenance()
            .iter()
            .all(|row| row.source() == ConfigSource::Tracked)
    );
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
            "spice.toml",
            concat!(
                "[lane]\nlife = \"Steer\"\nvendor = \"codex\"\n",
                "[listen]\nwindow = \"2m\"\n[lock]\nbusy = 70\n",
            ),
        )),
        Some(Document::new(
            ".spice/local.toml",
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
            "[lock.alpha]\nshards = 2\n[lock.\"a.b\"]\nshards = 3\n",
        )),
        Some("[lock.alpha]\nshards = 4\n[lock.zeta]\nshards = 5\n"),
    ))
    .expect("dynamic lock tables merge");

    assert_eq!(resolved.root().lock().exits().busy().get(), 70);
    assert_eq!(
        resolved
            .root()
            .lock()
            .named()
            .map(|(name, shards)| (name.as_str(), shards.get()))
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
resend = ["2m"]

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
        ["2m"]
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
band = -1.0

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
shards = 0

[lock.beta]
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
        ("policy", ConfigSource::Tracked, IssueKind::Unknown),
        ("proof.image", ConfigSource::Tracked, IssueKind::Malformed),
        ("tasks", ConfigSource::Tracked, IssueKind::Unknown),
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
    assert!(local_policy.message().contains("tracked spice.toml"));
    assert!(error.issues().windows(2).all(|pair| {
        pair[0].path().map(ToString::to_string) <= pair[1].path().map(ToString::to_string)
    }));
}

#[test]
fn syntax_positions_and_other_layer_diagnostics_are_both_returned() {
    let error = resolve(Inputs::new(
        Some(Document::new(
            "config/spice.toml",
            "# caf\u{e9}\n[lane]\nlease = [\n",
        )),
        Some(Document::new(".spice/local.toml", "[unknown]\nvalue = 1\n")),
        &[],
    ))
    .expect_err("syntax and unknown table refuse");

    let syntax = error
        .issues()
        .iter()
        .find(|issue| issue.kind() == IssueKind::Syntax)
        .expect("syntax issue");
    assert_eq!(syntax.document(), "config/spice.toml");
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
    let tracked = "[listen]\nwindow = \"2m\"\n[lock.alpha]\nshards = 2\n";
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
