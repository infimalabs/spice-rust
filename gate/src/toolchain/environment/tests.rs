mod tests {
    use super::*;
    use crate::toolchain::PACKAGED_ENV_INCLUDES;

    /// The one-based character column one needle occupies in one fixture line.
    fn column_at(line: &str, needle: &str) -> u32 {
        let start = line.find(needle).expect("the fixture line names the needle");
        u32::try_from(line[..start].chars().count() + 1).expect("a fixture column fits")
    }

    /// Every packaged spelling compiles, so no analysis can panic on a tree.
    ///
    /// The predecessor dialect carries a look-behind and a back-reference this
    /// engine has neither of, and both are answered by the compiler in this
    /// module rather than by dropping the pattern. This forces every table.
    #[test]
    fn every_packaged_spelling_compiles_in_this_engine() {
        assert!(!watchlist().is_empty(), "the packaged watchlist is not empty");
        assert!(
            declared_names().is_empty(),
            "release 0.31.0 declares no environment names"
        );
        assert!(!declaration_names().is_empty());
        assert!(!context_names().is_empty());
        assert_eq!(context().as_str(), CONTEXT_TEMPLATE);
        assert_eq!(destructured().as_str(), DESTRUCTURED_TEMPLATE);
        for (family, patterns) in access_gate() {
            assert!(!patterns.is_empty(), "{family} declares an idiom to gate on");
        }
        for (family, patterns) in access_names() {
            assert!(!patterns.is_empty(), "{family} reads a name off its access");
        }
    }

    /// The compiled env extensions and the declared globs are one selection.
    #[test]
    fn compiled_env_extensions_cover_exactly_the_declared_globs() {
        let expected: Vec<String> = ENV_EXTENSIONS
            .iter()
            .map(|extension| format!("**/*.{extension}"))
            .collect();
        assert_eq!(PACKAGED_ENV_INCLUDES, expected);
    }

    /// Every family the profile gives suffixes also gets an idiom and a name.
    #[test]
    fn every_declared_family_has_an_idiom_and_a_finding_name() {
        for (family, suffixes) in PACKAGED_ENV_PROFILE.family_suffixes {
            assert!(!suffixes.is_empty(), "{family} claims no suffix");
            assert!(
                access_gate().iter().any(|(named, _)| named == family),
                "{family} has suffixes and no access idiom to gate them with"
            );
            assert!(
                PACKAGED_ENV_PROFILE
                    .finding_names
                    .iter()
                    .any(|(named, _)| named == family),
                "{family} would report under an invented finding name"
            );
        }
    }

    /// An included extension with no family is still scanned, just not gated.
    #[test]
    fn a_family_is_only_claimed_by_the_suffixes_the_profile_names() {
        assert_eq!(family("py"), Some("python"));
        assert_eq!(family("zsh"), Some("shell"));
        assert_eq!(family("ts"), Some("javascript"));
        assert_eq!(family("rs"), None);
        assert!(ENV_EXTENSIONS.contains(&"rs"));
    }

    #[test]
    fn the_scanner_never_reports_its_own_implementation() {
        assert!(is_scanner_itself("spice/studies/envpolicy.py"));
        assert!(!is_scanner_itself("spice/studies/envpolicyx.py"));
    }

    /// Two reads of one name on one line are two accesses, not one.
    ///
    /// Findings are refused as duplicates when they agree in every field, so
    /// the column is what keeps the second read from being dropped. Collapsing
    /// them would report one place the environment is read where there are two.
    #[test]
    fn two_reads_on_one_line_are_two_accesses_at_two_columns() {
        let read = "os.getenv(\"SPICE_HOOKS\")";
        let line = format!("value = {read} or {read}");
        let found = accesses(Some("python"), &line);
        assert_eq!(found.len(), 2, "one line, two reads, two accesses");
        assert_eq!(found[0].name, "SPICE_HOOKS");
        assert_eq!(found[1].name, "SPICE_HOOKS");
        assert_ne!(found[0].column, found[1].column);
        assert_eq!(found[0].column, column_at(&line, "SPICE_HOOKS"));
    }

    /// A read under a name the watchlist never heard of is still an access.
    ///
    /// The gate names the family idiom because a dynamic read has no literal to
    /// name, and that is the whole reason it exists beside the watchlist.
    #[test]
    fn a_read_no_watchlist_pattern_catches_falls_through_to_the_gate() {
        let line = "home = os.environ[key]";
        let found = accesses(Some("python"), line);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "os env access");
        assert_eq!(found[0].column, column_at(line, "os.environ"));
        assert!(
            accesses(None, line).is_empty(),
            "a file of no declared family has no idiom to be gated by"
        );
    }

    /// The escape guard the packaged shell spelling opens with is honoured.
    #[test]
    fn an_escaped_shell_expansion_is_not_an_access() {
        assert_eq!(accesses(Some("shell"), "echo $HOME").len(), 1);
        assert!(accesses(Some("shell"), r"echo \$HOME").is_empty());
    }

    /// A marker on the line waives that line outright.
    #[test]
    fn a_marker_on_one_line_waives_that_line() {
        let text = concat!(
            "first = os.getenv(\"HOME\")  # env-policy: allow\n",
            "second = os.getenv(\"HOME\")\n"
        );
        assert_eq!(waived_lines(text), BTreeSet::from([1]));
    }

    /// A standalone marker comment waives the whole statement after it.
    ///
    /// The scanner catches whichever line the access is spelled on, which for a
    /// call broken over several lines is not the line a reader would annotate.
    #[test]
    fn a_standalone_marker_waives_the_statement_that_follows_it() {
        let text = concat!(
            "# env-policy: allow\n",
            "value = os.getenv(\n",
            "    \"HOME\",\n",
            ")\n",
            "other = os.getenv(\"HOME\")\n"
        );
        assert_eq!(waived_lines(text), BTreeSet::from([1, 2, 3, 4]));
    }

    /// A bracket inside a string cannot open a statement that never closes.
    #[test]
    fn string_contents_and_comments_do_not_decide_where_a_statement_ends() {
        assert_eq!(structural_line("value = \"(((\"  # (((").trim(), "value =");
        assert_eq!(delimiter_delta("([{}])"), 0);
        assert_eq!(
            statement_spans(&["value = \"(((\"", "", "other = 1"]),
            [(1, 1), (3, 3)]
        );
    }

    /// A statement left open at the end of a file still occupies its lines.
    #[test]
    fn an_unclosed_statement_runs_to_the_end_of_the_file() {
        assert_eq!(statement_spans(&["value = (", "    1,"]), [(1, 2)]);
    }

    /// Both quote characters read, which is what the back-reference did.
    #[test]
    fn a_quoted_literal_is_read_under_either_quote_character() {
        assert!(references(Some("python"), "os.getenv(\"HOME\")").contains("HOME"));
        assert!(references(Some("python"), "os.getenv('HOME')").contains("HOME"));
    }

    /// Every idiom the predecessor counts as a reference is counted here.
    #[test]
    fn a_reference_is_read_from_access_declaration_context_and_watchlist() {
        assert!(references(Some("lua"), "os.getenv('LUA_PATH')").contains("LUA_PATH"));
        assert!(references(None, "READY_ENV = \"READY\"").contains("READY"));
        let contextual = references(None, "environ.update({\"SPICE_HOOKS\": \"1\"})");
        assert!(contextual.contains("SPICE_HOOKS"));
        assert_eq!(
            references(Some("javascript"), "const { ONE, TWO: two, ...rest } = process.env;"),
            BTreeSet::from(["ONE".to_owned(), "TWO".to_owned()])
        );
        assert!(
            references(None, "let quoted = \"PLAIN\";").is_empty(),
            "a quoted literal on an ordinary line is not an environment name"
        );
    }

    /// A ledger name is a shape, and a trailing underscore is not that shape.
    #[test]
    fn a_ledger_name_opens_on_a_letter_and_never_closes_on_an_underscore() {
        assert!(is_ledger_name("ALPHA_HOME"));
        assert!(is_ledger_name("_PRIVATE"));
        assert!(!is_ledger_name("TRAILING_"));
        assert!(!is_ledger_name("lower"));
        assert!(!is_ledger_name(""));
    }
}
