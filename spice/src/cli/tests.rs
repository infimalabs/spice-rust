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
            )
            | (JournalAccess::Command, ReplyPositionLaw::SnapshotOrCommitted) => {}
            (JournalAccess::Command, ReplyPositionLaw::DirectOrCommitted) => {}
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

/// A resident contract maps only to a command kind that reads exactly when
/// the contract reads, so its client mapping cannot refuse every invocation.
#[test]
fn resident_contracts_read_exactly_when_their_command_kind_reads() {
    let mismatched = CONTRACTS
        .iter()
        .filter(|contract| contract.guards.contains(&Guard::Resident))
        .filter_map(|contract| {
            let kind = contract.battery.parse::<wire::CommandKind>().ok()?;
            (kind.is_read() != matches!(contract.journal, JournalAccess::Read))
                .then(|| contract.path.join(" "))
        })
        .collect::<Vec<_>>();
    assert!(mismatched.is_empty(), "{mismatched:?}");
}

#[test]

fn retired_fold_commands_have_no_parser_contract_or_landing() {
    for path in [
        &["task", "effort"][..],
        &["journal", "replay"][..],
        &["fold"][..],
    ] {
        assert!(contract_at(contracts(), path).is_none(), "{path:?}");
        assert_eq!(landing(path), Landing::Unaccounted, "{path:?}");
        let arguments = std::iter::once("spice").chain(path.iter().copied());
        assert!(
            command().try_get_matches_from(arguments).is_err(),
            "{path:?}"
        );
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
        invocation: "install [--package PACKAGE] [--apply=<digest>]",
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
        python: Some("release notes"),
        rust: &["release", "notes"],
        invocation: "release notes [--release-commit REV]",
        relationship: LocalRelationship::Replacement,
        rationale: "the Rust history fold replaces the Python first-parent release-note draft",
    },
];

#[test]
fn daemonless_local_succession_is_closed_and_explicit() {
    let python = LOCAL_SUCCESSION
        .iter()
        .filter_map(|row| row.python)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        python,
        BTreeSet::from(["lock run", "lock status", "release notes"]),
    );

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
fn review_follow_up_tracking_repeats_on_the_review_leaf() {
    // A finding has to be able to open more than one statement and wire more
    // than one existing task, so both operands accrue rather than overwrite.
    let matches = command()
        .try_get_matches_from([
            "spice",
            "task",
            "review",
            "BOARD-review01",
            "--finding",
            "the refusal has no fixture",
            "--then",
            "title=Cover the refusal | project=task.cli",
            "--then",
            "title=Cover the sentinel | project=task.board",
            "--followup",
            "BOARD-track001",
            "--followup",
            "BOARD-track002",
        ])
        .expect("declared follow-up operands parse");
    let leaf = selected_matches(&matches);

    assert_eq!(
        leaf.get_many::<String>("then")
            .expect("--then is declared on the review leaf")
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "title=Cover the refusal | project=task.cli",
            "title=Cover the sentinel | project=task.board",
        ],
        "a second --then replaced the first instead of accruing"
    );
    assert_eq!(
        leaf.get_many::<String>("followup")
            .expect("--followup is declared on the review leaf")
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["BOARD-track001", "BOARD-track002"],
        "a second --followup replaced the first instead of accruing"
    );
}

