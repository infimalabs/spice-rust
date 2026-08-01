//! The retirement ledger's jurisdiction.
//!
//! NAMES.md scopes this deny-list in the sentence directly beneath its own
//! Retired table: the table is the source of the gate deny-list over public
//! symbols, config keys, and clap names. Jurisdiction is therefore the names
//! this port *coins* — item, field, and variant declarations, and the spellings
//! serde and clap publish for them. Those are chosen deliberately, so a retired
//! word standing in one is a retired word shipped.
//!
//! Everything else is incidental text that shares an alphabet with the ledger
//! and nothing else. A doc comment says "the lifetime of the borrow" because
//! that is what Rust calls it; a class attribute says `flex` and `sticky`
//! because that is what CSS calls them; a local binding is invisible past its
//! own block. Judging those refuses correct code and answers with advice that
//! means nothing where it landed — "use ratchet" for a stylesheet class.
//!
//! Foreign skin is exempt for the reason NAMES.md gives directly: an external
//! crate name is its owner's convention, carried verbatim and coined never. So
//! `use` and `extern crate` paths are not judged.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use gate::retired_matches;
use syn::punctuated::Punctuated;
use syn::{Meta, Token};

/// Attributes that publish a spelling rather than describe one.
const NAMING_ATTRIBUTES: &[&str] = &["serde", "clap", "command", "arg"];

/// Keys inside those attributes whose value is a published name.
const NAMING_KEYS: &[&str] = &["rename", "alias", "name", "long", "value_name"];

#[test]
fn workspace_rust_uses_only_current_public_vocabulary() {
    let root = workspace_root();
    let mut sources = Vec::new();
    collect_rust_sources(&root, &mut sources);
    sources.sort();

    let mut violations = Vec::new();
    for path in &sources {
        let source = fs::read_to_string(path).expect("Rust source must be readable");
        let location = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();
        violations.extend(scan_source(&location, &source));
    }

    assert!(
        violations.is_empty(),
        "retired public vocabulary found:\n{}",
        violations.join("\n")
    );
}

/// The sweep above is enforcement, not an observation that today's tree is
/// clean.
///
/// Without this fixture, emptying the walker or dropping every visit leaves the
/// suite green while the gate guards nothing.
#[test]
fn an_introduced_retired_name_is_refused() {
    let violations = scan_source("fixture.rs", "pub struct Team;");

    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("lineage"), "{violations:?}");
}

#[test]
fn a_published_config_key_is_still_judged() {
    let source = "pub struct Row {\n    #[serde(rename = \"unclaim\")]\n    pub freed: bool,\n}\n";
    let violations = scan_source("fixture.rs", source);

    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("free"), "{violations:?}");
}

#[test]
fn a_published_flag_name_is_still_judged() {
    let source = "pub struct Args {\n    #[arg(long = \"unclaim\")]\n    pub freed: bool,\n}\n";
    let violations = scan_source("fixture.rs", source);

    assert_eq!(violations.len(), 1, "{violations:?}");
}

#[test]
fn borrow_prose_in_a_doc_comment_is_not_a_coined_name() {
    let source = "/// Bounded by the lifetime of the borrow.\npub struct Holder;\n";

    assert!(scan_source("fixture.rs", source).is_empty());
}

#[test]
fn a_stylesheet_class_is_not_a_coined_name() {
    let source = "pub fn row() -> &'static str {\n    \"flex items-center sticky top-0\"\n}\n";

    assert!(scan_source("fixture.rs", source).is_empty());
}

#[test]
fn a_local_binding_is_not_a_coined_name() {
    let source = "fn helper() {\n    let driver = 1;\n    let _ = driver;\n}\n";

    assert!(scan_source("fixture.rs", source).is_empty());
}

#[test]
fn a_foreign_crate_keeps_its_skin() {
    let source = "use some_driver_crate::thing;\n";

    assert!(scan_source("fixture.rs", source).is_empty());
}

/// One concept is one finding, however many times a file spells it.
#[test]
fn a_concept_named_repeatedly_is_reported_once() {
    let source = "pub struct Team;\npub fn team() {}\npub enum Roster {\n    Team,\n}\n";
    let violations = scan_source("fixture.rs", source);

    assert_eq!(violations.len(), 1, "{violations:?}");
}

#[test]
fn retired_vocabulary_is_refused_while_its_successor_is_admitted() {
    let matches = retired_matches("team");

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].successor, "lineage");
    assert!(retired_matches("lineage").is_empty());
}

/// One file's findings, deduplicated so a concept is named once.
struct Scan {
    location: String,
    violations: BTreeSet<String>,
}

impl Scan {
    fn new(location: &str) -> Self {
        Self {
            location: location.to_owned(),
            violations: BTreeSet::new(),
        }
    }

    fn coined(&mut self, candidate: &str) {
        for found in retired_matches(candidate) {
            self.violations.insert(format!(
                "{}: {:?} is retired; use {:?}",
                self.location, found.retired, found.successor
            ));
        }
    }

