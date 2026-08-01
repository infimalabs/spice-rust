//! The wrapper's import bound, measured rather than asserted.
//!
//! `run.rs` sits in front of every command an agent runs, so whatever it
//! reaches for is paid for on every invocation. Prose in its own header says it
//! names nothing but the standard library, and a header is exactly the kind of
//! rule that a later edit satisfies by accident and then quietly breaks: one
//! `use wire::...` costs nothing at review and everything at the door.
//!
//! So the bound is read out of the source. The wrapper is embedded at compile
//! time rather than found at runtime, which keeps this test at zero I/O and
//! turns a moved file into a compile error instead of a silent pass.

use std::collections::BTreeSet;

use syn::visit::Visit;
use syn::{ItemExternCrate, ItemUse, Path, UseTree};

/// The wrapper, exactly as it will be compiled.
const WRAPPER: &str = include_str!("../src/run.rs");

/// The one crate root the wrapper may name.
const STD: &str = "std";

/// Roots that name no crate at all.
const INHERENT: [&str; 2] = ["Self", "self"];

/// The wrapper reaches for the standard library and nothing else.
#[test]
fn the_wrapper_names_nothing_but_the_standard_library() {
    assert_eq!(judge("run.rs", WRAPPER), Vec::<String>::new());
}

/// A source law that cannot fail proves nothing, and every way out of std has
/// its own shape: an import, a reach back into the crate, an `extern crate`, a
/// glob that hides what it brought. Each is named on its own so a partial
/// reading of the source cannot pass for a whole one.
#[test]
fn every_way_out_of_the_standard_library_is_named() {
    const BREACH: &str = r"
use std::io::*;
use std::path::Path;
use wire::Disposition;
extern crate journal;

fn reach(path: &Path) -> Disposition {
    let _ = crate::inbox::path(path);
    let _ = fold::Inventory::new();
    Disposition::Live
}
";

    assert_eq!(
        judge("breach.rs", BREACH),
        vec![
            "breach.rs imports std::io::* through a glob".to_owned(),
            "breach.rs imports wire, which is not std".to_owned(),
            "breach.rs declares extern crate journal".to_owned(),
            "breach.rs names crate::inbox::path, rooted outside std".to_owned(),
            "breach.rs names fold::Inventory::new, rooted outside std".to_owned(),
        ]
    );
}

/// Reads one source and reports every way it leaves the standard library.
fn judge(location: &str, source: &str) -> Vec<String> {
    let syntax = syn::parse_file(source)
        .unwrap_or_else(|error| panic!("{location} must parse as Rust: {error}"));
    let mut judge = Judge {
        location,
        reachable: INHERENT
            .iter()
            .chain([&STD])
            .map(|&name| name.to_owned())
            .collect(),
        findings: Vec::new(),
    };
    judge.visit_file(&syntax);
    judge.findings
}

/// Collects what the source may name, then what it does name.
///
/// Imports are judged first because they decide the rest: once every `use`
/// roots at std, every name they introduce is a std name, and any other root a
/// path reaches for came from somewhere else.
struct Judge<'a> {
    location: &'a str,
    reachable: BTreeSet<String>,
    findings: Vec<String>,
}

impl Judge<'_> {
    /// Records a finding in the order it was read.
    fn refuse(&mut self, finding: String) {
        self.findings.push(format!("{} {finding}", self.location));
    }

    /// Walks one use tree, refusing anything rooted off std and remembering
    /// every name the rest of the source may then write bare.
    ///
    /// A glob is refused whatever it roots at: it introduces names this reading
    /// cannot enumerate, which would leave the bound resting on what the
    /// standard library happens to export.
    fn walk(&mut self, tree: &UseTree, route: &mut Vec<String>) {
        match tree {
            UseTree::Path(node) => {
                route.push(node.ident.to_string());
                if route.len() == 1 && node.ident != STD {
                    self.refuse(format!("imports {}, which is not std", node.ident));
                }
                self.walk(&node.tree, route);
                route.pop();
            }
            UseTree::Name(node) => {
                let name = node.ident.to_string();
                if route.is_empty() {
                    self.refuse(format!("imports {name}, which is not std"));
                } else if name == "self" {
                    let parent = route.last().cloned().unwrap_or_default();
                    self.reachable.insert(parent);
                } else {
                    self.reachable.insert(name);
                }
            }
            UseTree::Rename(node) => {
                if route.is_empty() {
                    self.refuse(format!("imports {}, which is not std", node.ident));
                }
                self.reachable.insert(node.rename.to_string());
            }
            UseTree::Glob(_) => {
                let written = route.join("::");
                self.refuse(format!("imports {written}::* through a glob"));
            }
            UseTree::Group(node) => {
                for item in &node.items {
                    self.walk(item, route);
                }
            }
        }
    }
}

impl<'ast> Visit<'ast> for Judge<'_> {
    fn visit_item_use(&mut self, item: &'ast ItemUse) {
        let mut route = Vec::new();
        self.walk(&item.tree, &mut route);
    }

    fn visit_item_extern_crate(&mut self, item: &'ast ItemExternCrate) {
        self.refuse(format!("declares extern crate {}", item.ident));
    }

    fn visit_path(&mut self, path: &'ast Path) {
        // A bare name is a local, a prelude item, or something an import
        // already answered for; only a qualified path can name a crate.
        if path.segments.len() > 1 {
            let root = path.segments[0].ident.to_string();
            if !self.reachable.contains(&root) {
                let written = path
                    .segments
                    .iter()
                    .map(|segment| segment.ident.to_string())
                    .collect::<Vec<_>>()
                    .join("::");
                self.refuse(format!("names {written}, rooted outside std"));
            }
        }
        syn::visit::visit_path(self, path);
    }
}
