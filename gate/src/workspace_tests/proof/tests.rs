use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

mod file_macro_authority;
mod output_macro_authority;

#[test]
fn equivalent_roots_plan_identical_path_independent_shards() {
    let first = Fixture::new("first");
    let second = Fixture::new("second");
    let first_plan = first.plan(true);
    let second_plan = second.plan(true);

    assert_eq!(first_plan, second_plan);
    assert!(!first_plan.typechecks().is_empty());
    assert!(!first_plan.tests().is_empty());
    for shard in first_plan.typechecks().iter().chain(first_plan.tests()) {
        assert!(shard.action_id().starts_with("cargo:"));
        assert_eq!(shard.argv()[2], "--package");
        assert_eq!(shard.argv()[3], shard.package());
        assert!(shard.input_paths().contains(&"Cargo.toml".to_owned()));
        assert!(shard.input_paths().contains(&"Cargo.lock".to_owned()));
        assert_eq!(
            shard
                .input_paths()
                .contains(&"proof-authority.txt".to_owned()),
            shard.package() == "fixture",
        );
        assert_eq!(
            shard.compile_time_authorities().len(),
            usize::from(shard.package() == "fixture") * 2,
        );
        assert_eq!(
            shard.compile_time_producers().len(),
            usize::from(shard.package() == "fixture") * 2,
        );
        assert!(
            shard
                .compile_time_producers()
                .iter()
                .all(|producer| !producer.external)
        );
        if let Some(authority) = shard.compile_time_authorities().first() {
            assert_eq!(authority.package, "fixture");
            assert_eq!(authority.kind, "custom-build");
            assert_eq!(authority.input_paths, ["proof-authority.txt"]);
            assert_eq!(authority.environment, ["FIXTURE_BUILD_INPUT"]);
            assert!(authority.generated_outputs.is_empty());
            let proc_macro = &shard.compile_time_authorities()[1];
            assert_eq!(proc_macro.package, "fixture-macro");
            assert_eq!(proc_macro.kind, "proc-macro");
            assert!(proc_macro.input_paths.is_empty());
            assert!(proc_macro.environment.is_empty());
            assert!(proc_macro.generated_outputs.is_empty());
        }
        let own_source = format!("{}/src/lib.rs", shard.package());
        let peer = if shard.package() == "fixture" {
            "peer/src/lib.rs"
        } else {
            "fixture/src/lib.rs"
        };
        assert!(shard.input_paths().contains(&own_source));
        assert!(!shard.input_paths().contains(&peer.to_owned()));
        assert!(!shard.input_paths().contains(&"notes.txt".to_owned()));
        assert!(
            !shard
                .action_id()
                .contains(first.root.to_string_lossy().as_ref())
        );
        assert!(
            shard
                .argv()
                .iter()
                .all(|argument| !argument.contains(first.root.to_string_lossy().as_ref()))
        );
    }
}

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "the repository graph regression uses the active test environment"
)]
fn repository_journal_graph_reaches_external_libsqlite3_sys_build_script() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("gate crate has a workspace parent");
    let environment =
        Environment::new(std::env::vars_os()).expect("ambient test environment is valid");
    let inventory = acquire_resolved_inventory(root, &environment, OsStr::new("cargo"))
        .expect("acquire repository Cargo graph");
    let journal = inventory
        .packages
        .iter()
        .find(|package| package.name == "journal")
        .expect("repository contains journal");
    let producers = resolved_compile_time_producers(&inventory, &journal.id)
        .expect("classify journal compile-time producers");
    let sqlite = producers
        .iter()
        .find(|producer| {
            producer.package == "libsqlite3-sys"
                && producer.kind == "custom-build"
                && producer.target == "build-script-build"
        })
        .expect("journal reaches libsqlite3-sys build script through rusqlite");
    assert!(sqlite.external);
    assert!(!sqlite.selected_tree_authority);
    assert!(sqlite.package_id.starts_with("registry+"));
    let serde_derive = producers
        .iter()
        .find(|producer| producer.package == "serde_derive" && producer.kind == "proc-macro")
        .expect("journal reaches the external serde_derive proc macro");
    assert!(serde_derive.external);
    assert!(!serde_derive.selected_tree_authority);
    assert!(serde_derive.package_id.starts_with("registry+"));
}

