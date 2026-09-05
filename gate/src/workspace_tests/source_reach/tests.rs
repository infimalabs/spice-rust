use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[test]
fn declared_sources_distinguish_private_public_reexported_and_test_only_modules() {
    let fixture = Fixture::new(
        "visibility",
        &[
            (
                "lib.rs",
                concat!(
                    "mod private;\n",
                    "pub mod public;\n",
                    "mod exposed;\n",
                    "pub use exposed::Thing;\n",
                    "#[cfg(test)] mod tests;\n",
                ),
            ),
            ("private.rs", "pub mod nested;\n"),
            ("private/nested.rs", "pub struct Hidden;\n"),
            ("public.rs", "pub mod nested;\n"),
            ("public/nested.rs", "pub struct Visible;\n"),
            ("exposed.rs", "pub struct Thing;\n"),
            ("tests.rs", "pub mod support;\n"),
            ("tests/support.rs", "pub struct TestOnly;\n"),
        ],
    );

    assert_eq!(
        fixture.reach(),
        BTreeMap::from([
            ("exposed.rs".to_owned(), (false, true)),
            ("private.rs".to_owned(), (false, false)),
            ("private/nested.rs".to_owned(), (false, false)),
            ("public.rs".to_owned(), (false, true)),
            ("public/nested.rs".to_owned(), (false, true)),
            ("tests.rs".to_owned(), (true, false)),
            ("tests/support.rs".to_owned(), (true, false)),
        ]),
    );
}

#[test]
fn unresolved_public_reexports_conservatively_reach_every_production_source() {
    let fixture = Fixture::new(
        "uncertain-reexport",
        &[
            (
                "lib.rs",
                concat!(
                    "pub mod api;\n",
                    "mod private;\n",
                    "#[cfg(test)] mod tests;\n",
                ),
            ),
            ("api.rs", "pub use super::*;\nmod hidden;\n"),
            ("api/hidden.rs", "pub struct Hidden;\n"),
            ("private.rs", "pub struct Private;\n"),
            ("tests.rs", "pub struct TestOnly;\n"),
        ],
    );

    assert_eq!(
        fixture.reach(),
        BTreeMap::from([
            ("api.rs".to_owned(), (false, true)),
            ("api/hidden.rs".to_owned(), (false, true)),
            ("private.rs".to_owned(), (false, true)),
            ("tests.rs".to_owned(), (true, false)),
        ]),
    );
}

#[test]
fn nested_public_reexports_follow_only_the_exported_module_path() {
    let fixture = Fixture::new(
        "nested-reexport",
        &[
            (
                "lib.rs",
                concat!(
                    "mod facade;\n",
                    "mod unrelated;\n",
                    "pub use facade::nested::Thing;\n",
                ),
            ),
            ("facade.rs", "pub mod nested;\nmod private;\n"),
            ("facade/nested.rs", "pub struct Thing;\n"),
            ("facade/private.rs", "pub struct Private;\n"),
            ("unrelated.rs", "pub struct Unrelated;\n"),
        ],
    );

    assert_eq!(
        fixture.reach(),
        BTreeMap::from([
            ("facade.rs".to_owned(), (false, true)),
            ("facade/nested.rs".to_owned(), (false, true)),
            ("facade/private.rs".to_owned(), (false, false)),
            ("unrelated.rs".to_owned(), (false, false)),
        ]),
    );
}

#[test]
fn textual_includes_inherit_the_enclosing_public_reexport_paths() {
    let fixture = Fixture::new(
        "included-declaration",
        &[
            (
                "lib.rs",
                "pub use hidden::Thing;\ninclude!(\"declarations.rs\");\n",
            ),
            ("declarations.rs", "mod hidden;\n"),
            ("hidden.rs", "pub struct Thing;\n"),
        ],
    );

    assert_eq!(
        fixture.reach(),
        BTreeMap::from([
            ("declarations.rs".to_owned(), (false, true)),
            ("hidden.rs".to_owned(), (false, true)),
        ]),
    );
}

