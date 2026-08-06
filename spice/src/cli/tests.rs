use super::*;

#[derive(Debug, Default, Eq, PartialEq)]
struct ClosureGaps {
    commands_without_contracts: BTreeSet<String>,
    contracts_without_commands: BTreeSet<String>,
}

fn closure_gaps(command: &Command, contracts: &[Contract]) -> ClosureGaps {
    let mut command_paths = BTreeSet::new();
    collect_command_paths(command, &mut Vec::new(), &mut command_paths);
    let contract_paths = contracts
        .iter()
        .map(|contract| contract.path.join(" "))
        .collect::<BTreeSet<_>>();

    ClosureGaps {
        commands_without_contracts: command_paths.difference(&contract_paths).cloned().collect(),
        contracts_without_commands: contract_paths.difference(&command_paths).cloned().collect(),
    }
}

fn collect_command_paths(command: &Command, path: &mut Vec<String>, leaves: &mut BTreeSet<String>) {
    for child in command.get_subcommands() {
        path.push(child.get_name().to_owned());
        if !child.is_subcommand_required_set() {
            leaves.insert(path.join(" "));
        }
        collect_command_paths(child, path, leaves);
        path.pop();
    }
}

#[test]
fn generated_tree_and_contract_rows_are_bijective() {
    assert_eq!(closure_gaps(&command(), CONTRACTS), ClosureGaps::default());

    let mut paths = BTreeSet::new();
    for contract in CONTRACTS {
        assert!(
            paths.insert(contract.path),
            "duplicate path: {:?}",
            contract.path
        );
        assert!(!contract.path.is_empty());
        assert!(!contract.syntax.trim().is_empty());
        assert!(!contract.battery.trim().is_empty());
        assert!(
            contract
                .syntax
                .starts_with(contract.path.join(" ").as_str()),
            "syntax does not begin with its registered path: {contract:?}",
        );
        assert_eq!(
            contract
                .outcomes
                .iter()
                .copied()
                .map(ExitOutcome::code)
                .collect::<Vec<_>>(),
            vec![0, 2, 1],
        );
        assert!(
            contract
                .facts
                .iter()
                .all(|kind| FactKind::ALL.contains(kind))
        );
        assert_eq!(
            contract.facts.iter().collect::<BTreeSet<_>>().len(),
            contract.facts.len(),
            "fact kind repeated in {contract:?}",
        );
        if contract.journal == JournalAccess::Command {
            assert!(
                contract.guards.contains(&Guard::Resident),
                "command journal access has no resident guard: {contract:?}",
            );
        }
        match (contract.journal, contract.reply_position) {
            (JournalAccess::None | JournalAccess::Inspect, ReplyPositionLaw::Direct) => {
                assert!(
                    contract.guards.contains(&Guard::Daemonless),
                    "a direct reply must belong to a daemonless leaf: {contract:?}",
                )
            }
            (JournalAccess::Read, ReplyPositionLaw::Snapshot)
            | (JournalAccess::Recovery, ReplyPositionLaw::Snapshot)
            | (
                JournalAccess::Command | JournalAccess::Recovery | JournalAccess::Resident,
                ReplyPositionLaw::Committed,
            ) => {}
            _ => panic!("reply position does not match the leaf's journal authority: {contract:?}"),
        }
        if contract.result == ResultClass::Plan {
            assert!(
                contract.guards.contains(&Guard::DigestOnMutation),
                "plan has no digest-bound mutation guard: {contract:?}",
            );
        }
        let _ = (contract.principal,);
    }
}

