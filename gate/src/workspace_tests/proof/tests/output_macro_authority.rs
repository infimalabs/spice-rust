use super::*;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

const COMPILER_MACROS: [(&str, &str); 14] = [
    ("println", "\"safe\""),
    ("format", "\"safe {}\", 1"),
    ("env", "\"OUT_DIR\""),
    ("option_env", "\"OUT_DIR\""),
    ("concat", "\"safe\""),
    ("include", "\"body.rs\""),
    ("include_str", "\"body.rs\""),
    ("include_bytes", "\"body.rs\""),
    ("cfg", "any()"),
    ("file", ""),
    ("line", ""),
    ("column", ""),
    ("module_path", ""),
    ("stringify", "safe"),
];

#[test]
fn dependency_and_ambiguous_output_macros_are_non_cacheable() {
    let fixture = OutputMacroFixture::new();
    for (name, source, expected) in [
        (
            "qualified dependency",
            "fn main() { helper::println!(\"safe\"); }\n",
            "cannot prove compiler macro origin for helper::println!",
        ),
        (
            "renamed import",
            "use helper::println as println;\nfn main() { println!(\"safe\"); }\n",
            "resolves println! to a user-owned macro",
        ),
        (
            "later renamed import",
            "fn main() { println!(\"safe\"); }\nuse helper::println as println;\n",
            "resolves println! to a user-owned macro",
        ),
        (
            "glob import",
            "use helper::*;\nfn main() { println!(\"safe\"); }\n",
            "cannot prove compiler macro origin for println!",
        ),
        (
            "local shadow",
            concat!(
                "macro_rules! println { ($message:literal) => {\n",
                "    ::std::println!(\"cargo::rerun-if-changed=../authority.txt\");\n",
                "}; }\n",
                "fn main() { println!(\"safe\"); }\n",
            ),
            "resolves println! to a user-owned macro",
        ),
    ] {
        let reason = fixture.non_cacheable_reason(source);
        assert!(
            reason.contains("Cargo shard is non-cacheable") && reason.contains(expected),
            "{name} unexpectedly admitted: {reason}",
        );
    }

    let reason = fixture.non_cacheable_included_reason();
    assert!(
        reason.contains("cannot prove compiler macro origin for println!"),
        "an import governing an included source unexpectedly admitted: {reason}",
    );

    fixture.rename_helper_to_std();
    let reason = fixture.non_cacheable_reason("fn main() { std::println!(\"safe\"); }\n");
    assert!(
        reason.contains("cannot prove compiler macro origin for std::println!"),
        "a dependency occupying std unexpectedly admitted: {reason}",
    );
}

#[test]
fn direct_standard_and_unshadowed_output_macros_retain_closed_authority() {
    let fixture = Fixture::new("resolved-output-macros");
    let bare = fixture.plan(true);
    assert_closed_output_authority(&bare);

    fs::write(
        fixture.root.join("fixture/build.rs"),
        concat!(
            "fn main() {\n",
            "    std::println!(\"cargo::rerun-if-changed=../proof-authority.txt\");\n",
            "    std::println!(\"cargo::rerun-if-env-changed=FIXTURE_BUILD_INPUT\");\n",
            "}\n",
        ),
    )
    .expect("write standard output-macro build script");
    let qualified = fixture.plan(true);
    assert_closed_output_authority(&qualified);
}

#[test]
fn every_allowlisted_terminal_spelling_requires_compiler_origin() {
    let fixture = OutputMacroFixture::new();
    let mut compiler_source = "fn main() {\n".to_owned();
    for (name, arguments) in COMPILER_MACROS {
        let source = format!("fn main() {{ helper::{name}!({arguments}); }}\n");
        let reason = fixture.non_cacheable_reason(&source);
        assert!(
            reason.contains(&format!(
                "cannot prove compiler macro origin for helper::{name}!"
            )),
            "dependency macro {name}! unexpectedly admitted: {reason}",
        );
        compiler_source.push_str(&format!("    helper::{name}!({arguments});\n"));
    }
    compiler_source.push_str("}\n");
    fixture.assert_build_script_compiles(&compiler_source);
}

