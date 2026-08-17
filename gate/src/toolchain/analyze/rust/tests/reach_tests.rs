use super::*;

#[test]
fn native_process_receives_only_explicit_reached_packages() {
    let fixture = RustFixture::new("selected-arguments");
    fixture.source(REMOVED_PLANT);
    fixture.add_reach_packages();
    let (cargo, log) = fixture.logging_cargo();

    let analysis = fixture.analyze_with(
        cargo.to_str().expect("UTF-8 logging Cargo"),
        &rust_bounds(),
        None,
    );

    assert_eq!(analysis.outcome, ArmOutcome::Passed);
    assert_eq!(
        fs::read_to_string(log).expect("read native Cargo arguments"),
        concat!(
            "check --locked --all-targets ",
            "--package consumer --package planted\n"
        )
    );
}

#[test]
fn native_process_receives_only_the_reached_direct_target() {
    let fixture = RustFixture::new("selected-target");
    fixture.source(REMOVED_PLANT);
    fs::create_dir(fixture.root.join("tests")).expect("create integration target directory");
    for target in ["selected", "unrelated"] {
        fs::write(
            fixture.root.join(format!("tests/{target}.rs")),
            "#[test]\nfn fixture() {}\n",
        )
        .expect("write integration target");
    }
    let (cargo, log) = fixture.logging_cargo();
    let analysis = fixture.analyze_changes_with(
        &["tests/selected.rs".to_owned()],
        cargo.to_str().expect("UTF-8 logging Cargo"),
        &rust_bounds(),
        None,
    );

    assert_eq!(analysis.outcome, ArmOutcome::Passed);
    assert_eq!(
        fs::read_to_string(log).expect("read targeted Cargo arguments"),
        "check --locked --package planted --test selected\n"
    );
}

#[test]
fn native_process_enables_only_the_reached_targets_required_feature() {
    let fixture = RustFixture::new("required-feature-target");
    fixture.source(REMOVED_PLANT);
    fs::create_dir(fixture.root.join("tests")).expect("create guarded target directory");
    fs::write(
        fixture.root.join("Cargo.toml"),
        format!(
            concat!(
                "{}\n",
                "[features]\nguarded = []\nunrelated = []\n\n",
                "[[test]]\nname = \"guarded\"\npath = \"tests/guarded.rs\"\n",
                "required-features = [\"guarded\"]\n\n",
                "[[test]]\nname = \"unrelated\"\npath = \"tests/unrelated.rs\"\n",
                "required-features = [\"unrelated\"]\n",
            ),
            FIXTURE_MANIFEST
        ),
    )
    .expect("write guarded target manifest");
    fs::write(
        fixture.root.join("tests/guarded.rs"),
        "fn guarded() { missing_guarded_symbol(); }\n",
    )
    .expect("write guarded target source");
    fs::write(
        fixture.root.join("tests/unrelated.rs"),
        "compile_error!(\"unrelated target compiled\");\n",
    )
    .expect("write unrelated guarded target source");
    let changes = ["tests/guarded.rs".to_owned()];
    let (cargo, log) = fixture.logging_cargo();

    let logged = fixture.analyze_changes_with(
        &changes,
        cargo.to_str().expect("UTF-8 logging Cargo"),
        &rust_bounds(),
        None,
    );

    assert_eq!(logged.outcome, ArmOutcome::Passed);
    assert_eq!(
        fs::read_to_string(log).expect("read guarded Cargo arguments"),
        concat!(
            "check --locked --package planted --test guarded ",
            "--features planted/guarded\n",
        ),
    );

    let observed = fixture.analyze_changes_with(&changes, env!("CARGO"), &rust_bounds(), None);

    assert_eq!(observed.outcome, ArmOutcome::Findings);
    let [finding] = observed.findings.as_slice() else {
        panic!(
            "guarded compiler failure must be one finding, got {}",
            observed.findings.len()
        );
    };
    assert_eq!(finding.path.as_deref(), Some("tests/guarded.rs"));
    let FindingDetail::RustTypeOutput { message, .. } = &finding.detail else {
        panic!("guarded compiler finding retains Rust detail: {finding:?}");
    };
    assert!(message.contains("missing_guarded_symbol"), "{message}");
}