#[test]
fn repeated_source_declarations_retain_each_public_reexport_path() {
    let fixture = Fixture::new(
        "repeated-declaration",
        &[
            (
                "lib.rs",
                concat!(
                    "#[path = \"shared.rs\"] mod left;\n",
                    "#[path = \"shared.rs\"] mod right;\n",
                    "pub use left::a::A;\n",
                    "pub use right::b::B;\n",
                ),
            ),
            ("shared.rs", "mod a;\nmod b;\n"),
            ("shared/a.rs", "pub struct A;\n"),
            ("shared/b.rs", "pub struct B;\n"),
        ],
    );

    assert_eq!(
        fixture.reach(),
        BTreeMap::from([
            ("shared.rs".to_owned(), (false, true)),
            ("shared/a.rs".to_owned(), (false, true)),
            ("shared/b.rs".to_owned(), (false, true)),
        ]),
    );
}

#[test]
fn item_reexports_do_not_publish_an_entire_private_facade_namespace() {
    let fixture = Fixture::new(
        "private-facade",
        &[
            ("lib.rs", "mod facade;\npub use facade::Thing;\n"),
            (
                "facade.rs",
                concat!(
                    "mod implementation;\n",
                    "pub mod unrelated;\n",
                    "pub use implementation::Internal as Thing;\n",
                ),
            ),
            ("facade/implementation.rs", "pub struct Internal;\n"),
            ("facade/unrelated.rs", "pub struct Unrelated;\n"),
        ],
    );

    assert_eq!(
        fixture.reach(),
        BTreeMap::from([
            ("facade.rs".to_owned(), (false, true)),
            ("facade/implementation.rs".to_owned(), (false, true)),
            ("facade/unrelated.rs".to_owned(), (false, false)),
        ]),
    );
}

#[test]
fn reexported_module_namespaces_publish_their_public_descendants() {
    let fixture = Fixture::new(
        "reexported-namespace",
        &[
            ("lib.rs", "mod facade;\npub use facade::api;\n"),
            ("facade.rs", "pub mod api;\npub mod unrelated;\n"),
            ("facade/api.rs", "pub mod nested;\n"),
            ("facade/api/nested.rs", "pub struct Public;\n"),
            ("facade/unrelated.rs", "pub struct Unrelated;\n"),
        ],
    );

    assert_eq!(
        fixture.reach(),
        BTreeMap::from([
            ("facade.rs".to_owned(), (false, true)),
            ("facade/api.rs".to_owned(), (false, true)),
            ("facade/api/nested.rs".to_owned(), (false, true)),
            ("facade/unrelated.rs".to_owned(), (false, false)),
        ]),
    );
}

#[test]
fn an_unparseable_declared_source_refuses_the_graph() {
    let fixture = Fixture::new(
        "unparseable",
        &[("lib.rs", "mod broken;\n"), ("broken.rs", "not Rust {{")],
    );

    let error = declared_path_sources(&fixture.root, &fixture.root.join("lib.rs"))
        .expect_err("an unparseable declared source must refuse");
    assert!(
        error.to_string().contains("parse Rust source"),
        "unexpected refusal: {error}",
    );
}