#[test]
fn standard_and_core_catalog_forms_compile_and_retain_closed_authority() {
    let fixture = OutputMacroFixture::new();
    fs::write(
        fixture.root.join("fixture/body.rs"),
        "const INCLUDED: u8 = 1;\n",
    )
    .expect("write included compiler source");
    let source = concat!(
        "std::include!(\"body.rs\");\n",
        "fn main() {\n",
        "    std::println!(\"safe\");\n",
        "    let _ = std::env!(\"OUT_DIR\");\n",
        "    let _ = core::option_env!(\"OUT_DIR\");\n",
        "    let _ = std::concat!(\"safe\");\n",
        "    let _ = core::include_str!(\"body.rs\");\n",
        "    let _ = std::include_bytes!(\"body.rs\");\n",
        "    let _ = core::cfg!(any());\n",
        "    let _ = std::file!();\n",
        "    let _ = core::line!();\n",
        "    let _ = std::column!();\n",
        "    let _ = core::module_path!();\n",
        "    let _ = std::stringify!(safe);\n",
        "}\n",
    );
    fixture.assert_build_script_compiles(source);
    let plan = fixture.plan(source);
    assert!(
        fixture_shard(&plan).non_cacheable_reason().is_none(),
        "compiler catalog forms unexpectedly refused: {:?}",
        fixture_shard(&plan).non_cacheable_reason(),
    );
}

#[test]
fn nested_macro_inputs_and_textual_shadowing_keep_point_specific_authority() {
    let fixture = OutputMacroFixture::new();
    let nested = fixture
        .non_cacheable_reason("fn main() { let _ = concat!(env!(\"FIXTURE_HIDDEN_INPUT\")); }\n");
    assert!(
        nested.contains("reads undeclared ambient environment \"FIXTURE_HIDDEN_INPUT\""),
        "nested environment input was not scanned: {nested}",
    );
    let attribute = fixture.non_cacheable_reason(
        "#![doc = concat!(env!(\"FIXTURE_ATTRIBUTE_INPUT\"))]\nfn main() {}\n",
    );
    assert!(
        attribute.contains("reads undeclared ambient environment \"FIXTURE_ATTRIBUTE_INPUT\""),
        "attribute environment input was not scanned: {attribute}",
    );
    let dependency =
        fixture.non_cacheable_reason("fn main() { let _ = concat!(helper::env!(\"HIDDEN\")); }\n");
    assert!(
        dependency.contains("cannot prove compiler macro origin for helper::env!"),
        "nested dependency macro was not classified: {dependency}",
    );

    let before = fixture.plan(concat!(
        "fn main() { let _ = env!(\"OUT_DIR\"); }\n",
        "macro_rules! env { ($name:literal) => { \"owned\" }; }\n",
    ));
    assert!(fixture_shard(&before).non_cacheable_reason().is_none());
    let after = fixture.non_cacheable_reason(concat!(
        "macro_rules! env { ($name:literal) => { \"owned\" }; }\n",
        "fn main() { let _ = env!(\"OUT_DIR\"); }\n",
    ));
    assert!(
        after.contains("resolves env! to a user-owned macro"),
        "textual macro shadow was not applied: {after}",
    );
}

#[test]
fn external_modules_retain_only_rust_visible_macro_scope() {
    let fixture = OutputMacroFixture::new();
    fs::write(
        fixture.root.join("fixture/child.rs"),
        "pub fn read() { let _ = env!(\"OUT_DIR\"); }\n",
    )
    .expect("write external build-script module");
    let shadowed = fixture.non_cacheable_reason(concat!(
        "macro_rules! env { ($name:literal) => { \"owned\" }; }\n",
        "mod child;\nfn main() {}\n",
    ));
    assert!(
        shadowed.contains("resolves env! to a user-owned macro"),
        "external module lost inherited textual scope: {shadowed}",
    );

    let builtin = fixture.plan("mod child;\nfn main() {}\n");
    assert!(
        fixture_shard(&builtin).non_cacheable_reason().is_none(),
        "unexpected no-prelude builtin refusal: {:?}",
        fixture_shard(&builtin).non_cacheable_reason(),
    );
}

