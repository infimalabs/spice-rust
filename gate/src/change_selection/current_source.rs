//! Explicit current-source selector retained for the xtask development rim.

use std::collections::BTreeSet;
use std::path::Path;

use super::{ChangeSelectionError, ChangeSelectionReport, select_with};
use crate::GateId;
use wire::Digest;

/// Select the richer Spice-source battery from its explicit xtask override.
///
/// This is intentionally separate from [`super::select_change_gates`]: these
/// path names describe this source repository and must never become packaged
/// applicability for an installed appliance.
pub fn select_current_source_change_gates(
    source: Digest,
    changed_sources: &[String],
) -> Result<ChangeSelectionReport, ChangeSelectionError> {
    select_with(
        source,
        changed_sources,
        GateId::BATTERY.to_vec(),
        gates_for_path,
    )
}

fn gates_for_path(path: &str) -> (Vec<GateId>, Option<String>) {
    if exhaustive_authority(path) {
        return (
            Vec::new(),
            Some(format!(
                "{path:?} changes current-source gate or toolchain selection authority"
            )),
        );
    }

    let mut gates = BTreeSet::from([GateId::LocalPaths, GateId::FileShape]);
    let extension = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "rs" => {
            if !known_workspace_rust_source(path) {
                return (
                    Vec::new(),
                    Some(format!(
                        "{path:?} is Rust source outside every current-source workspace root"
                    )),
                );
            }
            gates.extend([
                GateId::Formatters,
                GateId::EnvPolicy,
                GateId::Complexity,
                GateId::MagicNumbers,
                GateId::MarkdownLinks,
            ]);
        }
        "js" | "mjs" | "cjs" => {
            gates.extend([
                GateId::JavascriptUnused,
                GateId::EnvPolicy,
                GateId::EnvNameLedger,
                GateId::Complexity,
                GateId::MagicNumbers,
            ]);
            if frontend_path(path) {
                gates.insert(GateId::CanvasWebTypecheck);
            }
        }
        "html" | "css" => {
            if frontend_path(path) {
                gates.insert(GateId::CanvasWebTypecheck);
            }
        }
        "md" | "txt" | "rst" => {
            gates.extend([GateId::RepoDocs, GateId::Taste, GateId::MarkdownLinks]);
        }
        "py" | "pyi" => {
            gates.extend([
                GateId::PythonTypecheck,
                GateId::EnvPolicy,
                GateId::EnvNameLedger,
                GateId::Complexity,
                GateId::MagicNumbers,
                GateId::Reachability,
                GateId::SymbolReachability,
                GateId::PythonUnused,
                GateId::AssertionFreeTests,
                GateId::PrivateInternals,
            ]);
        }
        "cs" => {
            gates.extend([
                GateId::CsharpUnusedCandidates,
                GateId::Complexity,
                GateId::MagicNumbers,
            ]);
        }
        "sh" | "bash" | "zsh" => {
            gates.extend([
                GateId::EnvPolicy,
                GateId::EnvNameLedger,
                GateId::Complexity,
                GateId::MagicNumbers,
            ]);
        }
        "toml" | "json" | "yaml" | "yml" | "lock" => {
            if Path::new(path).file_name().and_then(|name| name.to_str()) == Some("Cargo.toml")
                && !known_workspace_manifest(path)
            {
                return (
                    Vec::new(),
                    Some(format!(
                        "{path:?} is a package manifest outside every current-source workspace root"
                    )),
                );
            }
            gates.insert(GateId::RepoShape);
            if frontend_path(path) || matches!(path, "package.json" | "package-lock.json") {
                gates.insert(GateId::CanvasWebTypecheck);
            }
        }
        "" if known_extensionless(path) => {
            gates.insert(GateId::RepoShape);
        }
        _ if frontend_path(path) => {
            gates.insert(GateId::CanvasWebTypecheck);
        }
        _ => {
            return (
                Vec::new(),
                Some(format!(
                    "{path:?} has no current-source change-local policy class"
                )),
            );
        }
    }
    (gates.into_iter().collect(), None)
}

fn known_workspace_rust_source(path: &str) -> bool {
    let Some(root) = path.split('/').next() else {
        return false;
    };
    matches!(root, "src" | "tests" | "examples" | "benches") || known_workspace_package_root(root)
}

fn known_workspace_manifest(path: &str) -> bool {
    let mut components = path.split('/');
    let Some(root) = components.next() else {
        return false;
    };
    components.next() == Some("Cargo.toml")
        && components.next().is_none()
        && known_workspace_package_root(root)
}

fn known_workspace_package_root(root: &str) -> bool {
    matches!(
        root,
        "app"
            | "artifact"
            | "atomic"
            | "board"
            | "bus"
            | "canvas"
            | "consent"
            | "dialect"
            | "dist"
            | "fold"
            | "gate"
            | "journal"
            | "lane"
            | "listen"
            | "proof"
            | "sill"
            | "spice"
            | "steer"
            | "tape"
            | "taste"
            | "vendor"
            | "wire"
            | "xtask"
    )
}

fn exhaustive_authority(path: &str) -> bool {
    matches!(
        path,
        ".cargo/config.toml"
            | ".githooks/pre-commit"
            | "spice-rust.toml"
            | "baseline.toml"
            | "rust-toolchain.toml"
            | "Cargo.lock"
            | "Cargo.toml"
            | "gate/src/battery/manifest.toml"
            | "gate/src/change_selection.rs"
            | "gate/src/lib.rs"
            | "gate/src/manifest.rs"
            | "gate/src/test_evidence.rs"
            | "gate/src/workspace_tests.rs"
            | "spice/src/change_gate.rs"
            | "spice/src/cli.rs"
            | "spice/src/cli/contracts.rs"
            | "spice/src/cli/mounts.rs"
            | "spice/src/cli/syntax.rs"
            | "spice/src/gate.rs"
            | "spice/src/gate_command.rs"
            | "spice/src/install/git-hooks/pre-commit"
            | "spice/src/install/git_hooks.rs"
            | "spice/src/lib.rs"
            | "spice/src/main.rs"
            | "xtask/src/lib.rs"
            | "xtask/src/main.rs"
    ) || path.starts_with("gate/src/workspace_tests/")
        || path.starts_with("spice/src/gate_command/pre_commit")
}

fn frontend_path(path: &str) -> bool {
    path.starts_with("app/") || path.starts_with("canvas/") || path.starts_with("package")
}

fn known_extensionless(path: &str) -> bool {
    matches!(path, ".gitignore" | ".gitattributes" | "LICENSE")
        || path.starts_with(".githooks/")
        || path.starts_with("spice/src/install/git-hooks/")
}
