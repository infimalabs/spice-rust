use super::*;

struct BrowserRefusalPrograms {
    directory: PathBuf,
    cargo_log: PathBuf,
}

#[test]
fn zero_exit_browser_process_without_a_receipt_refuses_and_never_lands() {
    let fixture = Fixture::new();
    let programs = install_browser_refusal_programs(&fixture);
    let output = Command::new(fixture.xtask())
        .arg("verify")
        .current_dir(&fixture.repository)
        .env(PROCESS_PATH, &programs.directory)
        .output()
        .expect("launch the real verifier process");

    assert_eq!(output.status.code(), Some(1), "{}", streams(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("browser coverage did not complete"),
        "{}",
        streams(&output)
    );
    assert_browser_refusal_cargo_trace(&fixture, &programs.cargo_log);
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("ordinal=13/13"),
        "{}",
        streams(&output)
    );
}

fn install_browser_refusal_programs(fixture: &Fixture) -> BrowserRefusalPrograms {
    let directory = fixture.root.join("programs");
    fs::create_dir(&directory).expect("create process fixture directory");
    install_real_git(&directory);
    install_real_rustfmt(&directory);
    let cargo_log = fixture.root.join("cargo-actions");
    executable(
        &directory.join("cargo"),
        &format!(
            concat!(
                "#!/bin/sh\n",
                "printf '%s\\n' \"$*\" >> '{}'\n",
                "if [ \"$*\" = \"--version\" ]; then\n",
                "  printf '%s\\n' 'cargo 1.97.1 (c980f4866 2026-06-30)'\n",
                "fi\n",
                "if [ \"$1\" = metadata ]; then\n",
                "  printf '%s\\n' '{}'\n",
                "fi\n",
                "exit 0\n",
            ),
            cargo_log.display(),
            browser_refusal_metadata(),
        ),
    );
    executable(
        &directory.join("trunk"),
        concat!(
            "#!/bin/sh\n",
            "if [ \"$*\" = \"--version\" ]; then\n",
            "  printf '%s\\n' 'trunk 0.21.14'\n",
            "fi\n",
            "exit 0\n",
        ),
    );
    executable(&directory.join("npm"), "#!/bin/sh\nexit 0\n");
    BrowserRefusalPrograms {
        directory,
        cargo_log,
    }
}

fn browser_refusal_metadata() -> String {
    serde_json::json!({
        "packages": [{
            "id": "verify-process-fixture-id",
            "name": "verify-process-fixture",
            "manifest_path": "Cargo.toml",
            "targets": [{
                "name": "verify_process_fixture",
                "kind": ["lib"],
                "src_path": "src/lib.rs",
                "test": true,
            }],
        }],
        "workspace_members": ["verify-process-fixture-id"],
    })
    .to_string()
}

fn assert_browser_refusal_cargo_trace(fixture: &Fixture, cargo_log: &Path) {
    let cargo_actions = fs::read_to_string(cargo_log).expect("read verifier Cargo actions");
    let chain = cargo_actions
        .lines()
        .filter(|action| *action != "--version")
        .collect::<Vec<_>>();
    let app_action = format!(
        "xtask app --spice-verifier-publish-root {}",
        fixture
            .repository
            .canonicalize()
            .expect("canonical verifier repository")
            .display()
    );
    assert_eq!(
        chain.get(..5),
        Some(
            &[
                "build --locked -p spice --bin spice",
                "clippy --locked --workspace --all-targets -- -D warnings",
                "clippy --locked -p app --target wasm32-unknown-unknown -- -D warnings",
                "check --locked --workspace --target wasm32-unknown-unknown",
                app_action.as_str(),
            ][..]
        )
    );
    let allowed = [
        "clippy --locked --workspace --all-targets -- -D warnings",
        "clippy --locked -p app --target wasm32-unknown-unknown -- -D warnings",
        "check --locked --workspace --target wasm32-unknown-unknown",
        app_action.as_str(),
        "build --locked -p spice --bin spice",
        "metadata --locked --format-version 1 --no-deps",
        "test --locked --workspace --all-targets --no-run",
    ];
    assert!(
        chain.iter().all(|action| allowed.contains(action)),
        "unexpected Cargo action under browser refusal: {cargo_actions:?}",
    );
    let mut unique_chain = chain.clone();
    unique_chain.sort_unstable();
    unique_chain.dedup();
    assert_eq!(
        unique_chain.len(),
        chain.len(),
        "duplicate Cargo action: {chain:?}"
    );
}
