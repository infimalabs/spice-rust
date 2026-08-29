#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use spice::config::{Document, Inputs, resolve};

const RETIRED_COMMANDS: &[&[&str]] = &[
    &["verify-action", "selection"],
    &["verify-action", "battery"],
    &["activate-hooks"],
    &["commit-msg", "message"],
    &["reference-transaction", "prepared"],
];

const SOURCE_DEVELOPMENT_DELEGATE: &[&str] = &[
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
];

#[test]
fn tracked_hooks_dispatch_only_to_installed_spice() {
    let hooks = workspace().join(".githooks");
    for (name, leaf) in [
        ("pre-commit", "pre-commit"),
        ("commit-msg", "commit-msg"),
        ("reference-transaction", "reference-transaction"),
    ] {
        let text = fs::read_to_string(hooks.join(name)).expect("read tracked hook");
        assert_eq!(text.matches("exec ").count(), 1, "{name}: {text}");
        assert!(
            text.contains(&format!("/.spice-install/bin/spice\" gate {leaf} \"$@\"")),
            "{name}: {text}",
        );
        for retired in ["cargo", "xtask"] {
            assert!(!text.contains(retired), "{name} retained {retired}: {text}");
        }
    }
}

#[test]
fn retired_xtask_hook_commands_are_rejected_and_absent_from_help() {
    let help = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("help")
        .output()
        .expect("run xtask help");
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).expect("help is UTF-8");
    for retired in ["verify-action", "activate-hooks", "commit-msg"] {
        assert!(!help.contains(retired), "help retained {retired}: {help}");
    }

    for arguments in RETIRED_COMMANDS {
        let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(*arguments)
            .output()
            .expect("run retired xtask spelling");
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(output.stdout.is_empty(), "{arguments:?}");
    }
}

#[test]
fn source_development_delegate_builds_one_public_spice_leaf() {
    let path = workspace().join("spice-rust.toml");
    let text = fs::read_to_string(&path).expect("read Rust policy");
    let resolved = resolve(Inputs::new(
        Some(Document::new("spice-rust.toml", &text)),
        None,
        &[],
    ))
    .expect("resolve Rust policy");
    let [command] = resolved.root().policy().pre_commit() else {
        panic!("Rust policy must carry one source-development delegate");
    };
    assert_eq!(command.label(), "Spice source gate");
    assert_eq!(command.run(), SOURCE_DEVELOPMENT_DELEGATE);
    assert!(!command.run().iter().any(|argument| argument == "xtask"));
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask crate has a workspace parent")
        .to_path_buf()
}