#[test]
fn external_path_build_script_makes_only_the_dependent_shard_non_cacheable() {
    let fixture = Fixture::new("external-path-producer");
    let external = fixture.root.with_extension("external-build");
    fs::create_dir_all(external.join("src")).expect("create external package source");
    fs::write(
        external.join("Cargo.toml"),
        concat!(
            "[package]\n",
            "name = \"external-build\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "build = \"build.rs\"\n",
        ),
    )
    .expect("write external package manifest");
    fs::write(external.join("build.rs"), "fn main() {}\n").expect("write external build script");
    fs::write(external.join("src/lib.rs"), "pub const EXTERNAL: u8 = 1;\n")
        .expect("write external package source");
    fs::write(
        fixture.root.join("fixture/Cargo.toml"),
        format!(
            concat!(
                "[package]\n",
                "name = \"fixture\"\n",
                "version = \"0.1.0\"\n",
                "edition = \"2024\"\n",
                "build = \"build.rs\"\n\n",
                "[package.metadata.spice.cargo-proof]\n",
                "version = 1\n",
                "complete = true\n\n",
                "[dependencies]\n",
                "fixture-macro = {{ path = \"../fixture-macro\" }}\n",
                "external-build = {{ path = {:?} }}\n",
            ),
            external,
        ),
    )
    .expect("add external build dependency");
    fs::write(
        fixture.root.join("Cargo.lock"),
        concat!(
            "# This file is automatically @generated by Cargo.\n",
            "version = 4\n\n",
            "[[package]]\n",
            "name = \"external-build\"\n",
            "version = \"0.1.0\"\n\n",
            "[[package]]\n",
            "name = \"fixture\"\n",
            "version = \"0.1.0\"\n",
            "dependencies = [\"external-build\", \"fixture-macro\"]\n\n",
            "[[package]]\n",
            "name = \"fixture-macro\"\n",
            "version = \"0.1.0\"\n\n",
            "[[package]]\n",
            "name = \"peer\"\n",
            "version = \"0.1.0\"\n",
        ),
    )
    .expect("add external build dependency to lockfile");

    let plan = fixture.plan(true);
    let shard = plan
        .typechecks()
        .iter()
        .find(|shard| shard.package() == "fixture")
        .expect("fixture typecheck shard is planned");
    let external_producer = shard
        .compile_time_producers()
        .iter()
        .find(|producer| producer.package == "external-build")
        .expect("external build script is classified");
    assert!(external_producer.external);
    assert!(!external_producer.selected_tree_authority);
    assert!(
        shard
            .non_cacheable_reason()
            .is_some_and(|reason| reason.contains("external external-build"))
    );
    assert!(
        plan.typechecks()
            .iter()
            .find(|shard| shard.package() == "peer")
            .expect("peer typecheck shard is planned")
            .non_cacheable_reason()
            .is_none()
    );
    fs::remove_dir_all(external).expect("remove external package");
}

