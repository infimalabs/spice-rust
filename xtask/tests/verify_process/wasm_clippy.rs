use super::*;

struct WasmPrograms {
    launcher: PathBuf,
    cargo_log: PathBuf,
}

#[test]
fn verifier_wasm_clippy_rejects_a_lint_hidden_from_the_native_target() {
    let fixture = Fixture::new();
    wasm_lint_fixture(&fixture.repository);
    let programs = install_wasm_programs(&fixture);

    let output = Command::new(&programs.launcher)
        .current_dir(&fixture.repository)
        .output()
        .expect("launch verifier against the wasm lint fixture");

    assert_wasm_refusal(&programs.cargo_log, &output);
}

fn install_wasm_programs(fixture: &Fixture) -> WasmPrograms {
    let programs = fixture.root.join("programs");
    fs::create_dir(&programs).expect("create wasm lint fixture programs");
    install_real_rustfmt(&programs);
    let cargo_log = fixture.root.join("cargo-actions");
    executable(
        &programs.join("cargo"),
        &format!(
            concat!(
                "#!/bin/sh\n",
                "printf '%s\\n' \"$*\" >> '{cargo_log}'\n",
                "if [ \"$*\" = \"--version\" ]; then\n",
                "  printf '%s\\n' 'cargo 1.97.1 (c980f4866 2026-06-30)'\n",
                "  exit 0\n",
                "fi\n",
                "if [ \"$*\" = \"clippy --locked -p app --target ",
                "wasm32-unknown-unknown -- -D warnings\" ]; then\n",
                "  {path_name}=${{{path_name}#*:}}\n",
                "  export {path_name}\n",
                "  exec cargo \"$@\"\n",
                "fi\n",
                "exit 0\n",
            ),
            cargo_log = cargo_log.display(),
            path_name = PROCESS_PATH,
        ),
    );
    let launcher = programs.join("verify");
    executable(
        &launcher,
        &format!(
            concat!(
                "#!/bin/sh\n",
                "{path_name}='{programs}':${{{path_name}}}\n",
                "export {path_name}\n",
                "exec '{xtask}' verify\n",
            ),
            path_name = PROCESS_PATH,
            programs = programs.display(),
            xtask = fixture.xtask().display(),
        ),
    );
    WasmPrograms {
        launcher,
        cargo_log,
    }
}

fn assert_wasm_refusal(cargo_log: &Path, output: &std::process::Output) {
    assert_eq!(output.status.code(), Some(1), "{}", streams(output));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("variable does not need to be mutable"),
        "{}",
        streams(output),
    );
    let cargo = fs::read_to_string(cargo_log).expect("read wasm lint Cargo actions");
    let actions = cargo.lines().collect::<Vec<_>>();
    assert_eq!(
        actions
            .iter()
            .copied()
            .filter(|action| action.starts_with("clippy "))
            .collect::<Vec<_>>(),
        [
            "clippy --locked --workspace --all-targets -- -D warnings",
            "clippy --locked -p app --target wasm32-unknown-unknown -- -D warnings",
        ],
    );
    assert!(
        actions.iter().all(|action| {
            *action == "--version"
                || *action == "build --locked -p spice --bin spice"
                || action.starts_with("clippy ")
        }),
        "unexpected Cargo action while the battery ran concurrently: {actions:?}",
    );
    assert!(
        actions
            .iter()
            .filter(|action| **action == "--version")
            .count()
            <= 1,
        "the concurrent battery repeated its Cargo probe: {actions:?}",
    );
}

fn wasm_lint_fixture(repository: &Path) {
    fs::write(
        repository.join("Cargo.toml"),
        concat!(
            "[package]\n",
            "name = \"app\"\n",
            "version = \"0.0.0\"\n",
            "edition = \"2024\"\n",
        ),
    )
    .expect("write wasm lint manifest");
    fs::write(
        repository.join("Cargo.lock"),
        concat!(
            "version = 4\n\n",
            "[[package]]\n",
            "name = \"app\"\n",
            "version = \"0.0.0\"\n",
        ),
    )
    .expect("write wasm lint lock");
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask manifest has a workspace parent");
    fs::copy(
        workspace.join("rust-toolchain.toml"),
        repository.join("rust-toolchain.toml"),
    )
    .expect("copy the workspace wasm lint toolchain");
    fs::write(
        repository.join("src/lib.rs"),
        concat!(
            "#[cfg(target_arch = \"wasm32\")]\n",
            "pub fn wasm_only_lint() {\n",
            "    let mut value = 7;\n",
            "    assert_eq!(value, 7);\n",
            "}\n",
        ),
    )
    .expect("write wasm-only lint source");
    git(
        repository,
        &[
            "add",
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            "src/lib.rs",
        ],
    );
}