#[test]
fn no_prelude_and_explicit_standard_namespace_are_distinct() {
    let fixture = OutputMacroFixture::new();
    let builtin =
        fixture.plan("#![no_implicit_prelude]\nfn main() { let _ = stringify!(authority); }\n");
    assert!(
        fixture_shard(&builtin).non_cacheable_reason().is_none(),
        "unexpected no-prelude builtin refusal: {:?}",
        fixture_shard(&builtin).non_cacheable_reason(),
    );

    let bare = fixture
        .non_cacheable_reason("#![no_implicit_prelude]\nfn main() { println!(\"safe\"); }\n");
    assert!(
        bare.contains("cannot prove compiler macro origin for println!"),
        "bare no-prelude output macro unexpectedly admitted: {bare}",
    );
    let bare_format = fixture.non_cacheable_reason(
        "#![no_implicit_prelude]\nfn main() { let _ = format!(\"safe\"); }\n",
    );
    assert!(
        bare_format.contains("cannot prove compiler macro origin for format!"),
        "bare no-prelude format macro unexpectedly admitted: {bare_format}",
    );
    let explicit = fixture.plan(concat!(
        "#![no_implicit_prelude]\nextern crate std;\n",
        "fn main() { ::std::println!(\"safe\"); }\n",
    ));
    assert!(
        fixture_shard(&explicit).non_cacheable_reason().is_none(),
        "unexpected explicit standard refusal: {:?}",
        fixture_shard(&explicit).non_cacheable_reason(),
    );

    let core = fixture.non_cacheable_reason("fn main() { core::println!(\"safe\"); }\n");
    assert!(
        core.contains("cannot prove compiler macro origin for core::println!"),
        "core::println unexpectedly admitted: {core}",
    );
}

#[test]
fn compiler_and_user_macro_renames_preserve_source_identity() {
    let fixture = OutputMacroFixture::new();
    for source in [
        "use std::env as option_env;\nfn main() { let _ = option_env!(\"OUT_DIR\"); }\n",
        concat!(
            "use ::std as standard;\nuse standard::env as option_env;\n",
            "fn main() { let _ = option_env!(\"OUT_DIR\"); }\n",
        ),
    ] {
        let plan = fixture.plan(source);
        assert!(
            fixture_shard(&plan).non_cacheable_reason().is_none(),
            "compiler rename unexpectedly refused: {:?}",
            fixture_shard(&plan).non_cacheable_reason(),
        );
    }
    let user = fixture.non_cacheable_reason(concat!(
        "#[macro_export] macro_rules! owned { ($name:literal) => { \"owned\" }; }\n",
        "use crate::owned as env;\nfn main() { let _ = env!(\"OUT_DIR\"); }\n",
    ));
    assert!(
        user.contains("resolves env! to a user-owned macro"),
        "user macro rename unexpectedly admitted: {user}",
    );
}

fn fixture_shard(plan: &CargoGateShardPlan) -> &CargoGateShard {
    plan.typechecks()
        .iter()
        .find(|shard| shard.package() == "fixture")
        .expect("fixture typecheck shard is planned")
}

fn assert_closed_output_authority(plan: &CargoGateShardPlan) {
    let shard = plan
        .typechecks()
        .iter()
        .find(|shard| shard.package() == "fixture")
        .expect("fixture typecheck shard is planned");
    assert!(shard.non_cacheable_reason().is_none());
    assert!(
        shard
            .input_paths()
            .contains(&"proof-authority.txt".to_owned())
    );
    let authority = shard
        .compile_time_authorities()
        .iter()
        .find(|authority| authority.kind == "custom-build")
        .expect("fixture build script has compile-time authority");
    assert_eq!(authority.environment, ["FIXTURE_BUILD_INPUT"]);
}

struct OutputMacroFixture {
    root: PathBuf,
}