#[test]
fn built_in_file_macros_are_discovered_in_every_rust_token_context() {
    let fixture = Fixture::new(
        "file-macros",
        &[
            (
                "crate/lib.rs",
                concat!(
                    "const CONFIG: &str = include_str!(\"../spice-rust.toml\");\n",
                    "const CONFIG_AGAIN: &str = std::r#include_str!(\"../spice-rust.toml\");\n",
                    "const TASTES: &[u8] = ::core::include_bytes!(\"../TASTES.md\",);\n",
                    "const EXPRESSION: &str = std::include!(\"../expression.rs\");\n",
                    "#[doc = ::std::include_str!(\"../release-authority/VERSION\")]\n",
                    "pub fn documented() {}\n",
                    "#[fixture(core::include_str!(\"../attribute.txt\"))]\n",
                    "pub struct Attributed;\n",
                    "outer!(nested(std::include_str!(\"../nested.txt\")));\n",
                    "outer!(nested(::r#std::r#include_str!(\"../global.txt\")));\n",
                    "outer!(nested(custom::include_str!(\"../missing-custom.txt\")));\n",
                    "crate::std::include_str!(\"../missing-nested-std.txt\");\n",
                    "macro_rules! generated { () => { ::core::include_bytes!(\"../generated.bin\") } }\n",
                    "#[cfg(test)] const TEST_ONLY: &str = include_str!(\"../test-only.txt\");\n",
                    "::std::include!(\"../declarations.rs\");\n",
                    "const IGNORED: &str = r#\"include_str!(\\\"missing-string.txt\\\")\"#;\n",
                    "// include_bytes!(\"missing-comment.bin\")\n",
                ),
            ),
            ("spice-rust.toml", "[workspace]\n"),
            ("TASTES.md", "tastes\n"),
            ("expression.rs", "include_str!(\"expression.txt\")"),
            ("expression.txt", "expression\n"),
            ("release-authority/VERSION", "1.2.3\n"),
            ("attribute.txt", "attribute\n"),
            ("nested.txt", "nested\n"),
            ("global.txt", "global\n"),
            ("generated.bin", "generated\n"),
            ("test-only.txt", "tests\n"),
            (
                "declarations.rs",
                "const RELEASE: &str = include_str!(\"release/authority.txt\");\n",
            ),
            ("release/authority.txt", "release\n"),
        ],
    );

    assert_eq!(
        fixture.reach_from("crate/lib.rs"),
        BTreeMap::from([
            ("TASTES.md".to_owned(), (false, true)),
            ("attribute.txt".to_owned(), (false, true)),
            ("declarations.rs".to_owned(), (false, true)),
            ("expression.rs".to_owned(), (false, true)),
            ("expression.txt".to_owned(), (false, true)),
            ("global.txt".to_owned(), (false, true)),
            ("generated.bin".to_owned(), (false, true)),
            ("nested.txt".to_owned(), (false, true)),
            ("release-authority/VERSION".to_owned(), (false, true)),
            ("release/authority.txt".to_owned(), (false, true)),
            ("spice-rust.toml".to_owned(), (false, true)),
            ("test-only.txt".to_owned(), (true, false)),
        ]),
    );
}

#[test]
fn include_bytes_accepts_binary_while_textual_file_macros_require_utf8() {
    let binary = [0xff, 0xfe, 0xfd];
    let bytes_fixture = Fixture::new(
        "binary-bytes",
        &[(
            "lib.rs",
            "const DATA: &[u8] = include_bytes!(\"data.bin\");\n",
        )],
    );
    fs::write(bytes_fixture.root.join("data.bin"), binary).expect("write binary authority");
    assert_eq!(
        bytes_fixture.reach(),
        BTreeMap::from([("data.bin".to_owned(), (false, true))]),
    );

    let text_fixture = Fixture::new(
        "binary-text",
        &[("lib.rs", "const DATA: &str = include_str!(\"data.bin\");\n")],
    );
    fs::write(text_fixture.root.join("data.bin"), binary).expect("write binary authority");
    let error = declared_path_sources(&text_fixture.root, &text_fixture.root.join("lib.rs"))
        .expect_err("include_str must reject non-UTF-8 authority");
    assert!(error.to_string().contains("is not UTF-8"), "{error}");
}

#[test]
fn bounded_out_dir_file_macros_are_generated_authority_not_selected_tree_paths() {
    let fixture = Fixture::new(
        "out-dir-file-macro",
        &[(
            "lib.rs",
            concat!(
                "include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\n",
                "const TEXT: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/text.txt\"));\n",
            ),
        )],
    );

    assert!(
        declared_path_sources(&fixture.root, &fixture.root.join("lib.rs"))
            .expect("bounded OUT_DIR paths defer to executable authority")
            .is_empty()
    );
}

