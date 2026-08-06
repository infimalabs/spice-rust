mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::Duration;

    use crate::toolchain::{
        PACKAGED_SCANNER_EXCLUDES, TOOLCHAIN_ARMS, declaration, fixture_external_finding,
    };
    use sill::{Acquisition, Deadlines};

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
        let expected: Vec<String> = TASTE_EXTENSIONS
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
                    | GateId::RustTypecheck
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

        let analysis = fixture.analyze(&cargo, &trunk, &canvas_exit_bounds());

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
    fn canvas_routes_generated_bytes_outside_the_selected_tree() {
        let fixture = CanvasFixture::new("external-output", true);
        let cargo = fixture.script(
            "cargo",
            concat!(
                "[ -d \"$CARGO_TARGET_DIR\" ] || exit 41\n",
                "[ -d \"$CARGO_BUILD_BUILD_DIR\" ] || exit 43\n",
                "printf cargo > \"$CARGO_TARGET_DIR/proof\"\n",
                "printf build > \"$CARGO_BUILD_BUILD_DIR/proof\"\n",
                "exit 0",
            ),
        );
        let trunk = fixture.script(
            "trunk",
            concat!(
                "[ -d \"$TRUNK_BUILD_DIST\" ] || exit 42\n",
                "printf trunk > \"$TRUNK_BUILD_DIST/proof\"\n",
                "exit 0",
            ),
        );

        let analysis = fixture.analyze(&cargo, &trunk, &canvas_exit_bounds());

        assert_eq!(analysis.outcome, ArmOutcome::Passed, "{}", analysis.reason);
        assert!(!fixture.root.join("target").exists());
        assert!(!fixture.root.join("app/dist").exists());
        assert!(fixture.output.join("target/xtask-app/proof").is_file());
        assert!(
            fixture
                .output
                .join("target/xtask-app-build/proof")
                .is_file()
        );
        assert!(!CanvasFixture::workspace_root(&fixture.root).exists());
    }

    /// Two canvas analyses of one repository run at once and share its build root.
    ///
    /// The claim is that the shared root is safe, so the two analyses have to
    /// genuinely overlap rather than be asserted not to. Each stub announces
    /// itself into the shared `CARGO_TARGET_DIR` and then meets the other
    /// through a pair of named pipes: opening one for writing finishes only once
    /// the other side opens it for reading, so neither stub gets past the
    /// rendezvous unless both are running. Should either analysis refuse before
    /// reaching its stub, the rendezvous never completes and the bounded runtime
    /// ends the arm rather than leaving the suite stopped.
    ///
    /// What real Cargo does under that overlap was measured rather than stubbed,
    /// because it is Cargo's behaviour and not this crate's: two `cargo check`
    /// runs sharing one `CARGO_TARGET_DIR` and `CARGO_BUILD_BUILD_DIR`, the
    /// first held inside a build script, made the second report `Blocking
    /// waiting for file lock on build directory` and then finish, both exiting
    /// zero. Cargo serializes the shared root; it does not corrupt it.
    #[test]
    fn two_canvas_analyses_at_once_share_one_repository_build_root() {
        let fixture = CanvasFixture::new("shared-build-root", true);
        let second_tree = fixture.second_tree("second-tree");
        let reached = fixture.base.join("first-reached");
        let released = fixture.base.join("second-reached");
        make_pipe(&reached);
        make_pipe(&released);

        let first_cargo = fixture.script(
            "cargo-first",
            &format!(
                concat!(
                    "printf first > \"$CARGO_TARGET_DIR/first\"\n",
                    "printf x > '{}'\n",
                    "cat '{}' > /dev/null\n",
                    "exit 0",
                ),
                reached.display(),
                released.display(),
            ),
        );
        let second_cargo = fixture.script(
            "cargo-second",
            &format!(
                concat!(
                    "printf second > \"$CARGO_TARGET_DIR/second\"\n",
                    "cat '{}' > /dev/null\n",
                    "printf x > '{}'\n",
                    "exit 0",
                ),
                reached.display(),
                released.display(),
            ),
        );
        let trunk = fixture.script("trunk", "exit 0");
        let bounds = canvas_bounds(Duration::from_secs(30));

        let (first, second) = thread::scope(|scope| {
            let other = scope.spawn(|| {
                fixture.analyze_tree(&second_tree, &second_cargo, &trunk, &bounds)
            });
            let mine = fixture.analyze(&first_cargo, &trunk, &bounds);
            (mine, other.join().expect("the second analysis does not panic"))
        });

        assert_eq!(first.outcome, ArmOutcome::Passed, "{}", first.reason);
        assert_eq!(second.outcome, ArmOutcome::Passed, "{}", second.reason);
        let shared = fixture.output.join("target/xtask-app");
        assert!(
            shared.join("first").is_file() && shared.join("second").is_file(),
            "both overlapping analyses compiled into {}",
            shared.display(),
        );
        assert!(
            shared.is_dir() && fixture.output.join("target/xtask-app-build").is_dir(),
            "the shared pair outlives the analyses that used it",
        );
        assert!(!CanvasFixture::workspace_root(&fixture.root).exists());
        assert!(!CanvasFixture::workspace_root(&second_tree).exists());
    }

    /// Plant a canvas workspace holding one artefact, as a live analysis leaves it.
    fn plant_canvas_workspace(tree: &Path) -> (PathBuf, PathBuf) {
        let root = CanvasFixture::workspace_root(tree);
        fs::create_dir_all(root.join("trunk-dist")).expect("plant a canvas workspace");
        let artefact = root.join("trunk-dist/planted.wasm");
        fs::write(&artefact, b"artefact").expect("plant a canvas artefact");
        (root, artefact)
    }

    /// A workspace a live analysis is writing into is never removed under it.
    ///
    /// The holder is a second acquisition of the very lock the arm takes.
    /// `flock` grants per open file description, so this contends exactly as a
    /// separate process would, and holding it *before* the arm starts is what
    /// makes the decision drivable: the refusal is the arm's answer to a live
    /// holder rather than the outcome of a race this test happened to win.
    #[test]
    fn a_canvas_workspace_a_live_analysis_holds_is_refused_and_survives_intact() {
        let fixture = CanvasFixture::new("held-workspace-lock", true);
        let (root, artefact) = plant_canvas_workspace(&fixture.root);
        let lock = build_root::canvas_workspace_lock(&root).expect("the workspace names a lock");
        let Ok(Acquisition::Acquired(held)) = lock.try_acquire() else {
            panic!("a freshly planted canvas workspace must be lockable");
        };
        let cargo = fixture.script("cargo", "exit 0");
        let trunk = fixture.script("trunk", "exit 0");

        let analysis = fixture.analyze(&cargo, &trunk, &canvas_exit_bounds());

        assert_eq!(analysis.outcome, ArmOutcome::ParseFailure);
        assert!(
            analysis.reason.contains(&root.display().to_string())
                && analysis
                    .reason
                    .contains("is in use by another analysis of this tree"),
            "the refusal names the workspace and why it refused: {}",
            analysis.reason,
        );
        assert!(
            fixture.log_lines().is_empty(),
            "neither tool ran: {:?}",
            fixture.log_lines(),
        );
        assert_eq!(
            fs::read(&artefact).expect("the held analysis's artefact survives"),
            b"artefact",
            "the held workspace was removed out from under its analysis",
        );
        drop(held);
    }

    /// The guard cannot pass by refusing every workspace it did not create.
    ///
    /// A workspace no analysis holds is what a killed run leaves behind, and its
    /// name comes from the selected tree rather than from anything this
    /// invocation minted, so refusing it would poison every later canvas
    /// analysis of that tree until an operator cleaned up by hand. Possession is
    /// what separates the two cases, so this one is reclaimed, and the planted
    /// artefact is asserted gone rather than merely unmentioned.
    #[test]
    fn a_canvas_workspace_no_live_analysis_holds_is_reclaimed_rather_than_refused() {
        let fixture = CanvasFixture::new("stale-workspace", true);
        let (root, artefact) = plant_canvas_workspace(&fixture.root);
        let cargo = fixture.script("cargo", "exit 0");
        let trunk = fixture.script("trunk", "exit 0");

        let analysis = fixture.analyze(&cargo, &trunk, &canvas_exit_bounds());

        assert_eq!(analysis.outcome, ArmOutcome::Passed, "{}", analysis.reason);
        assert!(
            !artefact.exists(),
            "the stale artefact outlived the run that abandoned it",
        );
        assert!(
            !fixture.log_lines().is_empty(),
            "the analysis the stale workspace would have blocked really ran",
        );
        assert!(!root.exists(), "teardown removed the reclaimed workspace");
    }

    /// A workspace that cannot be removed is reported, not dropped on the floor.
    ///
    /// The stub makes the directory holding the workspace read-only while the
    /// analysis is between creating that workspace and removing it, so the
    /// removal fails for the one reason a real one would: the entry cannot be
    /// unlinked from its parent. The report outranks the clean analysis because
    /// the workspace it left behind is what the previous test refuses.
    #[test]
    fn a_canvas_workspace_that_cannot_be_removed_is_reported_not_discarded() {
        let fixture = CanvasFixture::new("held-workspace", true);
        let cargo = fixture.script(
            "cargo",
            &format!("/bin/chmod 555 '{}'\nexit 0", fixture.base.display()),
        );
        let trunk = fixture.script("trunk", "exit 0");

        let analysis = fixture.analyze(&cargo, &trunk, &canvas_exit_bounds());

        fs::set_permissions(&fixture.base, fs::Permissions::from_mode(0o755))
            .expect("restore the fixture directory before anything can panic");

        assert_eq!(analysis.outcome, ArmOutcome::ParseFailure, "{}", analysis.reason);
        let workspace = CanvasFixture::workspace_root(&fixture.root);
        assert!(
            analysis.reason.contains(&workspace.display().to_string())
                && analysis.reason.contains("could not be removed"),
            "the report names the workspace and what happened to it: {}",
            analysis.reason,
        );
        assert!(workspace.is_dir(), "the workspace really did survive the run");
    }

    #[test]
    fn canvas_applicability_refuses_before_either_analysis_runs() {
        let fixture = CanvasFixture::new("not-applicable", false);
        let cargo = fixture.script("cargo", "exit 99");
        let trunk = fixture.script("trunk", "exit 99");

        let analysis = fixture.analyze(&cargo, &trunk, &canvas_exit_bounds());

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

        let analysis = fixture.analyze(&cargo, &trunk, &canvas_exit_bounds());

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

        let analysis = fixture.analyze(&cargo, &trunk, &canvas_exit_bounds());

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

    /// The formatter fixture reports the column this producer reports.
    ///
    /// Nothing downstream would notice them drifting: this arm refuses
    /// predecessor comparison before any finding equality runs, so a fixture
    /// naming a column would describe a locus the formatter has no way to
    /// report and would never be contradicted. Asking the producer is what
    /// makes the disagreement arrive here instead of nowhere.
    #[test]
    fn the_formatter_fixture_reports_the_column_this_arm_does() {
        let root = Path::new("/selected/tree");
        let printed = format!("{DIFF_PREFIX}/selected/tree/src/app.rs:9:\n");
        let findings = differing_files(root, &printed);

        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].column,
            fixture_external_finding(GateId::Formatters, "a").column,
            "the formatter arm and its fixture disagree on the column"
        );
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
                    &[],
                    refused_source,
                    |_| true,
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

    /// One repository holding its selected trees, so a run owns everything it makes.
    ///
    /// The selected tree is nested rather than placed straight in the temporary
    /// directory because the workspace the arm creates is a *sibling* of the
    /// selected tree. A tree at the top of `/tmp` puts that sibling there too,
    /// where nothing in this fixture can reach it: an analysis killed mid-run
    /// would leave it behind for good, and no test could make its removal fail
    /// without making `/tmp` itself unwritable. Nesting puts the sibling inside
    /// `base`, which this fixture creates, reaches and removes.
    struct CanvasFixture {
        base: PathBuf,
        root: PathBuf,
        output: PathBuf,
        log: PathBuf,
    }

    impl CanvasFixture {
        fn new(label: &str, applicable: bool) -> Self {
            let sequence = NEXT_CANVAS_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let base = std::env::temp_dir().join(format!(
                "gate-canvas-{label}-{}-{sequence}",
                std::process::id()
            ));
            if base.exists() {
                fs::remove_dir_all(&base).expect("remove stale canvas fixture");
            }
            let root = base.join("tree");
            let output = base.join("repository");
            fs::create_dir_all(&output).expect("create canvas output fixture");
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
            Self {
                base,
                root,
                output,
                log,
            }
        }

        /// The sibling directory the arm creates for the tree it is handed.
        fn workspace_root(tree: &Path) -> PathBuf {
            let mut name = tree.file_name().expect("selected tree is named").to_os_string();
            name.push(".spice-canvas-analysis");
            tree.with_file_name(name)
        }

        /// A second selected tree in the same repository, analyzable on its own.
        fn second_tree(&self, name: &str) -> PathBuf {
            let tree = self.base.join(name);
            fs::create_dir_all(tree.join("app/src")).expect("create second canvas tree");
            fs::write(tree.join("app/src/lib.rs"), "pub fn canvas() {}\n")
                .expect("write second canvas source");
            fs::write(tree.join("Cargo.toml"), "[workspace]\n")
                .expect("write second workspace manifest");
            fs::write(tree.join("app/index.html"), "<main></main>\n")
                .expect("write second canvas entrypoint");
            tree
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
            self.analyze_tree(&self.root, cargo, trunk, bounds)
        }

        fn analyze_tree(
            &self,
            tree: &Path,
            cargo: &Path,
            trunk: &Path,
            bounds: &Bounds,
        ) -> Analysis {
            let programs = BTreeMap::from([
                (CARGO, cargo.to_str().expect("UTF-8 cargo stub")),
                (TRUNK, trunk.to_str().expect("UTF-8 trunk stub")),
            ]);
            canvas_web_typecheck(
                declaration(GateId::CanvasWebTypecheck).expect("canvas arm is declared"),
                tree,
                &self.output,
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
            fs::remove_dir_all(&self.base).expect("remove canvas fixture");
        }
    }

    /// Bounds for the fixtures whose outcome is decided by the stub's exit status.
    ///
    /// These fixtures assert nothing about the runtime deadline, so a deadline that
    /// can elapse only gives them a second way to finish: on a loaded lane the wrapper
    /// can take longer to observe a stub that exited immediately than any budget worth
    /// naming, and the arm then reports Timeout instead of what the stub said. A runtime
    /// that cannot elapse removes that race by construction rather than lengthening it,
    /// leaving the blocking wait on the child's exit as the only way these fixtures end.
    fn canvas_exit_bounds() -> Bounds {
        canvas_bounds(Duration::MAX)
    }

    /// A named pipe two stubs meet at.
    ///
    /// The meeting is what makes overlap a fact rather than a hope: opening one
    /// end finishes only once the other end is open, so both stubs are provably
    /// running at the same moment without either one measuring a clock a loaded
    /// lane could outrun.
    fn make_pipe(path: &Path) {
        let status = std::process::Command::new("/usr/bin/mkfifo")
            .arg(path)
            .status()
            .expect("run mkfifo");
        assert!(status.success(), "mkfifo {} said {status}", path.display());
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