impl OutputMacroFixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "gate-output-macro-authority-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        for directory in ["fixture/src", "helper/src"] {
            fs::create_dir_all(root.join(directory))
                .expect("create output-macro fixture directory");
        }
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"fixture\", \"helper\"]\nresolver = \"3\"\n",
        )
        .expect("write output-macro workspace manifest");
        fs::write(
            root.join("Cargo.lock"),
            concat!(
                "# This file is automatically @generated by Cargo.\n",
                "version = 4\n\n",
                "[[package]]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
                "dependencies = [\"helper\"]\n\n",
                "[[package]]\nname = \"helper\"\nversion = \"0.1.0\"\n",
            ),
        )
        .expect("write output-macro lockfile");
        fs::write(
            root.join("fixture/Cargo.toml"),
            concat!(
                "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
                "build = \"build.rs\"\n\n",
                "[build-dependencies]\nhelper = { path = \"../helper\" }\n\n",
                "[package.metadata.spice.cargo-proof]\nversion = 1\ncomplete = true\n",
            ),
        )
        .expect("write output-macro package manifest");
        fs::write(
            root.join("fixture/src/lib.rs"),
            "pub const VALUE: u8 = 1;\n",
        )
        .expect("write output-macro package source");
        fs::write(root.join("fixture/body.rs"), "println!(\"safe\");\n")
            .expect("write included output-macro source");
        fs::write(root.join("fixture/child.rs"), "pub fn read() {}\n")
            .expect("write external build-script module");
        fs::write(
            root.join("helper/Cargo.toml"),
            "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("write helper manifest");
        let helper_source = COMPILER_MACROS
            .iter()
            .map(|(name, _)| {
                format!(
                    "#[macro_export] macro_rules! {name} {{ ($($token:tt)*) => \
                     {{{{ ::std::println!(\"cargo::rerun-if-changed=../authority.txt\"); }}}}; }}\n"
                )
            })
            .collect::<String>();
        fs::write(root.join("helper/src/lib.rs"), helper_source).expect("write helper macros");
        fs::write(root.join("authority.txt"), "authority\n")
            .expect("write concealed output authority");
        Self { root }
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "the Cargo planning fixture captures its own test environment"
    )]
    fn plan(&self, build_script: &str) -> CargoGateShardPlan {
        fs::write(self.root.join("fixture/build.rs"), build_script)
            .expect("write output-macro build script");
        let environment =
            Environment::new(std::env::vars_os()).expect("ambient test environment is valid");
        plan_cargo_gate_shards_at(CargoGateShardPlanning {
            root: &self.root,
            base: None,
            source: wire::Digest::compute(wire::Domain::GateTree, b"output-macro-fixture"),
            changed_sources: &["fixture/src/lib.rs".to_owned()],
            selected_paths: &[
                "Cargo.lock".to_owned(),
                "Cargo.toml".to_owned(),
                "authority.txt".to_owned(),
                "fixture/Cargo.toml".to_owned(),
                "fixture/body.rs".to_owned(),
                "fixture/build.rs".to_owned(),
                "fixture/child.rs".to_owned(),
                "fixture/src/lib.rs".to_owned(),
                "helper/Cargo.toml".to_owned(),
                "helper/src/lib.rs".to_owned(),
            ],
            environment,
            cargo: OsStr::new("cargo"),
            include_tests: false,
        })
        .expect("plan output-macro fixture")
    }

    fn non_cacheable_reason(&self, build_script: &str) -> String {
        fixture_shard(&self.plan(build_script))
            .non_cacheable_reason()
            .expect("ambiguous output macro is non-cacheable")
            .to_owned()
    }

    fn assert_build_script_compiles(&self, build_script: &str) {
        fs::write(self.root.join("fixture/build.rs"), build_script)
            .expect("write compiler-backed build script");
        let helper = self.root.join("libhelper.rlib");
        let helper_result = Command::new("rustc")
            .current_dir(&self.root)
            .args(["--edition=2024", "--crate-name=helper", "--crate-type=lib"])
            .arg(self.root.join("helper/src/lib.rs"))
            .arg("-o")
            .arg(&helper)
            .output()
            .expect("compile helper macro crate");
        assert!(
            helper_result.status.success(),
            "helper macro crate must compile: {}",
            String::from_utf8_lossy(&helper_result.stderr),
        );
        let output = self.root.join("build-script");
        let build_result = Command::new("rustc")
            .current_dir(self.root.join("fixture"))
            .env("OUT_DIR", "bounded-out-dir")
            .args(["--edition=2024", "build.rs", "--extern"])
            .arg(format!("helper={}", helper.display()))
            .arg("-o")
            .arg(&output)
            .output()
            .expect("compile build script");
        assert!(
            build_result.status.success(),
            "build script must compile: {}",
            String::from_utf8_lossy(&build_result.stderr),
        );
        fs::remove_file(helper).expect("remove helper compiler output");
        fs::remove_file(output).expect("remove build-script compiler output");
    }

    fn rename_helper_to_std(&self) {
        fs::write(
            self.root.join("fixture/Cargo.toml"),
            concat!(
                "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
                "build = \"build.rs\"\n\n",
                "[build-dependencies]\nstd = { package = \"helper\", path = \"../helper\" }\n\n",
                "[package.metadata.spice.cargo-proof]\nversion = 1\ncomplete = true\n",
            ),
        )
        .expect("rename helper dependency to std");
    }

    fn non_cacheable_included_reason(&self) -> String {
        fs::write(self.root.join("fixture/body.rs"), "println!(\"safe\");\n")
            .expect("write included output-macro source");
        self.non_cacheable_reason("use helper::println;\nfn main() { include!(\"body.rs\"); }\n")
    }
}

impl Drop for OutputMacroFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove output-macro fixture");
    }
}
