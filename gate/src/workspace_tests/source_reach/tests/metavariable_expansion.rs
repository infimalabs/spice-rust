use super::*;

#[test]
fn opaque_macro_bodies_refuse_metavariable_macro_paths() {
    for (label, fragment, transcriber, invocation) in [
        (
            "metavariable-ident",
            "ident",
            "$macro_name!(\"authority.txt\")",
            "include_str",
        ),
        (
            "metavariable-token",
            "tt",
            "$macro_name!(\"authority.txt\")",
            "include_str",
        ),
        (
            "metavariable-namespace",
            "ident",
            "$macro_name::include_str!(\"authority.txt\")",
            "std",
        ),
        (
            "metavariable-final-segment",
            "ident",
            "std::$macro_name!(\"authority.txt\")",
            "include_str",
        ),
        (
            "metavariable-non-file-call",
            "ident",
            "$macro_name!(authority)",
            "stringify",
        ),
    ] {
        let source = format!(
            "macro_rules! outer {{ ($macro_name:{fragment}) => {{ pub const VALUE: &str = {transcriber}; }}; }}\nouter!({invocation});\n"
        );
        let fixture = Fixture::new(
            label,
            &[("lib.rs", &source), ("authority.txt", "authority\n")],
        );
        fixture.assert_compiles("lib.rs");
        let error = fixture
            .reach_result_from("lib.rs", FileMacroCrateContext::test_default())
            .expect_err("metavariable macro paths have unresolved expansion authority");
        let detail = error.to_string();
        assert!(
            detail.contains(
                "cannot prove expansion-site file-macro namespace for macro path containing $macro_name"
            ),
            "unexpected refusal for {label}: {error}",
        );
    }
}

#[test]
fn path_fragment_macro_invocations_refuse_before_rust_expansion() {
    let fixture = Fixture::new(
        "path-fragment-macro-invocation",
        &[(
            "lib.rs",
            "macro_rules! outer { ($macro_name:path) => { $macro_name!(\"authority.txt\") }; }\nouter!(include_str);\n",
        )],
    );
    let error = fixture
        .reach_result_from("lib.rs", FileMacroCrateContext::test_default())
        .expect_err("path fragment in macro position has unresolved file-macro authority");
    assert!(
        error
            .to_string()
            .contains("macro path containing $macro_name"),
        "unexpected refusal: {error}",
    );
}

#[test]
fn renamed_non_file_compiler_macro_does_not_create_file_authority() {
    let fixture = Fixture::new(
        "renamed-non-file-compiler-macro",
        &[(
            "lib.rs",
            "use std::stringify as include_str;\npub const VALUE: &str = include_str!(missing.txt);\n",
        )],
    );
    assert!(fixture.reach().is_empty());
    fixture.assert_compiles("lib.rs");
}