#[test]
fn source_backed_external_producer_requires_and_binds_one_generation_authority() {
    let inventory = Inventory {
        packages: Vec::new(),
        resolution: CargoResolution::default(),
        binary_outputs: Vec::new(),
        target_directory: PathBuf::new(),
        build_directory: PathBuf::new(),
    };
    let producer = CargoCompileTimeProducer {
        package_id: "registry+https://example.invalid/index#fixture@1.0.0".to_owned(),
        package: "fixture".to_owned(),
        target: "build-script-build".to_owned(),
        kind: "custom-build".to_owned(),
        external: true,
        selected_tree_authority: false,
        source_generation_eligible: true,
    };
    let roots = BTreeSet::new();
    let selected_paths = Vec::new();
    let mut paths = BTreeSet::new();
    let external_paths = BTreeSet::new();

    let (_, missing) = compile_time_reuse(
        &inventory,
        Path::new("/selected"),
        &roots,
        &selected_paths,
        &mut paths,
        std::slice::from_ref(&producer),
        CompileTimeExternal {
            local_dependencies: &external_paths,
            source: None,
        },
    );
    assert!(matches!(missing, CargoShardReuse::NonCacheable { .. }));

    let source = CargoExternalSourceAuthority::new("a".repeat(64), "b".repeat(64)).unwrap();
    let (_, admitted) = compile_time_reuse(
        &inventory,
        Path::new("/selected"),
        &roots,
        &selected_paths,
        &mut paths,
        &[producer],
        CompileTimeExternal {
            local_dependencies: &external_paths,
            source: Some(&source),
        },
    );
    assert_eq!(admitted, CargoShardReuse::Reusable);

    let mutable = CargoCompileTimeProducer {
        package_id: "external-path#fixture@1.0.0".to_owned(),
        source_generation_eligible: false,
        ..producer_for_test()
    };
    let (_, refused) = compile_time_reuse(
        &inventory,
        Path::new("/selected"),
        &roots,
        &selected_paths,
        &mut paths,
        &[mutable],
        CompileTimeExternal {
            local_dependencies: &external_paths,
            source: Some(&source),
        },
    );
    assert!(matches!(refused, CargoShardReuse::NonCacheable { .. }));

    let shard = CargoGateShard {
        kind: CargoGateShardKind::Typecheck,
        action_id: "cargo:typecheck:fixture::lib:fixture".to_owned(),
        package: "fixture".to_owned(),
        targets: vec!["lib:fixture".to_owned()],
        argv: vec!["check".to_owned()],
        input_paths: vec!["Cargo.lock".to_owned()],
        compile_time_producers: vec![producer_for_test()],
        compile_time_authorities: Vec::new(),
        external_source: Some(source),
        reuse: CargoShardReuse::Reusable,
    };
    let mut moved = shard.clone();
    moved.external_source =
        Some(CargoExternalSourceAuthority::new("c".repeat(64), "b".repeat(64)).unwrap());
    assert_ne!(wire::encode(&shard).unwrap(), wire::encode(&moved).unwrap());
}

fn producer_for_test() -> CargoCompileTimeProducer {
    CargoCompileTimeProducer {
        package_id: "registry+https://example.invalid/index#fixture@1.0.0".to_owned(),
        package: "fixture".to_owned(),
        target: "build-script-build".to_owned(),
        kind: "custom-build".to_owned(),
        external: true,
        selected_tree_authority: false,
        source_generation_eligible: true,
    }
}

#[test]
fn shard_execution_returns_success_only_for_a_passing_command() {
    let fixture = Fixture::new("execute");
    let plan = fixture.plan(false);
    let shard = plan
        .typechecks()
        .first()
        .expect("fixture selects a typecheck shard");
    let mut output = Vec::new();
    let result = run_cargo_gate_shard_at(
        &fixture.root,
        &fixture.outputs,
        fixture.environment(),
        OsStr::new("cargo"),
        shard,
        &mut output,
    )
    .unwrap_or_else(|error| {
        panic!(
            "valid source passes its planned shard: {error}\n{}",
            String::from_utf8_lossy(&output),
        )
    });
    assert_eq!(result.action_id, shard.action_id());
    assert_eq!(result.completion, "passed");

    fs::write(fixture.root.join("fixture/src/lib.rs"), "pub fn broken(\n")
        .expect("write invalid Rust source");
    let error = run_cargo_gate_shard_at(
        &fixture.root,
        &fixture.outputs,
        fixture.environment(),
        OsStr::new("cargo"),
        shard,
        &mut Vec::new(),
    )
    .expect_err("failed Cargo execution cannot produce a successful result");
    assert!(error.to_string().contains("ended as exit code="), "{error}");
}

#[test]
fn incomplete_file_macro_authority_refuses_before_a_shard_can_be_planned() {
    let fixture = Fixture::new("incomplete-file-macro");
    fs::write(
        fixture.root.join("fixture/src/lib.rs"),
        "pub const AUTHORITY: &str = include_str!(\"../../missing.txt\");\n",
    )
    .expect("write incomplete file-macro source");

    let error = fixture
        .try_plan(true)
        .expect_err("incomplete file-macro closure cannot produce Cargo shards");
    assert!(
        error
            .to_string()
            .contains("is not readable selected-tree authority"),
        "unexpected refusal: {error}",
    );
}