#[test]
fn dynamic_missing_nonregular_and_escaping_file_macro_inputs_refuse() {
    let dynamic = Fixture::new(
        "dynamic-file-macro",
        &[(
            "lib.rs",
            "const DATA: &str = include_str!(concat!(\"a\", \".txt\"));\n",
        )],
    );
    assert_refusal(&dynamic, "non-literal include_str! path");

    let missing = Fixture::new(
        "missing-file-macro",
        &[(
            "lib.rs",
            "const DATA: &str = include_str!(\"missing.txt\");\n",
        )],
    );
    assert_refusal(&missing, "is not readable selected-tree authority");

    let directory = Fixture::new(
        "directory-file-macro",
        &[("lib.rs", "const DATA: &[u8] = include_bytes!(\"input\");\n")],
    );
    fs::create_dir(directory.root.join("input")).expect("create nonregular authority");
    assert_refusal(&directory, "is not a regular file");

    let escape = Fixture::new("escaping-file-macro", &[("crate/lib.rs", "")]);
    let outside_name = format!(
        "{}-outside.txt",
        escape
            .root
            .file_name()
            .expect("fixture root has a name")
            .to_string_lossy()
    );
    fs::write(
        escape.root.join("crate/lib.rs"),
        format!("const DATA: &str = include_str!(\"../../{outside_name}\");\n"),
    )
    .expect("write escaping file macro");
    let outside = escape
        .root
        .parent()
        .expect("fixture has parent")
        .join(&outside_name);
    fs::write(&outside, "outside\n").expect("write escaping authority");
    let error = declared_path_sources(&escape.root, &escape.root.join("crate/lib.rs"))
        .expect_err("selected-root escape must refuse");
    assert!(
        error.to_string().contains("escapes selected root"),
        "{error}"
    );
    fs::remove_file(outside).expect("remove escaping authority");
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_file_macro_authority_paths_refuse() {
    use std::os::unix::ffi::OsStringExt as _;

    let fixture = Fixture::new("non-utf8-file-macro", &[("lib.rs", "")]);
    let source_dir = fixture
        .root
        .join(std::ffi::OsString::from_vec(b"non-utf8-\xff".to_vec()));
    fs::create_dir(&source_dir).expect("create non-UTF-8 source directory");
    fs::write(
        source_dir.join("lib.rs"),
        "const DATA: &str = include_str!(\"authority.txt\");\n",
    )
    .expect("write source beneath non-UTF-8 path");
    fs::write(source_dir.join("authority.txt"), "authority\n")
        .expect("write authority beneath non-UTF-8 path");

    let error = declared_path_sources(&fixture.root, &source_dir.join("lib.rs"))
        .expect_err("non-UTF-8 selected authority path must refuse");
    assert!(error.to_string().contains("is not UTF-8"), "{error}");
}

#[cfg(unix)]
#[test]
fn symlink_file_macro_inputs_refuse_even_when_the_target_is_selected() {
    let fixture = Fixture::new(
        "symlink-file-macro",
        &[
            (
                "lib.rs",
                "const DATA: &str = include_str!(\"alias.txt\");\n",
            ),
            ("authority.txt", "authority\n"),
        ],
    );
    std::os::unix::fs::symlink("authority.txt", fixture.root.join("alias.txt"))
        .expect("create file-macro symlink");
    assert_refusal(&fixture, "is not a regular file");
}

fn assert_refusal(fixture: &Fixture, expected: &str) {
    let error = declared_path_sources(&fixture.root, &fixture.root.join("lib.rs"))
        .expect_err("invalid file-macro authority must refuse");
    assert!(
        error.to_string().contains(expected),
        "unexpected refusal: {error}"
    );
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str, files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "gate-source-reach-{label}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&root).expect("create source reach fixture root");
        let root = fs::canonicalize(root).expect("canonicalize source reach fixture root");
        for (path, contents) in files {
            let path = root.join(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create source reach fixture directory");
            }
            fs::write(path, contents).expect("write source reach fixture");
        }
        Self { root }
    }

    fn reach(&self) -> BTreeMap<String, (bool, bool)> {
        self.reach_from("lib.rs")
    }

    fn reach_from(&self, target: &str) -> BTreeMap<String, (bool, bool)> {
        declared_path_sources(&self.root, &self.root.join(target))
            .expect("fixture source graph resolves")
            .into_iter()
            .map(|source| {
                let path = source
                    .path
                    .strip_prefix(&self.root)
                    .expect("declared source stays in fixture")
                    .to_string_lossy()
                    .into_owned();
                (path, (source.test_only, source.dependent_tests))
            })
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove source reach fixture root");
    }
}
