//! Host-only verdict imports stay behind the wasm boundary they depend on.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use syn::{Item, Meta, UseTree};

const HOST_IMPORTS: [&str; 3] = ["CommittedTexts", "scan_jurisdiction", "tracked_paths"];

#[test]
fn verdict_host_imports_carry_their_items_wasm_boundary() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/verdict.rs");
    let source = fs::read_to_string(&path).expect("read verdict source");
    let file = syn::parse_file(&source).expect("parse verdict source");
    let expected = HOST_IMPORTS.into_iter().collect::<BTreeSet<_>>();
    let mut found = BTreeSet::new();

    for item in file.items {
        let Item::Use(item) = item else {
            continue;
        };
        let mut names = BTreeSet::new();
        imported_names(&item.tree, &mut names);
        let guarded = names
            .iter()
            .filter(|name| expected.contains(name.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if guarded.is_empty() {
            continue;
        }
        assert!(
            item.attrs.iter().any(excludes_wasm),
            "host-only verdict imports lack cfg(not(target_arch = \"wasm32\")): {}",
            guarded.join(", ")
        );
        found.extend(guarded);
    }

    assert_eq!(
        found.iter().map(String::as_str).collect::<BTreeSet<_>>(),
        expected,
        "the fixture must inspect every host-only verdict import"
    );
}

fn imported_names(tree: &UseTree, names: &mut BTreeSet<String>) {
    match tree {
        UseTree::Path(path) => imported_names(&path.tree, names),
        UseTree::Name(name) => {
            names.insert(name.ident.to_string());
        }
        UseTree::Rename(rename) => {
            names.insert(rename.ident.to_string());
        }
        UseTree::Group(group) => {
            for item in &group.items {
                imported_names(item, names);
            }
        }
        UseTree::Glob(_) => {}
    }
}

fn excludes_wasm(attribute: &syn::Attribute) -> bool {
    attribute.path().is_ident("cfg")
        && matches!(
            &attribute.meta,
            Meta::List(meta) if meta.tokens.to_string() == "not (target_arch = \"wasm32\")"
        )
}
