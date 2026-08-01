use std::fs;
use std::path::{Path, PathBuf};

use gate::retired_matches;
use syn::visit::Visit;

#[test]
fn workspace_rust_uses_only_current_public_vocabulary() {
    let root = workspace_root();
    let mut sources = Vec::new();
    collect_rust_sources(&root, &mut sources);
    sources.sort();

    let mut violations = Vec::new();
    for path in sources {
        if path.ends_with("gate/src/names.rs") {
            continue;
        }
        let source = fs::read_to_string(&path).expect("Rust source must be readable");
        let syntax = syn::parse_file(&source)
            .unwrap_or_else(|error| panic!("{} must parse: {error}", path.display()));
        let mut scan = SourceScan::new(&root, &path);
        scan.visit_file(&syntax);
        violations.extend(scan.violations);
    }

    assert!(
        violations.is_empty(),
        "retired public vocabulary found:\n{}",
        violations.join("\n")
    );
}

#[test]
fn retired_vocabulary_is_refused_while_its_successor_is_admitted() {
    let retired = ["te", "am"].concat();
    let matches = retired_matches(&retired);

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].successor, "lineage");
    assert!(retired_matches("lineage").is_empty());
}

struct SourceScan {
    location: PathBuf,
    violations: Vec<String>,
}

impl SourceScan {
    fn new(root: &Path, path: &Path) -> Self {
        Self {
            location: path.strip_prefix(root).unwrap_or(path).to_path_buf(),
            violations: Vec::new(),
        }
    }

    fn inspect(&mut self, candidate: &str) {
        for found in retired_matches(candidate) {
            self.violations.push(format!(
                "{}: {:?} is retired; use {:?}",
                self.location.display(),
                found.retired,
                found.successor
            ));
        }
    }

    fn inspect_literal(&mut self, value: &str) {
        let public_words = value
            .split_whitespace()
            .filter(|word| !word.contains("/dev/"))
            .collect::<Vec<_>>()
            .join(" ");
        self.inspect(&public_words);
    }
}

impl<'ast> Visit<'ast> for SourceScan {
    fn visit_ident(&mut self, identifier: &'ast syn::Ident) {
        self.inspect(&identifier.to_string());
    }

    fn visit_lit_str(&mut self, literal: &'ast syn::LitStr) {
        self.inspect_literal(&literal.value());
    }

    fn visit_macro(&mut self, expression: &'ast syn::Macro) {
        self.inspect_literal(&expression.tokens.to_string());
        syn::visit::visit_macro(self, expression);
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
