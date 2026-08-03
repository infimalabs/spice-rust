mod tests {
    use super::*;
    use crate::toolchain::{PACKAGED_SCANNER_EXCLUDES, TOOLCHAIN_ARMS, declaration};

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

    /// Every declared arm routes somewhere named, so a ninth cannot drift in.
    #[test]
    fn every_declared_arm_has_a_named_analysis_or_a_named_owed_analyzer() {
        for declaration in TOOLCHAIN_ARMS {
            let compiled = matches!(
                declaration.id,
                GateId::Formatters
                    | GateId::LocalPaths
                    | GateId::Taste
                    | GateId::JavascriptUnused
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

    /// One word twice on one line is one finding, because findings carry no column.
    #[test]
    fn a_word_repeated_on_one_line_is_caught_once() {
        let (pattern, replacement) = PACKAGED_TASTE_WORDS
            .first()
            .expect("the compiled vocabulary is not empty");
        let word = pattern.trim_end_matches('*');
        assert_eq!(discouraged(&format!("{word} and {word} again")), [(
            word.to_owned(),
            (*replacement).to_owned()
        )]);
    }

    #[test]
    fn words_split_on_everything_that_is_not_a_word_character() {
        assert_eq!(words("a whitelist, and-a blacklist."), [
            "a",
            "whitelist",
            "and",
            "a",
            "blacklist"
        ]);
        assert_eq!(words("   "), Vec::<String>::new());
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
}
