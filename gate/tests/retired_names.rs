//! The retirement ledger's jurisdiction.
//!
//! [`RETIRED_NAMES`](gate::RETIRED_NAMES) is the whole deny-list, and it runs
//! over public symbols, config keys, and clap names. Jurisdiction is therefore
//! the names this port *coins* — item, field, and variant declarations, the
//! spellings serde and clap publish for them, and the command paths the CLI
//! contract table publishes. Those are chosen deliberately, so a retired word
//! standing in one is a retired word shipped.
//!
//! A command path is judged differently from the rest, and the difference is
//! the position and not the ledger. An identifier is one name the port assembled
//! out of whatever words it liked, so a retired word anywhere inside it is that
//! word coined. A command path is a leaf filed under a stem, so passing through
//! a word is not naming it; what a path can do is *be* the retired name. See
//! [`retired_command_matches`](gate::retired_command_matches).
//!
//! Everything else is incidental text that shares an alphabet with the ledger
//! and nothing else. A doc comment says "the lifetime of the borrow" because
//! that is what Rust calls it; a class attribute says `flex` and `sticky`
//! because that is what CSS calls them; a local binding is invisible past its
//! own block. Judging those refuses correct code and answers with advice that
//! means nothing where it landed — "use ratchet" for a stylesheet class.
//!
//! Foreign skin is exempt for a reason of its own: an external crate name is
//! its owner's convention, carried verbatim and coined never. Renaming it is
//! not something this port can do, so refusing it would be advice no one can
//! take. `use` and `extern crate` paths are therefore not judged.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use gate::{retired_command_matches, retired_matches};
use syn::punctuated::Punctuated;
use syn::{Meta, Token};

mod support;

use support::{Tree, create_scratch_root};

/// Attributes that publish a spelling rather than describe one.
const NAMING_ATTRIBUTES: &[&str] = &["serde", "clap", "command", "arg"];

/// Keys inside those attributes whose value is a published name.
const NAMING_KEYS: &[&str] = &["rename", "alias", "name", "long", "value_name"];

#[test]
fn workspace_rust_uses_only_current_public_vocabulary() {
    let violations = scan_tree(&workspace_root());

    assert!(
        violations.is_empty(),
        "retired public vocabulary found:\n{}",
        violations.join("\n")
    );
}

/// The walk itself, over a tree this test plants and owns.
///
/// The sweep above reads the whole workspace, so it can only fail by finding a
/// real violation. A clean tree and an empty walk are the same green to it, and
/// that is the difference this fixture holds: the plant is reachable only by
/// descending into a subdirectory and reading the file there. Emptying
/// `collect_rust_sources`, dropping the read loop, or widening
/// `is_ignored_directory` far enough to swallow `nested` each stop the plant
/// from being found and turn this red.
#[test]
fn the_walk_descends_to_a_planted_violation_and_names_where_it_sat() {
    let tree = Tree::new("walk");
    tree.write("surface.rs", "pub struct Lineage;\n");
    tree.write("nested/deep/planted.rs", "pub struct Team;\n");
    tree.write("target/generated.rs", "pub struct Team;\n");
    tree.write(".vendored/carried.rs", "pub struct Team;\n");

    let violations = scan_tree(&tree.root);
    let planted = Path::new("nested").join("deep").join("planted.rs");

    assert_eq!(
        violations,
        vec![format!(
            "{}: \"team\" is retired; use \"lineage\"",
            planted.display()
        )],
        "the walk did not report exactly the planted violation, located at the \
         path it was planted under",
    );

    // The walk is answered before the tree is torn down, so a teardown failure
    // reports as itself rather than standing in front of the claim this test
    // exists to make.
    tree.remove();
}

