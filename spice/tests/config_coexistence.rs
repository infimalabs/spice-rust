//! Tracked configuration ownership for coexisting Python and Rust Spice.
//!
//! Python owns `spice.toml`; Rust owns `spice-rust.toml`. Equal table or key
//! names in those files are data equality, not a shared authority. Rust loads
//! only its named document, while the predecessor remains free to validate and
//! evolve its own repository layer without admitting Rust-only roots.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use spice::config::{Document, Inputs, descriptors, locations, resolve};
use wire::ConfigSource;

/// Root tables `spice/config/schema.py` declares, as `CONFIG_SCHEMA.children`.
///
/// The list is transcribed because the predecessor checkout is not a test
/// dependency of this repository. It protects the Python-owned file without
/// making Rust's independent schema answer to it.
const PREDECESSOR_ROOTS: &[&str] = &[
    "agent", "commands", "judge", "locks", "maxim", "maxims", "policy", "rtk", "say", "serve",
    "tasks", "taste", "wrappers",
];

/// Rust tracked roots the predecessor does not declare.
const RUST_ONLY_ROOTS: &[&str] = &[
    "board", "canvas", "consent", "gate", "lane", "listen", "lock", "proof", "steer", "vendor",
];

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("spice crate has a workspace parent")
        .to_path_buf()
}

fn python_configuration() -> PathBuf {
    workspace().join("spice.toml")
}

fn rust_configuration() -> PathBuf {
    workspace().join("spice-rust.toml")
}

fn roots_the_predecessor_refuses(document: &str) -> Vec<String> {
    let parsed: toml::Table = document
        .parse()
        .expect("tracked configuration parses as TOML");
    parsed
        .keys()
        .filter(|root| !PREDECESSOR_ROOTS.contains(&root.as_str()))
        .cloned()
        .collect()
}

struct RepositoryFixture(PathBuf);

impl RepositoryFixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-config-coexistence-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(root.join(".git")).expect("create fixture repository");
        let root = fs::canonicalize(root).expect("canonicalize fixture repository");
        for name in ["spice.toml", "spice-rust.toml"] {
            fs::copy(workspace().join(name), root.join(name))
                .unwrap_or_else(|error| panic!("copy fixture {name}: {error}"));
        }
        Self(root)
    }
}

impl Drop for RepositoryFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove coexistence fixture");
    }
}

#[test]
fn rust_and_python_tracked_configuration_paths_are_distinct() {
    let fixture = RepositoryFixture::new();
    let root = &fixture.0;
    let repository = spice::repository::Repository::discover(root).expect("fixture discovers");
    let rust = locations(&repository);
    let python = root.join("spice.toml");

    assert_eq!(rust.tracked(), root.join("spice-rust.toml"));
    assert_ne!(rust.tracked(), python);
    assert!(rust.tracked().is_file(), "Rust tracked policy is present");
    assert!(python.is_file(), "Python tracked policy is present");
}

#[test]
fn python_configuration_authors_no_root_its_schema_refuses() {
    let document = fs::read_to_string(python_configuration()).expect("read Python configuration");

    assert_eq!(
        roots_the_predecessor_refuses(&document),
        Vec::<String>::new(),
        "Python's own tracked document must satisfy the predecessor schema",
    );
}

#[test]
fn a_rust_only_root_is_admitted_without_entering_python_configuration() {
    let rust = fs::read_to_string(rust_configuration()).expect("read Rust configuration");
    let candidate = format!("{rust}\n[gate]\n");
    resolve(Inputs::new(
        Some(Document::new("spice-rust.toml", &candidate)),
        None,
        &[],
    ))
    .expect("Rust-only root is admitted in the Rust-owned document");

    let python = fs::read_to_string(python_configuration()).expect("read Python configuration");
    let planted = format!("{python}\n[gate]\n");
    assert_eq!(roots_the_predecessor_refuses(&planted), ["gate"]);
    assert_eq!(
        roots_the_predecessor_refuses(&python),
        Vec::<String>::new(),
        "the Python-owned bytes remain unchanged and valid",
    );
}

#[test]
fn the_independent_rust_schema_keeps_all_rust_only_roots() {
    let admitted: BTreeSet<&str> = descriptors()
        .iter()
        .filter(|descriptor| descriptor.layers().contains(&ConfigSource::Tracked))
        .filter_map(|descriptor| descriptor.template().as_str().split('.').next())
        .collect();
    let rust_only: Vec<&str> = admitted
        .iter()
        .copied()
        .filter(|root| !PREDECESSOR_ROOTS.contains(root))
        .collect();

    assert_eq!(rust_only, RUST_ONLY_ROOTS);
}
