use std::fs;
use std::path::{Path, PathBuf};

use spice::config::{Document, Inputs, resolve};

#[test]
fn python_and_rust_tracked_documents_keep_independent_pre_commit_policy() {
    let python = resolve_document("spice.toml");
    let rust = resolve_document("spice-rust.toml");

    assert!(!python.root().policy().pre_commit_builtins());
    let [command] = python.root().policy().pre_commit() else {
        panic!("the Python-owned document retains its one current bridge command");
    };
    assert_eq!(command.label(), "Rust gate");
    assert_eq!(
        command.run(),
        [
            "cargo",
            "run",
            "--locked",
            "--package",
            "spice",
            "--",
            "gate",
            "pre-commit",
        ]
    );

    assert!(rust.root().policy().pre_commit_builtins());
    assert!(
        rust.root().policy().pre_commit().is_empty(),
        "Rust reads only spice-rust.toml and applies its native packaged default",
    );
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