#[test]
fn executable_compile_time_code_requires_a_closed_declaration() {
    let fixture = Fixture::new("missing-build-authority");
    fs::write(
        fixture.root.join("fixture/Cargo.toml"),
        concat!(
            "[package]\n",
            "name = \"fixture\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "build = \"build.rs\"\n\n",
            "[dependencies]\n",
            "fixture-macro = { path = \"../fixture-macro\" }\n",
        ),
    )
    .expect("remove compile-time authority declaration");

    let reason = fixture.fixture_non_cacheable_reason();
    assert!(
        reason.contains("Cargo shard is non-cacheable")
            && reason.contains("no complete package.metadata.spice.cargo-proof"),
        "unexpected refusal: {reason}",
    );
}

#[test]
fn out_dir_input_without_a_build_script_refuses_before_planning() {
    let fixture = Fixture::new("missing-out-dir-producer");
    fs::write(
        fixture.root.join("fixture/Cargo.toml"),
        concat!(
            "[package]\n",
            "name = \"fixture\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n\n",
            "[dependencies]\n",
            "fixture-macro = { path = \"../fixture-macro\" }\n",
        ),
    )
    .expect("remove OUT_DIR producer");
    fs::remove_file(fixture.root.join("fixture/build.rs"))
        .expect("remove auto-discovered build script");
    fs::write(
        fixture.root.join("fixture/src/lib.rs"),
        "std::include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\n",
    )
    .expect("write OUT_DIR consumer");

    let reason = fixture.fixture_non_cacheable_reason();
    assert!(
        reason.contains("consumes OUT_DIR generated inputs without a custom-build producer"),
        "unexpected refusal: {reason}",
    );
}

#[test]
fn dynamic_rerun_directive_refuses_before_a_shard_can_be_planned() {
    let fixture = Fixture::new("dynamic-rerun");
    fs::write(
        fixture.root.join("fixture/build.rs"),
        "fn main() { let path = \"../proof-authority.txt\"; println!(\"cargo::rerun-if-changed={path}\"); }\n",
    )
    .expect("write dynamic build-script directive");

    let reason = fixture.fixture_non_cacheable_reason();
    assert!(
        reason.contains("dynamic or multiline compile-time output"),
        "unexpected refusal: {reason}",
    );
}

#[test]
fn positional_interpolation_cannot_hide_a_cargo_directive() {
    let fixture = Fixture::new("positional-output");
    fs::write(
        fixture.root.join("fixture/build.rs"),
        "fn main() { println!(\"{}\", \"cargo::rerun-if-changed=../proof-authority.txt\"); }\n",
    )
    .expect("write positionally interpolated build-script output");

    let reason = fixture.fixture_non_cacheable_reason();
    assert!(
        reason.contains("computed or interpolated compile-time output"),
        "unexpected refusal: {reason}",
    );
}

#[test]
fn malformed_rerun_directive_refuses_before_a_shard_can_be_planned() {
    let fixture = Fixture::new("malformed-rerun");
    fs::write(
        fixture.root.join("fixture/build.rs"),
        "fn main() { println!(\"cargo::rerun-if-changed\"); }\n",
    )
    .expect("write malformed build-script directive");

    let reason = fixture.fixture_non_cacheable_reason();
    assert!(
        reason.contains("malformed Cargo directive"),
        "unexpected refusal: {reason}",
    );
}

#[test]
fn linker_directive_is_non_cacheable_without_external_input_authority() {
    let fixture = Fixture::new("linker-directive");
    fs::write(
        fixture.root.join("fixture/build.rs"),
        "fn main() { println!(\"cargo::rustc-link-search=native=../native\"); }\n",
    )
    .expect("write native linker build-script directive");

    let reason = fixture.fixture_non_cacheable_reason();
    assert!(
        reason.contains("unsupported Cargo directive \"rustc-link-search\""),
        "unexpected refusal: {reason}",
    );
}

