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