/// A scratch root is created, and a name already taken is skipped.
///
/// Both candidates here sit inside a tree this test owns, so the collision is
/// the one being tested rather than one another run happened to cause. The
/// stale tree holds a retired name, which is what adopting it would have handed
/// to the walk above as if the fixture had planted it.
#[test]
fn scratch_setup_creates_its_root_rather_than_adopting_one_that_exists() {
    let owner = Tree::new("owner");
    let taken = owner.path("taken");
    let free = owner.path("free");
    fs::create_dir(&taken).expect("plant a stale root");
    fs::write(taken.join("left-behind.rs"), "pub struct Team;\n").expect("plant a stale source");

    let root = create_scratch_root([taken.clone(), free.clone()].into_iter());

    assert_eq!(
        root, free,
        "a name already taken was adopted rather than skipped"
    );
    assert!(root.exists(), "the chosen root was named but never created");
    assert!(
        scan_tree(&root).is_empty(),
        "the created root carried sources it never wrote"
    );
    assert_eq!(
        scan_tree(&taken).len(),
        1,
        "the stale tree was disturbed rather than left where it was"
    );

    owner.remove();
}

/// Unwinding keeps the failure that caused it and still retires owned scratch.
///
/// The explicit teardown path above can state its own failure, but a second
/// panic from teardown would abort while the test is already unwinding. This
/// pins the quiet [`Drop`] path instead: the planted panic must remain the one
/// observed, and the root live at that instant must nonetheless be gone.
#[test]
fn scratch_drop_removes_its_root_without_replacing_the_original_panic() {
    const ORIGINAL_PANIC: &str = "planted panic before scratch teardown";

    let tree = Tree::new("unwind");
    let root = tree.root.clone();

    let panic = std::panic::catch_unwind(|| {
        let live_tree = tree;
        assert!(
            live_tree.root.exists(),
            "the scratch root was not live before the planted panic"
        );
        std::panic::panic_any(ORIGINAL_PANIC);
    })
    .expect_err("the planted panic was not observed after unwinding");

    assert_eq!(
        panic.downcast_ref::<&str>().copied(),
        Some(ORIGINAL_PANIC),
        "scratch teardown replaced the original panic"
    );
    assert!(
        !root.exists(),
        "the owned scratch root survived panic unwinding at {}",
        root.display()
    );
}

/// Every violation in one tree, each located relative to that tree's root.
fn scan_tree(root: &Path) -> Vec<String> {
    let mut sources = Vec::new();
    collect_rust_sources(root, &mut sources);
    sources.sort();

    let mut violations = Vec::new();
    for path in &sources {
        let source = fs::read_to_string(path).expect("Rust source must be readable");
        let location = path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string();
        violations.extend(scan_source(&location, &source));
    }
    violations
}

/// The sweep above is enforcement, not an observation that today's tree is
/// clean.
///
/// Without this fixture, emptying the walker or dropping every visit leaves the
/// suite green while the gate guards nothing.
#[test]
fn a_genuine_retirement_is_refused_while_an_upper_bound_is_admitted() {
    let source = "pub struct Team;\npub const HARD_CEILING: usize = 3;\n";
    let violations = scan_source("fixture.rs", source);

    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("lineage"), "{violations:?}");
    assert!(retired_matches("hard_ceiling").is_empty());
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

#[test]
fn exact_predecessor_lineage_keys_and_provenance_are_foreign_skin_only() {
    let foreign = concat!(
        "pub const COLLECTION_KEY: &str = \"teams\";\n",
        "pub const IDENTITY_KEY: &str = \"teamId\";\n",
        "pub const SOURCE_PATH: &str = \"teams[0].teamId\";\n",
        "pub struct Lineage;\n",
    );
    assert!(scan_source("fixture.rs", foreign).is_empty());

    let current = "pub struct MigrationRow { pub team: String }\n";
    let violations = scan_source("fixture.rs", current);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("lineage"), "{violations:?}");
}

/// One concept is one finding, however many times a file spells it.
#[test]
fn a_concept_named_repeatedly_is_reported_once() {
    let source = "pub struct Team;\npub fn team() {}\npub enum Roster {\n    Team,\n}\n";
    let violations = scan_source("fixture.rs", source);

    assert_eq!(violations.len(), 1, "{violations:?}");
}

