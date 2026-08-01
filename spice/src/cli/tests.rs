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
fn contract_syntax_drives_argument_registration() {
    let parser = command();
    for argv in [
        &["spice", "doctor"][..],
        &["spice", "watch", "root"],
        &["spice", "task", "add", "title", "--project", "rim.cli"],
        &["spice", "task", "depends", "TASK", "FIRST", "SECOND"],
        &["spice", "lane", "add", "--life", "Drive"],
        &["spice", "install", "--apply=digest"],
        &["spice", "run", "--", "cargo", "test", "--locked"],
        &["spice", "config", "key", "value"],
    ] {
        parser
            .clone()
            .try_get_matches_from(argv)
            .unwrap_or_else(|error| panic!("{argv:?} must parse: {error}"));
    }

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