#[test]
fn every_contract_has_an_executable_landing_disposition() {
    assert_eq!(contracts().len(), 88);

    let unaccounted = contracts()
        .iter()
        .filter(|contract| landing(contract.path) == Landing::Unaccounted)
        .map(|contract| contract.path.join(" "))
        .collect::<Vec<_>>();
    assert_eq!(unaccounted, Vec::<String>::new());

    let landed = contracts()
        .iter()
        .filter(|contract| landing(contract.path) == Landing::Landed)
        .count();
    assert_eq!(landed, 88);
    assert_eq!(landing(&["demo"]), Landing::Landed);
    assert_eq!(landing(&["install"]), Landing::Landed);
    assert_eq!(landing(&["lock"]), Landing::Landed);
    assert_eq!(landing(&["lock", "run"]), Landing::Landed);
    assert_eq!(landing(&["config", "migrate-tasks"]), Landing::Landed);
    assert_eq!(landing(&["release", "publish"]), Landing::Landed);
    assert_eq!(landing(&["restore", "verify"]), Landing::Landed);
    assert_eq!(landing(&["lane"]), Landing::Landed);
    assert_eq!(landing(&["lane", "add"]), Landing::Landed);
    assert_eq!(landing(&["lane", "drain"]), Landing::Landed);
    assert_eq!(landing(&["lane", "wake"]), Landing::Landed);
    assert_eq!(landing(&["lane", "import"]), Landing::Landed);
    assert_eq!(landing(&["steer", "recover"]), Landing::Landed);
    assert_eq!(landing(&["task", "reclaim"]), Landing::Landed);
    assert_eq!(landing(&["task", "status"]), Landing::Landed);
    assert_eq!(landing(&["recall"]), Landing::Landed);
    assert_eq!(landing(&["steer"]), Landing::Landed);
    assert_eq!(landing(&["steer", "cancel"]), Landing::Landed);
    assert_eq!(landing(&["inbox"]), Landing::Landed);
    assert_eq!(landing(&["ack"]), Landing::Landed);
    assert_eq!(landing(&["consent"]), Landing::Landed);
    assert_eq!(landing(&["consent", "allow"]), Landing::Landed);
    assert_eq!(landing(&["consent", "deny"]), Landing::Landed);
    assert_eq!(landing(&["consent", "policy"]), Landing::Landed);
    assert_eq!(landing(&["trust"]), Landing::Landed);
    assert_eq!(landing(&["trust", "grant"]), Landing::Landed);
    assert_eq!(landing(&["trust", "revoke"]), Landing::Landed);
    assert_eq!(landing(&["taste"]), Landing::Landed);
    assert_eq!(landing(&["taste", "add"]), Landing::Landed);
    assert_eq!(landing(&["taste", "drop"]), Landing::Landed);
    for path in [
        &["backup"][..],
        &["restore"][..],
        &["journal"][..],
        &["journal", "replay"][..],
        &["journal", "reindex"][..],
        &["journal", "verify"][..],
        &["fold"][..],
        &["thread"][..],
    ] {
        assert_eq!(landing(path), Landing::Landed, "{path:?}");
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum LocalRelationship {
    Replacement,
    Addition,
}

#[derive(Clone, Copy, Debug)]
struct LocalSuccession {
    python: Option<&'static str>,
    rust: &'static [&'static str],
    invocation: &'static str,
    relationship: LocalRelationship,
    rationale: &'static str,
}

const LOCAL_SUCCESSION: &[LocalSuccession] = &[
    LocalSuccession {
        python: None,
        rust: &["install"],
        invocation: "install [--apply=<digest>]",
        relationship: LocalRelationship::Addition,
        rationale: "Spice publishes its one repository-local appliance and generated assets",
    },
    LocalSuccession {
        python: None,
        rust: &["install"],
        invocation: "install --unapply",
        relationship: LocalRelationship::Addition,
        rationale: "Spice reverses only the exact repository-local install receipt",
    },
    LocalSuccession {
        python: Some("lock status"),
        rust: &["lock"],
        invocation: "lock",
        relationship: LocalRelationship::Replacement,
        rationale: concat!(
            "the daemonless Rust report replaces Python lock status over ",
            "Rust-owned lock state",
        ),
    },
    LocalSuccession {
        python: Some("lock run"),
        rust: &["lock", "run"],
        invocation: "lock run NAME -- CMD...",
        relationship: LocalRelationship::Replacement,
        rationale: concat!(
            "the daemonless Rust child runner preserves lock refusal, exit, and ",
            "signal contracts",
        ),
    },
    LocalSuccession {
        python: None,
        rust: &["restore", "verify"],
        invocation: "restore verify INPUT",
        relationship: LocalRelationship::Addition,
        rationale: "strict offline archive verification is an intentional native-only addition",
    },
    LocalSuccession {
        python: None,
        rust: &["release", "notes"],
        invocation: "release notes INPUT",
        relationship: LocalRelationship::Addition,
        rationale: "strict offline release-note rendering is an intentional native-only addition",
    },
];

#[test]
fn daemonless_local_succession_is_closed_and_explicit() {
    let python = LOCAL_SUCCESSION
        .iter()
        .filter_map(|row| row.python)
        .collect::<BTreeSet<_>>();
    assert_eq!(python, BTreeSet::from(["lock run", "lock status"]),);

    let rust = LOCAL_SUCCESSION
        .iter()
        .map(|row| row.rust.join(" "))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        rust,
        BTreeSet::from([
            "install".to_owned(),
            "lock".to_owned(),
            "lock run".to_owned(),
            "release notes".to_owned(),
            "restore verify".to_owned(),
        ]),
    );
    assert_eq!(
        LOCAL_SUCCESSION
            .iter()
            .map(|row| row.relationship)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([LocalRelationship::Replacement, LocalRelationship::Addition,]),
    );

    for row in LOCAL_SUCCESSION {
        assert!(!row.invocation.trim().is_empty());
        assert!(!row.rationale.trim().is_empty());
        assert!(
            contract_at(contracts(), row.rust).is_some(),
            "local mapping names an unknown Rust leaf: {:?}",
            row.rust,
        );
        assert_eq!(
            landing(row.rust),
            Landing::Landed,
            "local mapping names an unlanded Rust leaf: {:?}",
            row.rust,
        );
    }
}