#[test]
fn undeclared_ambient_environment_refuses_before_planning() {
    let fixture = Fixture::new("ambient-environment");
    fs::write(
        fixture.root.join("fixture/build.rs"),
        "fn main() { let _ = std::env::var(\"AMBIENT_BUILD_INPUT\"); }\n",
    )
    .expect("write ambient build-script read");

    let reason = fixture.fixture_non_cacheable_reason();
    assert!(
        reason.contains("reads undeclared ambient environment \"AMBIENT_BUILD_INPUT\""),
        "unexpected refusal: {reason}",
    );
}

#[test]
fn undeclared_filesystem_read_refuses_before_planning() {
    let fixture = Fixture::new("ambient-file");
    fs::write(
        fixture.root.join("fixture/build.rs"),
        "fn main() { let _ = std::fs::read_to_string(\"../notes.txt\"); }\n",
    )
    .expect("write undeclared build-script file read");

    let reason = fixture.fixture_non_cacheable_reason();
    assert!(
        reason.contains("reads undeclared compile-time file \"../notes.txt\""),
        "unexpected refusal: {reason}",
    );
}

#[test]
fn process_dependency_refuses_before_planning() {
    let fixture = Fixture::new("process-dependency");
    fs::write(
        fixture.root.join("fixture/build.rs"),
        "fn main() { let _ = std::process::Command::new(\"git\").status(); }\n",
    )
    .expect("write process-dependent build script");

    let reason = fixture.fixture_non_cacheable_reason();
    assert!(
        reason.contains("unbounded method dispatch .status()"),
        "unexpected refusal: {reason}",
    );
}

#[test]
fn renamed_process_constructor_is_non_cacheable() {
    let fixture = Fixture::new("renamed-process-constructor");
    fs::write(
        fixture.root.join("fixture/build.rs"),
        "use std::process::Command as C;\nfn main() { let _ = C::new(\"git\"); }\n",
    )
    .expect("write renamed process constructor");

    let reason = fixture.fixture_non_cacheable_reason();
    assert!(
        reason.contains("unrecognized executable call C::new"),
        "unexpected refusal: {reason}",
    );
}

#[test]
fn helper_wrapper_call_is_non_cacheable() {
    let fixture = Fixture::new("helper-wrapper");
    fs::write(
        fixture.root.join("fixture/build.rs"),
        concat!(
            "fn main() { helper(); }\n",
            "fn helper() { let _ = std::process::Command::new(\"git\"); }\n",
        ),
    )
    .expect("write process helper wrapper");

    let reason = fixture.fixture_non_cacheable_reason();
    assert!(
        reason.contains("unrecognized executable call helper"),
        "unexpected refusal: {reason}",
    );
}

#[test]
fn declared_helper_still_refuses_a_dangerous_scanned_body() {
    let fixture = Fixture::new("declared-dangerous-helper");
    let manifest = fixture.root.join("fixture/Cargo.toml");
    let current = fs::read_to_string(&manifest).expect("read fixture manifest");
    fs::write(
        &manifest,
        current.replace(
            "complete = true\n",
            "complete = true\ncalls = [\"helper\"]\n",
        ),
    )
    .expect("declare exact helper call");
    fs::write(
        fixture.root.join("fixture/build.rs"),
        concat!(
            "fn main() { helper(); }\n",
            "fn helper() { let _ = std::process::Command::new(\"git\"); }\n",
        ),
    )
    .expect("write declared process helper");

    let reason = fixture.fixture_non_cacheable_reason();
    assert!(
        reason.contains("unbounded process or network dependency std::process::Command::new"),
        "unexpected refusal: {reason}",
    );
}

#[test]
fn method_dispatch_is_non_cacheable() {
    let fixture = Fixture::new("method-dispatch");
    fs::write(
        fixture.root.join("fixture/build.rs"),
        "fn main() { let _ = std::env::var(\"HOME\").unwrap(); }\n",
    )
    .expect("write method-based environment access");

    let reason = fixture.fixture_non_cacheable_reason();
    assert!(
        reason.contains("unbounded method dispatch .unwrap()"),
        "unexpected refusal: {reason}",
    );
}

