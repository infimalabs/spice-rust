use super::*;

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
