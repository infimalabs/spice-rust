use super::*;

#[test]
fn repeated_metavariable_macro_paths_refuse_but_harmless_bodies_remain_admitted() {
    let repeated = Fixture::new(
        "repeated-metavariable-macro-path",
        &[
            (
                "lib.rs",
                concat!(
                    "macro_rules! outer {\n",
                    "    ($($macro_name:ident),+) => { $(const _: &str = $macro_name!(\"authority.txt\");)+ };\n",
                    "}\n",
                    "outer!(include_str);\n",
                ),
            ),
            ("authority.txt", "authority\n"),
        ],
    );
    repeated.assert_compiles("lib.rs");
    let error = repeated
        .reach_result_from("lib.rs", FileMacroCrateContext::test_default())
        .expect_err("repeated metavariable macro paths must refuse");
    assert!(
        error
            .to_string()
            .contains("macro path containing $macro_name"),
        "unexpected refusal: {error}",
    );

    let harmless = Fixture::new(
        "harmless-opaque-macro-body",
        &[(
            "lib.rs",
            "macro_rules! stringify_value { ($value:expr) => { stringify!($value) }; }\npub const VALUE: &str = stringify_value!(authority);\n",
        )],
    );
    harmless.assert_compiles("lib.rs");
    assert!(harmless.reach().is_empty());
}