#[test]
fn computed_compile_time_output_is_non_cacheable() {
    let fixture = Fixture::new("computed-output");
    fs::write(
        fixture.root.join("fixture/build.rs"),
        "fn main() { let message = \"cargo::rerun-if-changed=../proof-authority.txt\"; println!(message); }\n",
    )
    .expect("write computed build-script output");

    let reason = fixture.fixture_non_cacheable_reason();
    assert!(
        reason.contains("computed or interpolated compile-time output"),
        "unexpected refusal: {reason}",
    );
}

#[test]
fn filesystem_output_is_non_cacheable() {
    let fixture = Fixture::new("filesystem-output");
    fs::write(
        fixture.root.join("fixture/build.rs"),
        "fn main() { let _ = std::fs::write(\"generated.rs\", \"generated\"); }\n",
    )
    .expect("write filesystem output");

    let reason = fixture.fixture_non_cacheable_reason();
    assert!(
        reason.contains("unbounded compile-time filesystem output"),
        "unexpected refusal: {reason}",
    );
}

#[test]
fn escaping_rerun_path_refuses_before_planning() {
    let fixture = Fixture::new("escaping-rerun");
    let external = fixture.root.with_extension("external-authority");
    fs::write(&external, "outside selected tree\n").expect("write external authority");
    fs::write(
        fixture.root.join("fixture/build.rs"),
        format!(
            "fn main() {{ println!(\"cargo::rerun-if-changed=../../{}\"); }}\n",
            external.file_name().unwrap().to_string_lossy(),
        ),
    )
    .expect("write escaping build-script directive");

    let reason = fixture.fixture_non_cacheable_reason();
    assert!(
        reason.contains("escapes selected root"),
        "unexpected refusal: {reason}",
    );
    fs::remove_file(external).expect("remove external authority");
}