#[test]
fn task_bootstrap_status_and_authored_add_fields_are_parser_owned() {
    let list = command()
        .try_get_matches_from([
            "spice",
            "task",
            "list",
            "--project",
            "project:task.cli",
            "--limit",
            "2",
            "--status",
            "waiting",
        ])
        .expect("task list filters parse");
    let leaf = selected_matches(&list);
    assert_eq!(
        selected_contract(&list)
            .expect("list selects a contract")
            .path,
        ["task", "list"]
    );
    assert_eq!(
        leaf.get_one::<String>("project").map(String::as_str),
        Some("project:task.cli")
    );
    assert_eq!(
        leaf.get_one::<String>("status").map(String::as_str),
        Some("waiting")
    );
    let withdrawn = command()
        .try_get_matches_from(["spice", "task", "list", "--status", "deleted"])
        .expect("the board carries deletion truth, so the filter that reaches it parses");
    assert_eq!(
        selected_matches(&withdrawn)
            .get_one::<String>("status")
            .map(String::as_str),
        Some("deleted"),
        "a withdrawn row is off the default board, so naming the status is the only way back to it"
    );

    let status = command()
        .try_get_matches_from(["spice", "task", "status"])
        .expect("task status parses");
    assert_eq!(
        selected_contract(&status)
            .expect("status selects a contract")
            .path,
        ["task", "status"]
    );

    let show = command()
        .try_get_matches_from(["spice", "task", "show", "TASK-1k4Q5gJw"])
        .expect("task show parses");
    assert_eq!(
        selected_contract(&show)
            .expect("show selects a contract")
            .path,
        ["task", "show"]
    );
    assert_eq!(
        selected_matches(&show)
            .get_one::<String>("HANDLE")
            .map(String::as_str),
        Some("TASK-1k4Q5gJw")
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
            "--depends",
            "TASK-1k4Q5gJw",
            "--depends",
            "TASK-1k4Q5gJx",
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
        leaf.get_many::<String>("depends")
            .expect("dependency values")
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["TASK-1k4Q5gJw", "TASK-1k4Q5gJx"]
    );
    assert_eq!(
        leaf.get_one::<String>("origin").map(String::as_str),
        Some("task:CLI-parent")
    );
}

#[test]
fn bounded_task_read_and_ledger_export_options_are_parser_owned() {
    let list = command()
        .try_get_matches_from([
            "spice",
            "task",
            "list",
            "--max-bytes",
            "4096",
            "--max-text",
            "120",
            "--after",
            "TASK-1k4Q5gJw",
        ])
        .expect("bounded task list parses");
    let leaf = selected_matches(&list);
    assert_eq!(
        leaf.get_one::<String>("max-bytes").map(String::as_str),
        Some("4096")
    );
    assert_eq!(
        leaf.get_one::<String>("after").map(String::as_str),
        Some("TASK-1k4Q5gJw")
    );

    let show = command()
        .try_get_matches_from([
            "spice",
            "task",
            "show",
            "TASK-1k4Q5gJw",
            "--context",
            "--history",
            "--limit",
            "5",
            "--max-bytes",
            "4096",
            "--max-text",
            "512",
            "--after",
            "42",
        ])
        .expect("bounded task show parses");
    let leaf = selected_matches(&show);
    assert!(leaf.get_flag("context"));
    assert!(leaf.get_flag("history"));
    assert_eq!(
        leaf.get_one::<String>("after").map(String::as_str),
        Some("42")
    );

    let ledger = command()
        .try_get_matches_from([
            "spice",
            "doc",
            "ledger",
            "TASK-1k4Q5gJw",
            "--output",
            "ledger.md",
        ])
        .expect("exact ledger export parses");
    assert_eq!(
        selected_matches(&ledger)
            .get_one::<String>("output")
            .map(String::as_str),
        Some("ledger.md")
    );
}

