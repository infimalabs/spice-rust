use std::fs;
use std::path::{Path, PathBuf};

use spice::config::{Document, Inputs, resolve};
use steer::WrapperReference;

#[test]
fn source_policy_suppresses_install_guidance_without_disabling_its_commit_gate() {
    let rust = resolve_document("spice-rust.toml");

    assert_eq!(
        rust.root().wrappers().spice(),
        Some(WrapperReference::Appliance)
    );
    assert!(!rust.root().policy().pre_commit_builtins());
    let [command] = rust.root().policy().pre_commit() else {
        panic!("the Rust-owned document retains its one current-source command");
    };
    assert_eq!(command.label(), "Spice source gate");
    assert_eq!(
        command.run(),
        [
            "spice",
            "dev",
            "cargo",
            "--",
            "run",
            "--quiet",
            "--locked",
            "--package",
            "spice",
            "--bin",
            "spice",
            "--",
            "gate",
            "selection",
        ]
    );
}

#[test]
fn generic_rust_policy_without_the_source_override_keeps_packaged_builtins() {
    let generic = resolve(Inputs::new(None, None, &[]))
        .expect("an absent tracked document resolves to packaged defaults");

    assert!(generic.root().policy().pre_commit_builtins());
    assert!(generic.root().policy().pre_commit().is_empty());
}

fn resolve_document(name: &str) -> spice::config::Resolved {
    let document = fs::read_to_string(workspace().join(name)).unwrap();
    resolve(Inputs::new(Some(Document::new(name, &document)), None, &[]))
        .unwrap_or_else(|error| panic!("{name} resolves through the current registry: {error}"))
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("spice crate has a workspace parent")
        .to_path_buf()
}