struct Fixture {
    root: PathBuf,
    outputs: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gate-cargo-proof-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        let outputs = root.with_extension("outputs");
        fs::create_dir_all(root.join("fixture/src")).expect("create fixture source");
        fs::create_dir_all(root.join("fixture/tests")).expect("create fixture tests");
        fs::create_dir_all(root.join("fixture-macro/src")).expect("create macro source");
        fs::create_dir_all(root.join("peer/src")).expect("create peer source");
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nresolver = \"2\"\nmembers = [\"fixture\", \"fixture-macro\", \"peer\"]\n",
        )
        .expect("write workspace manifest");
        fs::write(
            root.join("Cargo.lock"),
            concat!(
                "# This file is automatically @generated by Cargo.\n",
                "version = 4\n\n",
                "[[package]]\n",
                "name = \"fixture\"\n",
                "version = \"0.1.0\"\n",
                "dependencies = [\"fixture-macro\"]\n\n",
                "[[package]]\n",
                "name = \"fixture-macro\"\n",
                "version = \"0.1.0\"\n\n",
                "[[package]]\n",
                "name = \"peer\"\n",
                "version = \"0.1.0\"\n",
            ),
        )
        .expect("write lockfile");
        fs::write(
            root.join("fixture/Cargo.toml"),
            concat!(
                "[package]\n",
                "name = \"fixture\"\n",
                "version = \"0.1.0\"\n",
                "edition = \"2024\"\n",
                "build = \"build.rs\"\n\n",
                "[package.metadata.spice.cargo-proof]\n",
                "version = 1\n",
                "complete = true\n",
                "\n[dependencies]\n",
                "fixture-macro = { path = \"../fixture-macro\" }\n",
            ),
        )
        .expect("write package manifest");
        fs::write(
            root.join("fixture/build.rs"),
            concat!(
                "fn main() {\n",
                "    println!(\"cargo::rerun-if-changed=../proof-authority.txt\");\n",
                "    println!(\"cargo::rerun-if-env-changed=FIXTURE_BUILD_INPUT\");\n",
                "}\n",
            ),
        )
        .expect("write package build script");
        fs::write(
            root.join("fixture/src/lib.rs"),
            concat!(
                "pub const VALUE: u8 = 1;\n",
                "pub const AUTHORITY: &str = ::std::r#include_str!(\"../../proof-authority.txt\");\n",
                "#[fixture_macro::passthrough]\n",
                "pub fn expanded() {}\n",
            ),
        )
        .expect("write library source");
        fs::write(
            root.join("fixture-macro/Cargo.toml"),
            concat!(
                "[package]\n",
                "name = \"fixture-macro\"\n",
                "version = \"0.1.0\"\n",
                "edition = \"2024\"\n\n",
                "[lib]\n",
                "proc-macro = true\n\n",
                "[package.metadata.spice.cargo-proof]\n",
                "version = 1\n",
                "complete = true\n",
            ),
        )
        .expect("write proc-macro package manifest");
        fs::write(
            root.join("fixture-macro/src/lib.rs"),
            concat!(
                "extern crate proc_macro;\n",
                "use proc_macro::TokenStream;\n",
                "#[proc_macro_attribute]\n",
                "pub fn passthrough(_: TokenStream, item: TokenStream) -> TokenStream { item }\n",
            ),
        )
        .expect("write proc-macro source");
        fs::write(
            root.join("fixture/tests/integration.rs"),
            "#[test]\nfn value_is_one() { assert_eq!(fixture::VALUE, 1); }\n",
        )
        .expect("write integration source");
        fs::write(
            root.join("peer/Cargo.toml"),
            "[package]\nname = \"peer\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write peer manifest");
        fs::write(root.join("peer/src/lib.rs"), "pub const PEER: u8 = 2;\n")
            .expect("write peer source");
        fs::write(root.join("notes.txt"), "not a Cargo input\n").expect("write unrelated source");
        fs::write(root.join("proof-authority.txt"), "root Cargo input\n")
            .expect("write root Cargo authority");
        fs::write(root.join("file-macro-authority.txt"), "file authority\n")
            .expect("write file-macro authority");
        Self { root, outputs }
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "the process-execution fixture captures its own test environment"
    )]
    fn environment(&self) -> Environment {
        Environment::new(std::env::vars_os()).expect("ambient test environment is valid")
    }

    fn plan(&self, include_tests: bool) -> CargoGateShardPlan {
        self.try_plan(include_tests)
            .expect("plan fixture Cargo shards")
    }

    fn try_plan(&self, include_tests: bool) -> Result<CargoGateShardPlan, WorkspaceTestError> {
        let changed_sources = [
            "fixture/src/lib.rs".to_owned(),
            "peer/src/lib.rs".to_owned(),
        ];
        let selected_paths = [
            "Cargo.lock".to_owned(),
            "Cargo.toml".to_owned(),
            "fixture/Cargo.toml".to_owned(),
            "fixture/build.rs".to_owned(),
            "fixture/src/lib.rs".to_owned(),
            "fixture/tests/integration.rs".to_owned(),
            "file-macro-authority.txt".to_owned(),
            "fixture-macro/Cargo.toml".to_owned(),
            "fixture-macro/src/lib.rs".to_owned(),
            "notes.txt".to_owned(),
            "peer/Cargo.toml".to_owned(),
            "peer/src/lib.rs".to_owned(),
            "proof-authority.txt".to_owned(),
        ];
        plan_cargo_gate_shards_at(CargoGateShardPlanning {
            root: &self.root,
            base: None,
            source: wire::Digest::compute(wire::Domain::GateTree, b"fixture"),
            changed_sources: &changed_sources,
            selected_paths: &selected_paths,
            environment: self.environment(),
            cargo: OsStr::new("cargo"),
            external_source: None,
            include_tests,
        })
    }

    fn fixture_non_cacheable_reason(&self) -> String {
        let plan = self.plan(true);
        let fixture = plan
            .typechecks()
            .iter()
            .find(|shard| shard.package() == "fixture")
            .expect("fixture typecheck shard is planned");
        let peer = plan
            .typechecks()
            .iter()
            .find(|shard| shard.package() == "peer")
            .expect("peer typecheck shard is planned");
        assert!(
            peer.non_cacheable_reason().is_none(),
            "unrelated peer must remain reusable"
        );
        fixture
            .non_cacheable_reason()
            .expect("fixture shard must be non-cacheable")
            .to_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove Cargo proof fixture");
        if self.outputs.exists() {
            fs::remove_dir_all(&self.outputs).expect("remove Cargo proof outputs");
        }
    }
}