#[test]
fn task_creation_repair_origins_are_parser_owned() {
    for argv in [
        vec![
            "spice",
            "task",
            "capture",
            "--project",
            "task.cli",
            "--origin",
            "ack:1kLGp2NJ",
        ],
        vec![
            "spice",
            "task",
            "oops",
            "Unexpected origin loss",
            "--origin",
            "ack:1kLGp2NJ",
        ],
        vec![
            "spice",
            "taste",
            "add",
            "Recurring evidence",
            "--origin",
            "ack:1kLGp2NJ",
        ],
        vec![
            "spice",
            "taste",
            "drop",
            "fallbacks",
            "--origin",
            "ack:1kLGp2NJ",
        ],
    ] {
        let parsed = command()
            .try_get_matches_from(argv)
            .expect("creation repair origin parses");
        assert_eq!(
            selected_matches(&parsed)
                .get_one::<String>("origin")
                .map(String::as_str),
            Some("ack:1kLGp2NJ")
        );
    }
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
fn reference_transaction_contract_names_early_conditional_git() {
    let contract = contract_at(CONTRACTS, &["gate", "reference-transaction"])
        .expect("reference-transaction has one registered contract");
    assert_eq!(
        contract.syntax,
        "gate reference-transaction preparing|prepared|committed|aborted"
    );
    assert_eq!(contract.principal, Principal::Local);
    assert_eq!(contract.journal, JournalAccess::None);
    assert_eq!(contract.reply_position, ReplyPositionLaw::Direct);
    assert!(contract.facts.is_empty());
    assert_eq!(contract.guards, &[Guard::Daemonless, Guard::Toolful]);
    assert_eq!(contract.result, ResultClass::Reply);
    assert_eq!(contract.outcomes, ExitOutcome::ALL);
    assert_eq!(contract.battery, "gate::reference_transaction");

    let [git] = contract.tools else {
        panic!("reference-transaction must declare exactly one Git tool")
    };
    assert_eq!(git.name, "git");
    assert_eq!(git.owner, "gate::reference_transaction");
    assert_eq!(git.mode, ToolMode::Launch);
    assert_eq!(git.requirement, ToolRequirement::Conditional);
    assert_eq!(git.source, ToolSource::ReferenceTransactionContract);
    assert_eq!(git.bounds, ToolBounds::ReferenceTransaction);
    assert_eq!(git.cancellation, ToolCancellation::ChildOwned);
    assert_eq!(git.network, ToolNetwork::Permitted);

    for state in ["preparing", "prepared", "committed", "aborted"] {
        command()
            .try_get_matches_from(["spice", "gate", "reference-transaction", state])
            .unwrap_or_else(|error| panic!("registered state {state:?} refused: {error}"));
    }
    assert!(
        command()
            .try_get_matches_from(["spice", "gate", "reference-transaction", "unknown"])
            .is_err()
    );
    let mut gate = command()
        .find_subcommand("gate")
        .expect("gate branch is generated")
        .clone();
    let help = gate.render_help().to_string();
    assert!(help.contains("reference-transaction"), "{help}");
}

#[test]
fn task_done_contract_admits_durable_handoff_before_terminal_commit() {
    let contract =
        contract_at(CONTRACTS, &["task", "done"]).expect("task done has one registered contract");
    assert_eq!(contract.journal, JournalAccess::Command);
    assert_eq!(contract.reply_position, ReplyPositionLaw::DirectOrCommitted);

    let operation = "018f82d4-6f59-7d06-92e3-bce913116302";
    let parsed = command()
        .try_get_matches_from([
            "spice",
            "task",
            "done",
            "TASK-1k4Q5gJw",
            "--validation",
            "focused checks passed",
            "--operation",
            operation,
        ])
        .expect("terminal completion retry parses");
    let leaf = selected_matches(&parsed);
    assert_eq!(
        leaf.get_one::<String>("HANDLE").map(String::as_str),
        Some("TASK-1k4Q5gJw")
    );
    assert_eq!(
        leaf.get_one::<String>("operation").map(String::as_str),
        Some(operation)
    );

    command()
        .try_get_matches_from(["spice", "task", "done", "TASK-1k4Q5gJw"])
        .expect("default claim operation replay parses");
}

#[test]
fn release_notes_exposes_one_read_only_history_target() {
    let contract = contract_at(CONTRACTS, &["release", "notes"])
        .expect("release notes has one registered contract");
    assert_eq!(
        contract.journal,
        JournalAccess::None,
        "release notes opened a journal instead of reading Git history",
    );
    assert!(
        contract.facts.is_empty(),
        "release notes read a hidden fact instead of Git history: {:?}",
        contract.facts,
    );
    assert!(
        contract.guards.contains(&Guard::Offline),
        "release notes left the offline guard for read-only local history",
    );

    let parser = command();
    let admitted = parser
        .clone()
        .try_get_matches_from([
            "spice",
            "release",
            "notes",
            "--release-commit",
            "release-head",
        ])
        .expect("one explicit release target parses");
    assert_eq!(
        selected_matches(&admitted)
            .get_one::<String>("release-commit")
            .map(String::as_str),
        Some("release-head"),
    );
    parser
        .clone()
        .try_get_matches_from(["spice", "release", "notes"])
        .expect("HEAD is the default release target");
    assert!(
        parser
            .clone()
            .try_get_matches_from(["spice", "release", "notes", "FIRST"])
            .is_err(),
        "a positional evidence file must not survive the history-fold replacement",
    );
}

#[test]
fn contract_syntax_drives_argument_registration() {
    let parser = command();
    for argv in [
        &["spice", "doctor"][..],
        &["spice", "watch", "root"],
        &["spice", "task", "add", "title", "--project", "rim.cli"],
        &[
            "spice",
            "task",
            "add",
            "title",
            "--requires-journal-schema",
            "16",
        ],
        &["spice", "task", "require", "TASK", "--journal-schema", "16"],
        &["spice", "task", "require", "TASK", "--clear"],
        &["spice", "task", "depends", "TASK", "FIRST", "SECOND"],
        &["spice", "lane", "add", "--life", "Drive"],
        &[
            "spice",
            "lane",
            "add",
            "--team",
            "018f82d4-6f59-7d06-92e3-bce913116302",
        ],
        &["spice", "install", "--apply=digest"],
        &["spice", "install", "--package", "/tmp/spice-package"],
        &["spice", "install", "symbols"],
        &[
            "spice",
            "install",
            "symbols",
            "--sample",
            "/tmp/spice-sample.txt",
        ],
        &["spice", "restore", "verify", "bundle"],
        &["spice", "run", "--", "cargo", "test", "--locked"],
        &["spice", "task", "recover", "TASK-1k4Q5gJw"],
        &["spice", "task", "recover", "--restore"],
        &["spice", "task", "recover"],
        &[
            "spice",
            "task",
            "add",
            "title",
            "--project",
            "task.recovery",
            "--recover",
        ],
        &["spice", "config", "key", "value"],
        &["spice", "config", "key", "value", "--apply=digest"],
    ] {
        parser
            .clone()
            .try_get_matches_from(argv)
            .unwrap_or_else(|error| panic!("{argv:?} must parse: {error}"));
    }
    assert!(
        parser
            .clone()
            .try_get_matches_from([
                "spice",
                "lane",
                "add",
                "--resume",
                "019fe3e7-c53f-7422-a6eb-a73a51879e40",
            ])
            .is_err(),
        "lane add --resume must not parse after lane add became vacant-only"
    );

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
fn a_bare_appliance_after_is_present_without_consuming_a_following_flag() {
    let parser = command();
    let appliance = |argv: &[&str]| {
        let matches = parser
            .clone()
            .try_get_matches_from(argv)
            .unwrap_or_else(|error| panic!("{argv:?} must parse: {error}"));
        let require = matches
            .subcommand_matches("task")
            .and_then(|task| task.subcommand_matches("require"))
            .expect("task require matches");
        (
            require.contains_id("appliance-after"),
            require.get_one::<String>("appliance-after").cloned(),
            require.get_flag("clear"),
        )
    };
    let require = ["spice", "task", "require", "TASK"];
    assert_eq!(appliance(&require), (false, None, false));
    assert_eq!(
        appliance(&[&require[..], &["--appliance-after"]].concat()),
        (true, None, false),
        "the client then names the version the resident runs"
    );
    assert_eq!(
        appliance(&[&require[..], &["--appliance-after", "0.2.0"]].concat()),
        (true, Some("0.2.0".to_owned()), false)
    );
    assert_eq!(
        appliance(&[&require[..], &["--appliance-after", "--clear"]].concat()),
        (true, None, true)
    );
}

#[test]
fn task_trust_grant_requirement_flags_are_registered() {
    let parser = command();
    let digest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    for argv in [
        vec![
            "spice",
            "task",
            "add",
            "title",
            "--requires-trust-grant",
            digest,
        ],
        vec!["spice", "task", "require", "TASK", "--trust-grant", digest],
    ] {
        parser
            .clone()
            .try_get_matches_from(argv)
            .expect("trust requirement flag parses");
    }
}

mod late;
