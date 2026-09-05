use super::*;

#[test]
fn transitive_compiler_namespace_input_reaches_the_cargo_shard() {
    let fixture = Fixture::new("transitive-file-macro-authority");
    fs::write(
        fixture.root.join("fixture/src/lib.rs"),
        concat!(
            "use ::std as standard;\n",
            "use standard as core;\n",
            "pub const AUTHORITY: &str = core::include_str!(\"../../file-macro-authority.txt\");\n",
        ),
    )
    .expect("write transitive file-macro source");
    let original = fixture.plan(true);
    assert!(
        original
            .typechecks()
            .iter()
            .find(|shard| shard.package() == "fixture")
            .expect("fixture typecheck shard is planned")
            .input_paths()
            .contains(&"file-macro-authority.txt".to_owned())
    );
    let authority = fixture.root.join("file-macro-authority.txt");
    fs::remove_file(&authority).expect("remove file-macro input");
    let error = fixture
        .try_plan(true)
        .expect_err("a removed compiler input must refuse Cargo planning");
    assert!(
        error
            .to_string()
            .contains("is not readable selected-tree authority"),
        "unexpected refusal: {error}",
    );
}