    fn name(&mut self, identifier: &syn::Ident) {
        self.coined(&identifier.to_string());
    }
}

fn scan_source(location: &str, source: &str) -> Vec<String> {
    let syntax =
        syn::parse_file(source).unwrap_or_else(|error| panic!("{location} must parse: {error}"));
    let mut scan = Scan::new(location);
    scan_items(&mut scan, &syntax.items);
    scan.violations.into_iter().collect()
}

fn scan_items(scan: &mut Scan, items: &[syn::Item]) {
    for item in items {
        scan_item(scan, item);
    }
}

fn scan_item(scan: &mut Scan, item: &syn::Item) {
    match item {
        syn::Item::Struct(node) => {
            scan.name(&node.ident);
            scan_attributes(scan, &node.attrs);
            scan_fields(scan, &node.fields);
        }
        syn::Item::Enum(node) => {
            scan.name(&node.ident);
            scan_attributes(scan, &node.attrs);
            for variant in &node.variants {
                scan.name(&variant.ident);
                scan_attributes(scan, &variant.attrs);
                scan_fields(scan, &variant.fields);
            }
        }
        syn::Item::Union(node) => {
            scan.name(&node.ident);
            scan_attributes(scan, &node.attrs);
            for field in &node.fields.named {
                scan_field(scan, field);
            }
        }
        syn::Item::Fn(node) => {
            scan.name(&node.sig.ident);
            scan_attributes(scan, &node.attrs);
        }
        syn::Item::Mod(node) => {
            scan.name(&node.ident);
            scan_attributes(scan, &node.attrs);
            if let Some((_, items)) = &node.content {
                scan_items(scan, items);
            }
        }
        syn::Item::Trait(node) => {
            scan.name(&node.ident);
            scan_attributes(scan, &node.attrs);
            for member in &node.items {
                scan_trait_item(scan, member);
            }
        }
        syn::Item::Impl(node) => {
            for member in &node.items {
                scan_impl_item(scan, member);
            }
        }
        syn::Item::Const(node) => {
            scan.name(&node.ident);
            scan_attributes(scan, &node.attrs);
        }
        syn::Item::Static(node) => {
            scan.name(&node.ident);
            scan_attributes(scan, &node.attrs);
        }
        syn::Item::Type(node) => {
            scan.name(&node.ident);
            scan_attributes(scan, &node.attrs);
        }
        syn::Item::Macro(node) => {
            if let Some(identifier) = &node.ident {
                scan.name(identifier);
            }
        }
        _ => {}
    }
}

fn scan_trait_item(scan: &mut Scan, member: &syn::TraitItem) {
    match member {
        syn::TraitItem::Const(node) => scan.name(&node.ident),
        syn::TraitItem::Fn(node) => scan.name(&node.sig.ident),
        syn::TraitItem::Type(node) => scan.name(&node.ident),
        _ => {}
    }
}

fn scan_impl_item(scan: &mut Scan, member: &syn::ImplItem) {
    match member {
        syn::ImplItem::Const(node) => scan.name(&node.ident),
        syn::ImplItem::Fn(node) => scan.name(&node.sig.ident),
        syn::ImplItem::Type(node) => scan.name(&node.ident),
        _ => {}
    }
}

fn scan_fields(scan: &mut Scan, fields: &syn::Fields) {
    for field in fields {
        scan_field(scan, field);
    }
}

fn scan_field(scan: &mut Scan, field: &syn::Field) {
    if let Some(identifier) = &field.ident {
        scan.name(identifier);
    }
    scan_attributes(scan, &field.attrs);
}

/// Published spellings carried in attributes, such as a serde rename or a clap
/// flag, which name a surface without ever appearing as an identifier.
fn scan_attributes(scan: &mut Scan, attributes: &[syn::Attribute]) {
    for attribute in attributes {
        if !NAMING_ATTRIBUTES
            .iter()
            .any(|name| attribute.path().is_ident(name))
        {
            continue;
        }
        let Ok(entries) =
            attribute.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
        else {
            continue;
        };
        for entry in entries {
            let Meta::NameValue(pair) = entry else {
                continue;
            };
            if !NAMING_KEYS.iter().any(|key| pair.path.is_ident(key)) {
                continue;
            }
            if let syn::Expr::Lit(expression) = &pair.value
                && let syn::Lit::Str(text) = &expression.lit
            {
                scan.coined(&text.value());
            }
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("gate must be a direct workspace member")
        .to_path_buf()
}

fn collect_rust_sources(directory: &Path, sources: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("workspace directory must be readable") {
        let entry = entry.expect("workspace entry must be readable");
        let path = entry.path();
        let file_type = entry.file_type().expect("workspace entry must have a type");
        if file_type.is_dir() && !is_ignored_directory(&path) {
            collect_rust_sources(&path, sources);
        } else if file_type.is_file() && path.extension().is_some_and(|value| value == "rs") {
            sources.push(path);
        }
    }
}

fn is_ignored_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "target" || name.starts_with('.'))
}