#[test]
fn closure_check_names_both_directions_of_drift() {
    let parser_with_orphan = command().subcommand(Command::new("orphan"));
    let missing_contract = closure_gaps(&parser_with_orphan, CONTRACTS);
    assert_eq!(
        missing_contract.commands_without_contracts,
        BTreeSet::from(["orphan".to_owned()])
    );

    let orphan_contract = Contract {
        path: &["missing"],
        syntax: "missing",
        principal: Principal::Local,
        journal: JournalAccess::None,
        reply_position: ReplyPositionLaw::Direct,
        facts: &[],
        guards: &[],
        tools: NO_TOOLS,
        result: ResultClass::Reply,
        outcomes: ExitOutcome::ALL,
        battery: "fixture::missing",
    };
    let mut contracts = CONTRACTS.to_vec();
    contracts.push(orphan_contract);
    let missing_command = closure_gaps(&command(), &contracts);
    assert_eq!(
        missing_command.contracts_without_commands,
        BTreeSet::from(["missing".to_owned()])
    );
}

#[test]
fn review_finding_and_note_are_declared_on_the_only_review_leaf() {
    let matches = command()
        .try_get_matches_from([
            "spice",
            "task",
            "review",
            "BOARD-review01",
            "--finding",
            "missing negative-path evidence",
            "--note",
            "add a refusal fixture",
        ])
        .expect("declared review operands parse");
    let contract = selected_contract(&matches).expect("review selects a contract");
    let leaf = selected_matches(&matches);

    assert_eq!(contract.path, ["task", "review"]);
    assert_eq!(
        leaf.get_one::<String>("finding").map(String::as_str),
        Some("missing negative-path evidence")
    );
    assert_eq!(
        leaf.get_one::<String>("note").map(String::as_str),
        Some("add a refusal fixture")
    );
}

#[test]
fn task_bootstrap_status_and_authored_add_fields_are_parser_owned() {
    let status = command()
        .try_get_matches_from(["spice", "task", "status"])
        .expect("task status parses");
    assert_eq!(
        selected_contract(&status)
            .expect("status selects a contract")
            .path,
        ["task", "status"]
    );

    let add = command()
        .try_get_matches_from([
            "spice",
            "task",
            "add",
            "Restore the mosaic",
            "--project",
            "task.cli",
            "--description",
            "Carry the operator context",
            "--acceptance",
            "Status reports counts",
            "--acceptance",
            "Origin survives creation",
            "--origin",
            "task:CLI-parent",
        ])
        .expect("authored task fields parse");
    let leaf = selected_matches(&add);
    assert_eq!(
        leaf.get_one::<String>("description").map(String::as_str),
        Some("Carry the operator context")
    );
    assert_eq!(
        leaf.get_many::<String>("acceptance")
            .expect("acceptance values")
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["Status reports counts", "Origin survives creation"]
    );
    assert_eq!(
        leaf.get_one::<String>("origin").map(String::as_str),
        Some("task:CLI-parent")
    );
}