/// The verb is refused where it is published and admitted where it is declared.
///
/// The refusal exemplar is a contract row, not a function-style or type-style
/// spelling, because the row is what a port of `spice task ingest` would
/// actually write and the earlier exemplars could not fail for it. Both halves
/// are load-bearing and both were shown so: matching a command path by
/// contained word instead of whole reds the third fixture, and widening the
/// row's spellings back to the bare verb reds the second.
#[test]
fn a_published_command_path_is_refused_where_the_same_verb_declared_is_not() {
    let published = concat!(
        "const CONTRACTS: &[Contract] = &[\n",
        "    row!([\"task\", \"ingest\"], \"task ingest PATH\", OperatorOrLane,\n",
        "        Command, [], [], Reply, \"board::task_ingest\"),\n",
        "];\n",
    );
    let violations = scan_source("fixture.rs", published);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("task ingest"), "{violations:?}");
    assert!(violations[0].contains("doc"), "{violations:?}");

    let declared = concat!(
        "pub mod ingest;\n",
        "pub fn ingest_transcripts() {}\n",
        "pub enum IngestError {\n    Empty,\n}\n",
    );
    assert!(scan_source("fixture.rs", declared).is_empty());

    let routed = concat!(
        "const CONTRACTS: &[Contract] = &[\n",
        "    row!([\"session\", \"briefing\"], \"session briefing\", Local,\n",
        "        None, [], [], Reply, \"spice::session\"),\n",
        "];\n",
    );
    assert!(
        scan_source("fixture.rs", routed).is_empty(),
        "a path routed under a retired word does not publish it"
    );
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

    /// One command path the port publishes, judged as the whole name it is.
    fn published(&mut self, path: &[String]) {
        for found in retired_command_matches(&path.join(" ")) {
            self.violations.insert(format!(
                "{}: command {:?} is retired; use {:?}",
                self.location, found.retired, found.successor
            ));
        }
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
            scan_published_paths(scan, &node.expr);
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

/// Command paths published by a const table of rows, such as `CONTRACTS`.
///
/// Nothing above reaches these. The table is an `Item::Const`, which is scanned
/// for its own ident and never its initializer, and each row is a macro call
/// inside that initializer, which `Item::Macro` never reaches either because it
/// reads a `macro_rules` name and not a token stream. A row's leading string
/// array is the path the parser and help are generated from, so it is the
/// spelling the port publishes and the one a porting mistake would spell.
fn scan_published_paths(scan: &mut Scan, initializer: &syn::Expr) {
    let syn::Expr::Reference(reference) = initializer else {
        return;
    };
    let syn::Expr::Array(rows) = reference.expr.as_ref() else {
        return;
    };
    for row in &rows.elems {
        let syn::Expr::Macro(invocation) = row else {
            continue;
        };
        if let Ok(path) = invocation.mac.parse_body_with(leading_path) {
            scan.published(&path);
        }
    }
}

/// The leading string array of a row, with the arms after it left unread.
///
/// Only the path is parsed because the rest is not uniformly expressions: one
/// `row!` arm spells its journal access as `Journal => Position`, which no
/// expression parser accepts. Draining the remainder is what lets this stay a
/// parse of the part that matters rather than of the whole invocation.
fn leading_path(input: syn::parse::ParseStream) -> syn::Result<Vec<String>> {
    let array: syn::ExprArray = input.parse()?;
    input.step(|cursor| {
        let mut rest = *cursor;
        while let Some((_, next)) = rest.token_tree() {
            rest = next;
        }
        Ok(((), rest))
    })?;
    Ok(array
        .elems
        .iter()
        .filter_map(|element| match element {
            syn::Expr::Lit(literal) => match &literal.lit {
                syn::Lit::Str(text) => Some(text.value()),
                _ => None,
            },
            _ => None,
        })
        .collect())
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
