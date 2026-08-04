mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use crate::toolchain::{PACKAGED_SCANNER_EXCLUDES, TOOLCHAIN_ARMS, declaration};
    use sill::Deadlines;

    static NEXT_CANVAS_FIXTURE: AtomicU64 = AtomicU64::new(0);
    static NEXT_DECODING_FIXTURE: AtomicU64 = AtomicU64::new(0);

    /// The compiled exclusions and the retained sentences are one rule set.
    ///
    /// Both spellings ship in every report, so a rule changed in one place and
    /// not the other would put a false sentence next to a true decision.
    #[test]
    fn compiled_exclusions_render_exactly_the_declared_sentences() {
        let rendered: Vec<String> = SCANNER_EXCLUSIONS
            .iter()
            .map(|exclusion| exclusion.sentence())
            .collect();
        assert_eq!(rendered, PACKAGED_SCANNER_EXCLUDES);
    }

    /// The formatter excludes build output, which is a different rule set.
    ///
    /// Formatters is the one scanning arm that does not use the scanner
    /// exclusions, so the two tables are proven distinct as well as faithful.
    #[test]
    fn compiled_build_exclusions_render_exactly_the_declared_globs() {
        let declared = declaration(GateId::Formatters)
            .expect("formatters is declared")
            .jurisdiction
            .excludes;
        let expected: Vec<String> = BUILD_EXCLUSIONS
            .iter()
            .map(|directory| format!("{directory}/**"))
            .collect();
        assert_eq!(declared, expected);
        assert_ne!(declared, PACKAGED_SCANNER_EXCLUDES);
    }

    /// Build exclusions bind at the top level only, unlike scanner exclusions.
    #[test]
    fn build_exclusions_remove_only_a_leading_component() {
        assert!(built("target/debug/app.rs"));
        assert!(built("vendor/crate/src/lib.rs"));
        assert!(!built("gate/target/debug/app.rs"));
        assert!(!built("target"));
        assert!(!built("src/main.rs"));
    }

    /// The compiled prose extensions and the declared globs are one selection.
    #[test]
    fn compiled_prose_extensions_cover_exactly_the_declared_globs() {
        let declared = declaration(GateId::Taste)
            .expect("taste is declared")
            .jurisdiction
            .includes;
        let expected: Vec<String> = PROSE_EXTENSIONS
            .iter()
            .map(|extension| format!("**/*.{extension}"))
            .collect();
        assert_eq!(declared, expected);

        let rust = declaration(GateId::Formatters)
            .expect("formatters is declared")
            .jurisdiction
            .includes;
        assert!(rust.contains(&format!("**/*.{RUST_EXTENSION}").as_str()));
    }

    #[test]
    fn compiled_javascript_extensions_cover_exactly_the_declared_globs() {
        let declared = declaration(GateId::JavascriptUnused)
            .expect("javascript-unused is declared")
            .jurisdiction
            .includes;
        let expected: Vec<String> = JAVASCRIPT_EXTENSIONS
            .iter()
            .map(|extension| format!("**/*.{extension}"))
            .collect();
        assert_eq!(declared, expected);
    }

    /// Every declared arm routes somewhere named, so one more cannot drift in.
    #[test]
    fn every_declared_arm_has_a_named_analysis_or_a_named_owed_analyzer() {
        for declaration in TOOLCHAIN_ARMS {
            let compiled = matches!(
                declaration.id,
                GateId::Formatters
                    | GateId::LocalPaths
                    | GateId::Taste
                    | GateId::JavascriptUnused
                    | GateId::CsharpUnusedCandidates
                    | GateId::CanvasWebTypecheck
                    | GateId::EnvPolicy
                    | GateId::EnvNameLedger
            );
            let owed = super::super::acquire::owed_analyzer(declaration.id);
            assert_eq!(
                compiled,
                owed.is_none(),
                "{} is both compiled and owed, or neither",
                declaration.id.as_str()
            );
        }
    }

    #[test]
    fn canvas_argument_vectors_are_exactly_the_two_declared_analyses() {
        let declared = declaration(GateId::CanvasWebTypecheck)
            .expect("canvas arm is declared")
            .analysis;
        let compiled = [
            format!("{CARGO} {}", CARGO_WASM_ARGUMENTS.join(" ")),
            format!("{TRUNK} {}", TRUNK_BUILD_ARGUMENTS.join(" ")),
        ];

        assert_eq!(compiled, declared);
    }

    #[test]
    fn canvas_runs_both_declared_commands_in_order_from_their_owned_directories() {
        let fixture = CanvasFixture::new("ordered", true);
        let cargo = fixture.script("cargo", "exit 0");
        let trunk = fixture.script("trunk", "exit 0");

        let analysis = fixture.analyze(&cargo, &trunk, &canvas_bounds(Duration::from_secs(2)));

        assert_eq!(analysis.outcome, ArmOutcome::Passed);
        assert!(analysis.findings.is_empty());
        assert_eq!(analysis.exit_code, None);
        let root = fs::canonicalize(&fixture.root).expect("canonical fixture root");
        assert_eq!(fixture.log_lines(), [
            format!(
                "{}|check --locked --workspace --target wasm32-unknown-unknown",
                root.display()
            ),
            format!(
                "{}|build index.html --release --locked --filehash=false",
                root.join("app").display()
            ),
        ]);
    }

    #[test]
    fn canvas_applicability_refuses_before_either_analysis_runs() {
        let fixture = CanvasFixture::new("not-applicable", false);
        let cargo = fixture.script("cargo", "exit 99");
        let trunk = fixture.script("trunk", "exit 99");

        let analysis = fixture.analyze(&cargo, &trunk, &canvas_bounds(Duration::from_secs(2)));

        assert_eq!(analysis.outcome, ArmOutcome::NotApplicable);
        assert!(fixture.log_lines().is_empty());
    }

    #[test]
    fn canvas_typed_diagnostic_stops_the_pair_with_source_evidence() {
        let fixture = CanvasFixture::new("diagnostic", true);
        let cargo = fixture.script(
            "cargo",
            concat!(
                "printf '%s\\n' 'error[E0308]: mismatched types' ",
                "' --> app/src/lib.rs:7:9' >&2\n",
                "exit 1",
            ),
        );
        let trunk = fixture.script("trunk", "exit 99");

        let analysis = fixture.analyze(&cargo, &trunk, &canvas_bounds(Duration::from_secs(2)));

        assert_eq!(analysis.outcome, ArmOutcome::Findings);
        assert_eq!(analysis.exit_code, None);
        assert_eq!(analysis.findings, [ArmFinding {
            path: Some("app/src/lib.rs".to_owned()),
            line: Some(7),
            column: Some(9),
            detail: FindingDetail::CanvasWebOutput {
                code: "E0308".to_owned(),
                message: "mismatched types".to_owned(),
            },
        }]);
        assert_eq!(fixture.log_lines().len(), 1, "trunk ran after a finding");
    }

    #[test]
    fn canvas_unparsed_nonzero_retains_the_code_and_invents_no_findings() {
        let fixture = CanvasFixture::new("nonzero", true);
        let cargo = fixture.script("cargo", "printf 'opaque refusal\\n' >&2\nexit 17");
        let trunk = fixture.script("trunk", "exit 99");

        let analysis = fixture.analyze(&cargo, &trunk, &canvas_bounds(Duration::from_secs(2)));

        assert_eq!(analysis.outcome, ArmOutcome::NonzeroAnalysis);
        assert_eq!(analysis.exit_code, Some(17));
        assert!(analysis.findings.is_empty());
        assert_eq!(fixture.log_lines().len(), 1, "trunk ran after refusal");
    }

    #[test]
    fn canvas_small_runtime_bound_reports_timeout_and_stops_the_pair() {
        let fixture = CanvasFixture::new("timeout", true);
        let cargo = fixture.script("cargo", "/bin/sleep 5\nexit 0");
        let trunk = fixture.script("trunk", "exit 99");

        let analysis = fixture.analyze(&cargo, &trunk, &canvas_bounds(Duration::from_millis(25)));

        assert_eq!(analysis.outcome, ArmOutcome::Timeout);
        assert_eq!(analysis.exit_code, None);
        assert!(analysis.findings.is_empty());
        assert!(analysis.reason.contains("cargo wasm32 check"));
        assert!(
            fixture
                .log_lines()
                .iter()
                .all(|call| !call.contains("build index.html")),
            "trunk ran after timeout"
        );
    }

    #[test]
    fn exclusions_remove_a_component_anywhere_in_the_path() {
        assert!(excluded("node_modules/left-pad/index.js"));
        assert!(excluded("web/node_modules/index.js"));
        assert!(excluded(".spice-worktree/notes.md"));
        assert!(!excluded("src/node_modules_helper.rs"));
        assert!(!excluded("docs/guide.md"));
    }

    #[test]
    fn extensions_are_lowercased_and_absent_for_dotfiles() {
        assert_eq!(extension("docs/GUIDE.MD").as_deref(), Some("md"));
        assert_eq!(extension("src/main.rs").as_deref(), Some("rs"));
        assert_eq!(extension(".gitignore"), None);
        assert_eq!(extension("Makefile"), None);
    }

    /// A trailing star matches any continuation, including none at all.
    #[test]
    fn compiled_vocabulary_matches_whole_words_and_ignores_case() {
        assert!(matches_pattern("orphan*", "Orphaned"));
        assert!(matches_pattern("orphan*", "orphan"));
        assert!(matches_pattern("whitelist", "WHITELIST"));
        assert!(!matches_pattern("whitelist", "whitelists"));
        assert!(!matches_pattern("orphan*", "morphan"));
    }

    /// One word twice on one line is two occurrences at two columns.
    ///
    /// Each is a separate place the prose has to be rewritten. They agree in
    /// word and replacement, so the column is the only thing that keeps the
    /// second from being refused as a finding identical to the first.
    #[test]
    fn a_word_repeated_on_one_line_is_caught_at_every_occurrence() {
        let (pattern, replacement) = PACKAGED_TASTE_WORDS
            .first()
            .expect("the compiled vocabulary is not empty");
        let word = pattern.trim_end_matches('*');
        let line = format!("{word} and {word} again");
        let found = discouraged(&line);
        assert_eq!(found.len(), 2, "one line, two occurrences, two findings");
        for occurrence in &found {
            assert_eq!(occurrence.word, word);
            assert_eq!(occurrence.replacement, *replacement);
        }
        assert_eq!(found[0].column, 1);
        assert_eq!(
            u32::try_from(line.rfind(word).expect("the second occurrence") + 1),
            Ok(found[1].column)
        );
    }

    #[test]
    fn words_split_on_everything_that_is_not_a_word_character() {
        assert_eq!(words("a whitelist, and-a blacklist."), [
            (1, "a".to_owned()),
            (3, "whitelist".to_owned()),
            (14, "and".to_owned()),
            (18, "a".to_owned()),
            (20, "blacklist".to_owned())
        ]);
        assert_eq!(words("   "), Vec::<(u32, String)>::new());
    }

    /// A column is a character count, not a byte count.
    #[test]
    fn a_word_after_a_multibyte_character_opens_at_a_character_column() {
        assert_eq!(words("\u{e9}\u{e9} whitelist"), [
            (1, "\u{e9}\u{e9}".to_owned()),
            (4, "whitelist".to_owned())
        ]);
    }

    /// A change path that is not tree-relative would read outside the tree.
    ///
    /// `Path::join` discards the root when handed an absolute path, so this is
    /// the difference between scanning the capture and scanning the machine.
    #[test]
    fn a_change_path_that_is_not_tree_relative_is_never_selected() {
        let temporary = std::env::temp_dir();
        let planted = temporary.join("gate-analyze-outside-selection");
        fs::write(&planted, "planted\n").expect("plant one file outside the tree");
        let root = temporary.join("gate-analyze-empty-tree");
        fs::create_dir_all(&root).expect("create one empty selected tree");

        let absolute = planted.to_str().expect("a UTF-8 temporary path").to_owned();
        assert!(
            root.join(&absolute).is_file(),
            "joining an absolute change onto the root must escape it, or this proves nothing"
        );

        let changes = vec![
            absolute,
            "./src/app.rs".to_owned(),
            "../peer/src/app.rs".to_owned(),
            String::new(),
        ];
        assert_eq!(
            selected_changes(&root, &changes, |_| true),
            Vec::<String>::new()
        );

        fs::remove_file(&planted).expect("remove the planted file");
        fs::remove_dir(&root).expect("remove the empty selected tree");
    }

    #[test]
    fn a_reported_region_outside_the_selected_tree_is_not_a_finding() {
        let root = Path::new("/selected/tree");
        let printed = format!(
            "{DIFF_PREFIX}/elsewhere/src/main.rs:4:\n{DIFF_PREFIX}/selected/tree/src/app.rs:9:\n"
        );
        let findings = differing_files(root, &printed);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].path.as_deref(), Some("src/app.rs"));
        assert_eq!(findings[0].line, Some(9));
    }

    /// Many reported regions in one file are still one unformatted file.
    #[test]
    fn repeated_regions_collapse_to_the_earliest_line_of_each_file() {
        let root = Path::new("/selected/tree");
        let printed = format!(
            "{DIFF_PREFIX}/selected/tree/src/app.rs:31:\n\
             {DIFF_PREFIX}/selected/tree/src/app.rs:8:\n"
        );
        let findings = differing_files(root, &printed);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, Some(8));
    }

    /// Every analysis that decodes selected bytes refuses the same read fault.
    ///
    /// Effective-user privilege can make mode-bit fixtures readable even after
    /// a test removes every read bit. Injecting the acquisition failure at the
    /// private reader boundary proves the semantic contract on every platform
    /// and under every container user without changing production acquisition.
    #[test]
    fn a_source_read_refusal_terminates_every_decoding_arm() {
        const SETTINGS: &str = "app/settings.py";
        const PROSE: &str = "docs/guide.md";

        fn refused_source(_path: &Path) -> io::Result<Vec<u8>> {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected source read refusal",
            ))
        }

        let fixture = DecodingFixture::new();
        let settings = vec![SETTINGS.to_owned()];
        let prose = vec![PROSE.to_owned()];
        let analyses = [
            (
                GateId::LocalPaths,
                SETTINGS,
                local_paths_with(
                    declaration(GateId::LocalPaths).expect("local-paths declaration"),
                    &fixture.root,
                    &settings,
                    refused_source,
                ),
            ),
            (
                GateId::Taste,
                PROSE,
                taste_with(
                    declaration(GateId::Taste).expect("taste declaration"),
                    &fixture.root,
                    &prose,
                    refused_source,
                ),
            ),
            (
                GateId::EnvPolicy,
                SETTINGS,
                env_policy_with(
                    declaration(GateId::EnvPolicy).expect("env-policy declaration"),
                    &fixture.root,
                    &settings,
                    refused_source,
                ),
            ),
            (
                GateId::EnvNameLedger,
                SETTINGS,
                env_name_ledger_with(
                    declaration(GateId::EnvNameLedger).expect("env-name-ledger declaration"),
                    &fixture.root,
                    refused_source,
                ),
            ),
        ];

        for (id, source, analysis) in analyses {
            assert_eq!(
                analysis.outcome,
                ArmOutcome::ParseFailure,
                "{} answered for bytes it never read: {}",
                id.as_str(),
                analysis.reason,
            );
            assert!(
                analysis.reason.contains(source),
                "{} refused without naming {source}: {}",
                id.as_str(),
                analysis.reason,
            );
        }
    }

    struct DecodingFixture {
        root: PathBuf,
    }

    impl DecodingFixture {
        fn new() -> Self {
            let sequence = NEXT_DECODING_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "gate-decoding-{}-{sequence}",
                std::process::id(),
            ));
            if root.exists() {
                fs::remove_dir_all(&root).expect("remove stale decoding fixture");
            }
            fs::create_dir_all(root.join("app")).expect("create application fixture directory");
            fs::create_dir_all(root.join("docs")).expect("create prose fixture directory");
            fs::write(root.join("app/settings.py"), "import os\n")
                .expect("write environment fixture source");
            fs::write(root.join("docs/guide.md"), "Ordinary prose.\n")
                .expect("write prose fixture source");
            Self { root }
        }
    }

    impl Drop for DecodingFixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).expect("remove decoding fixture");
        }
    }

    struct CanvasFixture {
        root: PathBuf,
        log: PathBuf,
    }

    impl CanvasFixture {
        fn new(label: &str, applicable: bool) -> Self {
            let sequence = NEXT_CANVAS_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "gate-canvas-{label}-{}-{sequence}",
                std::process::id()
            ));
            if root.exists() {
                fs::remove_dir_all(&root).expect("remove stale canvas fixture");
            }
            fs::create_dir_all(root.join("app/src")).expect("create canvas fixture tree");
            fs::write(root.join("app/src/lib.rs"), "pub fn canvas() {}\n")
                .expect("write canvas source");
            if applicable {
                fs::write(root.join("Cargo.toml"), "[workspace]\n")
                    .expect("write workspace manifest");
                fs::write(root.join("app/index.html"), "<main></main>\n")
                    .expect("write canvas entrypoint");
            }
            let log = root.join("calls.log");
            Self { root, log }
        }

        fn script(&self, name: &str, body: &str) -> PathBuf {
            let path = self.root.join(name);
            let log = self.log.to_string_lossy();
            assert!(!log.contains('\''), "fixture log path cannot be shell quoted");
            fs::write(
                &path,
                format!(
                    "#!/bin/sh\nprintf '%s|%s\\n' \"$(pwd)\" \"$*\" >> '{log}'\n{body}\n"
                ),
            )
            .expect("write canvas tool stub");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .expect("make canvas tool stub executable");
            path
        }

        fn analyze(&self, cargo: &Path, trunk: &Path, bounds: &Bounds) -> Analysis {
            let programs = BTreeMap::from([
                (CARGO, cargo.to_str().expect("UTF-8 cargo stub")),
                (TRUNK, trunk.to_str().expect("UTF-8 trunk stub")),
            ]);
            canvas_web_typecheck(
                declaration(GateId::CanvasWebTypecheck).expect("canvas arm is declared"),
                &self.root,
                bounds,
                &programs,
            )
        }

        fn log_lines(&self) -> Vec<String> {
            let text = match fs::read_to_string(&self.log) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(error) => panic!("read canvas call log: {error}"),
            };
            text
                .lines()
                .map(str::to_owned)
                .collect()
        }
    }

    impl Drop for CanvasFixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).expect("remove canvas fixture");
        }
    }

    fn canvas_bounds(runtime: Duration) -> Bounds {
        Bounds::new(
            "canvas analyzer test",
            Deadlines::new(runtime, Duration::from_secs(1), Duration::from_secs(1))
                .expect("valid canvas deadlines"),
            Duration::from_millis(25),
            16 * 1024,
        )
        .expect("valid canvas bounds")
    }
}
