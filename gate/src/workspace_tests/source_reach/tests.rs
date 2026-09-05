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
        declared_path_sources(&self.root.join("lib.rs"))
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