#[test]
fn tool_postures_follow_the_closed_command_order() {
    let declared = contracts()
        .iter()
        .filter(|contract| !contract.tools.is_empty())
        .map(|contract| {
            (
                contract.path.join(" "),
                contract
                    .tools
                    .iter()
                    .map(|tool| tool.name)
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        declared,
        vec![
            ("doctor".to_owned(), vec!["rtk"]),
            ("lock run".to_owned(), vec!["CMD"]),
            ("dev pre-commit".to_owned(), vec!["gate policy executable"],),
            ("gate".to_owned(), vec!["gate policy executable"]),
            ("gate camp".to_owned(), vec!["gate policy executable"]),
            ("gate kill".to_owned(), vec!["gate mutation executable"]),
            ("gate receipt".to_owned(), vec!["gate policy executable"],),
            (
                "proof".to_owned(),
                vec!["OCI runtime", "sealed battery member"],
            ),
            (
                "proof replay".to_owned(),
                vec!["OCI runtime", "sealed battery member"],
            ),
            ("task next".to_owned(), vec!["git"]),
            ("task capture".to_owned(), vec!["git"]),
            ("task done".to_owned(), vec!["git"]),
            ("task review".to_owned(), vec!["git"]),
            ("run".to_owned(), vec!["CMD"]),
        ]
    );
    assert!(posture_violations(contracts()).is_empty());
}

#[test]
fn malformed_tool_posture_mutations_are_refused() {
    let row =
        |path: &[&str]| *contract_at(contracts(), path).expect("fixture command has a contract");

    let mut missing = row(&["lock", "run"]);
    missing.tools = NO_TOOLS;
    assert!(
        posture_violations(&[missing])
            .iter()
            .any(|violation| violation.contains("toolful guard and launch declarations"))
    );

    let mut stray = row(&["watch"]);
    stray.tools = CALLER_COMMAND;
    assert!(
        posture_violations(&[stray])
            .iter()
            .any(|violation| violation.contains("toolful guard and launch declarations"))
    );

    const DUPLICATE: &[ExternalTool] = &[
        CALLER_COMMAND[0],
        ExternalTool {
            owner: "conflicting owner",
            ..CALLER_COMMAND[0]
        },
    ];
    let mut duplicate = row(&["lock", "run"]);
    duplicate.tools = DUPLICATE;
    assert!(
        posture_violations(&[duplicate])
            .iter()
            .any(|violation| violation.contains("declared more than once"))
    );

    const UNNAMED: &[ExternalTool] = &[ExternalTool {
        name: "",
        owner: "",
        ..CALLER_COMMAND[0]
    }];
    let mut unnamed = row(&["lock", "run"]);
    unnamed.tools = UNNAMED;
    let unnamed_violations = posture_violations(&[unnamed]);
    assert!(
        unnamed_violations
            .iter()
            .any(|violation| violation.contains("has no name"))
    );
    assert!(
        unnamed_violations
            .iter()
            .any(|violation| violation.contains("has no owner"))
    );

    const ACTIVE_OBSERVATION: &[ExternalTool] = &[ExternalTool {
        bounds: ToolBounds::GateContract,
        cancellation: ToolCancellation::Deadline,
        network: ToolNetwork::Permitted,
        ..DOCTOR_TOOLS[0]
    }];
    let mut observation = row(&["doctor"]);
    observation.tools = ACTIVE_OBSERVATION;
    assert!(
        posture_violations(&[observation])
            .iter()
            .any(|violation| violation.contains("carries execution posture"))
    );
}

#[test]
fn every_registered_leaf_renders_its_exact_usage() {
    let parser = command();
    for contract in CONTRACTS {
        let mut leaf = contract
            .path
            .iter()
            .fold(&parser, |command, name| {
                command
                    .find_subcommand(name)
                    .expect("registered path exists")
            })
            .clone();
        let help = leaf.render_help().to_string();
        assert!(
            help.contains(format!("spice {}", contract.syntax).as_str()),
            "help lost usage for {:?}: {help}",
            contract.path,
        );
    }
}

#[test]
fn release_notes_surfaces_name_only_the_canonical_explicit_input() {
    let contract = contract_at(CONTRACTS, &["release", "notes"])
        .expect("release notes has one registered contract");
    assert_eq!(contract.syntax, "release notes INPUT");

    assert_eq!(
        contract.journal,
        JournalAccess::None,
        "release notes opened a journal instead of reading its explicit input",
    );
    assert!(
        contract.facts.is_empty(),
        "release notes read a hidden fact instead of its explicit input: {:?}",
        contract.facts,
    );
    assert!(
        contract.guards.contains(&Guard::Offline),
        "release notes left the offline guard that keeps its input explicit",
    );

    // One operand carries the whole input: a Git range or a sealed manifest
    // would need a second endpoint, or none at all.
    let parser = command();
    let admitted = parser
        .clone()
        .try_get_matches_from(["spice", "release", "notes", "release-input.json"])
        .expect("one explicit input parses");
    assert_eq!(
        selected_matches(&admitted)
            .get_one::<String>("INPUT")
            .map(String::as_str),
        Some("release-input.json"),
    );
    for legacy in [
        &["spice", "release", "notes"][..],
        &["spice", "release", "notes", "FIRST", "SECOND"],
    ] {
        assert!(
            parser.clone().try_get_matches_from(legacy).is_err(),
            "{legacy:?} must be rejected: release notes takes exactly one canonical input",
        );
    }

    // The native dispatch reads that one operand as a file, proven against the
    // binary rim in `release_command::tests`; on wasm32 the same operand is the
    // only thing the terminal refusal can name.
    let wasm_dispatch = include_str!("../main.rs");
    assert!(
        wasm_dispatch
            .contains("reading the explicit release notes INPUT file is unavailable on wasm32")
    );
    assert!(!wasm_dispatch.contains("immutable Git object inspection"));
}

#[test]
fn contract_syntax_drives_argument_registration() {
    let parser = command();
    for argv in [
        &["spice", "doctor"][..],
        &["spice", "watch", "root"],
        &["spice", "task", "add", "title", "--project", "rim.cli"],
        &["spice", "task", "depends", "TASK", "FIRST", "SECOND"],
        &["spice", "lane", "add", "--life", "Drive"],
        &[
            "spice",
            "lane",
            "add",
            "--lineage",
            "018f82d4-6f59-7d06-92e3-bce913116302",
        ],
        &["spice", "install", "--apply=digest"],
        &["spice", "restore", "verify", "bundle"],
        &["spice", "run", "--", "cargo", "test", "--locked"],
        &["spice", "config", "key", "value"],
        &["spice", "config", "key", "value", "--apply=digest"],
    ] {
        parser
            .clone()
            .try_get_matches_from(argv)
            .unwrap_or_else(|error| panic!("{argv:?} must parse: {error}"));
    }

    // Adoption is a bare switch: task capture reads it with get_flag, which
    // panics outright on an option carrying a value.
    let stolen = parser
        .clone()
        .try_get_matches_from(["spice", "task", "capture", "TASK-1k4Q5gJw", "--steal"])
        .expect("task capture accepts --steal");
    let capture = |matches: &clap::ArgMatches| {
        matches
            .subcommand_matches("task")
            .and_then(|task| task.subcommand_matches("capture"))
            .expect("task capture matches")
            .get_flag("steal")
    };
    assert!(capture(&stolen));
    let plain = parser
        .clone()
        .try_get_matches_from(["spice", "task", "capture", "TASK-1k4Q5gJw"])
        .expect("task capture still accepts a bare handle");
    assert!(!capture(&plain));

    // Release one exposes no finite, test-only execution arm for watch.
    for argv in [
        &["spice", "doctor", "garbage"][..],
        &["spice", "watch", "root", "--once"],
        &["spice", "task", "add"],
        &["spice", "task", "depends", "TASK"],
        &["spice", "lane", "add", "--life", "Sprint"],
        &["spice", "install", "--apply=digest", "--unapply"],
        &["spice", "run", "cargo", "test"],
    ] {
        assert!(
            parser.clone().try_get_matches_from(argv).is_err(),
            "{argv:?} must be rejected",
        );
    }
}

#[test]
fn configuration_flag_mappings_resolve_in_the_generated_tree() {
    let parser = command();
    for mapping in crate::config::descriptors()
        .iter()
        .filter_map(|descriptor| descriptor.flag())
    {
        let contract = contract_at(CONTRACTS, mapping.command())
            .unwrap_or_else(|| panic!("missing command contract for {mapping:?}"));
        assert!(
            contract.syntax.contains(mapping.flag()),
            "contract syntax lost {mapping:?}: {:?}",
            contract.syntax
        );

        let leaf = mapping.command().iter().fold(&parser, |command, name| {
            command
                .find_subcommand(name)
                .unwrap_or_else(|| panic!("generated tree lost {mapping:?}"))
        });
        assert!(
            leaf.get_arguments()
                .any(|argument| argument.get_long() == mapping.flag().strip_prefix("--")),
            "generated parser lost {mapping:?}"
        );
    }
}

#[test]
fn mutating_contract_syntax_mutates_registration_and_help() {
    let mut contracts = CONTRACTS.to_vec();
    let doctor = contracts
        .iter_mut()
        .find(|contract| contract.path == ["doctor"])
        .expect("doctor contract exists");
    doctor.syntax = "doctor REQUIRED";

    let parser = command_from(&contracts);
    assert!(
        parser
            .clone()
            .try_get_matches_from(["spice", "doctor"])
            .is_err(),
        "a required operand added to the table must change registration",
    );
    parser
        .clone()
        .try_get_matches_from(["spice", "doctor", "value"])
        .expect("the syntax mutation admits its required operand");

    let mut doctor = parser
        .find_subcommand("doctor")
        .expect("doctor command exists")
        .clone();
    assert!(
        doctor
            .render_help()
            .to_string()
            .contains("spice doctor REQUIRED"),
        "the syntax mutation must change help",
    );
}

#[test]
fn selected_leaf_recovers_the_same_contract_row() {
    let matches = command()
        .try_get_matches_from(["spice", "doctor"])
        .expect("doctor parses");
    assert_eq!(
        selected_contract(&matches),
        contract_at(CONTRACTS, &["doctor"])
    );
}

/// The renderer flag is recognised wherever spice's own words are.
///
/// A position spice fails to recognise silently renders a sentence to a
/// caller who asked for a document, and that caller learns of it by failing to
/// parse what arrived.
#[test]
fn the_renderer_flag_is_read_wherever_spice_owns_the_word() {
    for argv in [
        &["spice", "--json", "task", "next"][..],
        &["spice", "task", "--json", "next"][..],
        &["spice", "task", "next", "--json"][..],
        &["spice", "--json", "doctor"][..],
        &["spice", "doctor", "--json"][..],
    ] {
        let matches = command()
            .try_get_matches_from(argv)
            .unwrap_or_else(|error| panic!("{argv:?} must parse: {error}"));
        assert!(json_selected(&matches), "{argv:?} asked for JSON");
    }

    let matches = command()
        .try_get_matches_from(["spice", "task", "next"])
        .expect("an unflagged leaf parses");
    assert!(!json_selected(&matches), "nobody asked for JSON");
}

/// A placeholder for each operand a contract requires before its command
/// delimiter, or nothing when it admits no child command.
///
/// The delimiter is found through the same tokeniser registration reads the
/// row with, so a test cannot come to a different conclusion about where one
/// leaf's words end from the one the parser was built on.
fn operands_before_delimiter(contract: &Contract) -> Option<Vec<&'static str>> {
    let tail = contract
        .syntax
        .strip_prefix(contract.path.join(" ").as_str())
        .expect("syntax begins with its registered path");
    let tokens = syntax_tokens(tail.trim()).expect("registered syntax tokenises");
    let delimiter = tokens.iter().position(|token| *token == "--")?;
    Some(
        tokens[..delimiter]
            .iter()
            .filter(|token| !is_optional(token))
            .map(|_| "OPERAND")
            .collect(),
    )
}

/// Every command delimiter in the registry ends spice's words and begins the
/// caller's.
///
/// This walks the rows rather than naming `run`, because the property belongs
/// to the delimiter and a second leaf that admits a child command would
/// otherwise inherit the claim without ever being asked. It fails in the
/// opposite direction from the placement law above: a delimiter spice reads
/// past both silently changes its own rendering and hands the child a shorter
/// command than the operator typed, and the child is an arbitrary tool that is
/// as likely as spice to have a flag by that name.
#[test]
fn a_child_commands_words_begin_at_the_delimiter() {
    let delimited = CONTRACTS
        .iter()
        .filter_map(|contract| Some((contract, operands_before_delimiter(contract)?)))
        .collect::<Vec<_>>();
    assert!(
        !delimited.is_empty(),
        "the registry admits at least one child command",
    );

    for (contract, operands) in delimited {
        let mut argv = vec!["spice"];
        argv.extend(contract.path.iter().copied());
        argv.extend(operands);

        let mut renderer = argv.clone();
        renderer.extend(["--json", "--", "echo"]);
        let matches = command()
            .try_get_matches_from(&renderer)
            .unwrap_or_else(|error| panic!("{renderer:?} must parse: {error}"));
        assert!(
            json_selected(&matches),
            "{renderer:?} asked for JSON before the delimiter",
        );

        let mut child = argv;
        child.extend(["--", "echo", "--json"]);
        let matches = command()
            .try_get_matches_from(&child)
            .unwrap_or_else(|error| panic!("{child:?} must parse: {error}"));
        assert!(
            !json_selected(&matches),
            "{child:?} spelled the child's flag, not the renderer's",
        );
    }
}
