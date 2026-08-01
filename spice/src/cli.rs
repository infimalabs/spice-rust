//! The generated command tree and its closed contract registry.

use std::collections::BTreeSet;

use clap::{Arg, ArgAction, ArgMatches, Command};
use wire::FactKind;

const RAW_ARGUMENTS: &str = "contract-arguments";

/// Principal admitted to a command leaf.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Principal {
    Local,
    Operator,
    Lane,
    OperatorOrLane,
}

/// Relationship a leaf has with the authoritative journal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Authority {
    None,
    Read,
    Command,
    Recovery,
    Resident,
}

/// Additional posture or admission rule carried by a leaf.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Guard {
    Daemonless,
    Offline,
    Toolful,
    Resident,
    OperatorCapability,
    LaneCapability,
    DigestOnMutation,
    JournalLock,
}

/// Shape returned when a leaf succeeds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResultClass {
    Reply,
    Plan,
    Stream,
}

/// Public process outcomes shared by every command renderer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExitOutcome {
    Reply,
    Refusal,
    Fault,
}

impl ExitOutcome {
    pub(crate) const ALL: &[Self] = &[Self::Reply, Self::Refusal, Self::Fault];

    #[cfg(test)]
    const fn code(self) -> i32 {
        match self {
            Self::Reply => 0,
            Self::Refusal => 2,
            Self::Fault => 1,
        }
    }
}

/// One generated CLI contract row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Contract {
    pub(crate) path: &'static [&'static str],
    pub(crate) syntax: &'static str,
    pub(crate) principal: Principal,
    pub(crate) authority: Authority,
    pub(crate) facts: &'static [FactKind],
    pub(crate) guards: &'static [Guard],
    pub(crate) result: ResultClass,
    pub(crate) outcomes: &'static [ExitOutcome],
    pub(crate) battery: &'static str,
}

macro_rules! row {
    (
        [$($path:literal),+], $syntax:literal, $principal:ident, $authority:ident,
        [$($fact:ident),*], [$($guard:ident),*], $result:ident, $battery:literal
    ) => {
        Contract {
            path: &[$($path),+],
            syntax: $syntax,
            principal: Principal::$principal,
            authority: Authority::$authority,
            facts: &[$(FactKind::$fact),*],
            guards: &[$(Guard::$guard),*],
            result: ResultClass::$result,
            outcomes: ExitOutcome::ALL,
            battery: $battery,
        }
    };
}

/// Every current leaf. Parser registration and help are generated from these rows.
pub(crate) const CONTRACTS: &[Contract] = &[
    row!(
        ["watch"],
        "watch [ROOT]",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Stream,
        "listen::watch"
    ),
    row!(
        ["retro"],
        "retro [LANE|LOCUS|PATH]",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Stream,
        "listen::retro"
    ),
    row!(
        ["doctor"],
        "doctor",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "spice::doctor"
    ),
    row!(
        ["version"],
        "version",
        Local,
        Read,
        [],
        [Daemonless, Offline],
        Reply,
        "spice::version"
    ),
    row!(
        ["install"],
        "install [--apply=<digest>|--unapply]",
        Local,
        None,
        [],
        [Daemonless, Offline, DigestOnMutation],
        Plan,
        "spice::install"
    ),
    row!(
        ["lock"],
        "lock",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "sill::lock"
    ),
    row!(
        ["lock", "run"],
        "lock run NAME [--shard N] -- CMD",
        Local,
        None,
        [],
        [Daemonless, Toolful],
        Stream,
        "sill::lock_run"
    ),
    row!(
        ["restore", "verify"],
        "restore verify BUNDLE",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "journal::restore_verify"
    ),
    row!(
        ["gate"],
        "gate [--at REV]",
        Local,
        None,
        [],
        [Daemonless, Toolful],
        Reply,
        "gate::run"
    ),
    row!(
        ["gate", "camp"],
        "gate camp",
        Local,
        None,
        [],
        [Daemonless, Toolful],
        Reply,
        "gate::camp"
    ),
    row!(
        ["gate", "kill"],
        "gate kill",
        Local,
        None,
        [],
        [Daemonless, Toolful],
        Reply,
        "gate::kill"
    ),
    row!(
        ["gate", "receipt"],
        "gate receipt",
        Local,
        None,
        [],
        [Daemonless, Toolful],
        Reply,
        "gate::receipt"
    ),
    row!(
        ["proof"],
        "proof",
        Local,
        None,
        [],
        [Daemonless, Offline, Toolful],
        Reply,
        "proof::seal"
    ),
    row!(
        ["proof", "replay"],
        "proof replay",
        Local,
        None,
        [],
        [Daemonless, Offline, Toolful],
        Reply,
        "proof::replay"
    ),
    row!(
        ["release", "notes"],
        "release notes",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "dist::release_notes"
    ),
    row!(
        ["up"],
        "up",
        Local,
        Resident,
        [JournalMigrated, TrustPended, UpSeated],
        [JournalLock],
        Stream,
        "spice::up"
    ),
    row!(
        ["down"],
        "down",
        Operator,
        Command,
        [
            UpQuiescing,
            UpDraining,
            UpTerminating,
            UpKilling,
            UpVerifying,
            UpDark
        ],
        [Resident, OperatorCapability],
        Reply,
        "spice::down"
    ),
    row!(
        ["board"],
        "board [FAMILY]",
        OperatorOrLane,
        Read,
        [],
        [Resident],
        Reply,
        "board::read"
    ),
    row!(
        ["board", "graph"],
        "board graph VIEW [--at BORN]",
        OperatorOrLane,
        Read,
        [],
        [Resident],
        Reply,
        "board::graph"
    ),
    row!(
        ["board", "handout"],
        "board handout OUTPUT [--at BORN]",
        Operator,
        Read,
        [],
        [Resident, OperatorCapability],
        Reply,
        "board::handout"
    ),
    row!(
        ["task", "next"],
        "task next",
        OperatorOrLane,
        Command,
        [ClaimWon],
        [Resident],
        Reply,
        "board::next"
    ),
    row!(
        ["task", "add"],
        "task add TITLE [--project PROJECT]",
        OperatorOrLane,
        Command,
        [TaskAdded],
        [Resident],
        Reply,
        "board::task_add"
    ),
    row!(
        ["task", "done"],
        "task done [--validation TEXT]",
        Lane,
        Command,
        [TaskDone, TaskPhased],
        [Resident, LaneCapability],
        Reply,
        "board::task_done"
    ),
    row!(
        ["task", "review"],
        "task review HANDLE",
        Lane,
        Command,
        [TaskReviewed],
        [Resident, LaneCapability],
        Reply,
        "board::task_review"
    ),
    row!(
        ["task", "note"],
        "task note HANDLE TEXT",
        OperatorOrLane,
        Command,
        [TaskNoted],
        [Resident],
        Reply,
        "board::task_note"
    ),
    row!(
        ["task", "defer"],
        "task defer HANDLE UNTIL",
        OperatorOrLane,
        Command,
        [TaskDeferred],
        [Resident],
        Reply,
        "board::task_defer"
    ),
    row!(
        ["task", "wake"],
        "task wake HANDLE",
        OperatorOrLane,
        Command,
        [TaskWoken],
        [Resident],
        Reply,
        "board::task_wake"
    ),
    row!(
        ["task", "free"],
        "task free HANDLE",
        OperatorOrLane,
        Command,
        [ClaimFreed],
        [Resident],
        Reply,
        "board::task_free"
    ),
    row!(
        ["task", "oops"],
        "task oops TEXT",
        OperatorOrLane,
        Command,
        [TaskAdded, TaskNoted],
        [Resident],
        Reply,
        "board::task_oops"
    ),
    row!(
        ["task", "depends"],
        "task depends HANDLE AFTER...",
        OperatorOrLane,
        Command,
        [TaskEdited],
        [Resident],
        Reply,
        "board::task_depends"
    ),
    row!(
        ["doc"],
        "doc [PATH|-] [--apply=<digest>]",
        OperatorOrLane,
        Command,
        [TaskAdded, TaskEdited, DocApplied],
        [Resident, DigestOnMutation],
        Plan,
        "board::doc"
    ),
    row!(
        ["doc", "ledger"],
        "doc ledger HANDLE",
        OperatorOrLane,
        Read,
        [],
        [Resident],
        Reply,
        "dialect::ledger"
    ),
    row!(
        ["lane"],
        "lane",
        Operator,
        Read,
        [],
        [Resident, OperatorCapability],
        Reply,
        "lane::read"
    ),
    row!(
        ["lane", "add"],
        "lane add [WORKTREE] [--life Steer|Drive|Drain] [--vendor NAME]",
        Operator,
        Command,
        [LaneSeated, LaneStarted],
        [Resident, OperatorCapability],
        Reply,
        "lane::add"
    ),
    row!(
        ["lane", "drain"],
        "lane drain ID",
        Operator,
        Command,
        [LaneExited, LaneDrained],
        [Resident, OperatorCapability],
        Reply,
        "lane::drain"
    ),
    row!(
        ["lane", "wake"],
        "lane wake ID",
        Operator,
        Command,
        [LaneResumed],
        [Resident, OperatorCapability],
        Reply,
        "lane::wake"
    ),
    row!(
        ["steer"],
        "steer LANE TEXT",
        Operator,
        Command,
        [SteerSent],
        [Resident, OperatorCapability],
        Reply,
        "steer::send"
    ),
    row!(
        ["steer", "cancel"],
        "steer cancel ID",
        Operator,
        Command,
        [SteerCanceled],
        [Resident, OperatorCapability],
        Reply,
        "steer::cancel"
    ),
    row!(
        ["inbox"],
        "inbox [LANE]",
        OperatorOrLane,
        Read,
        [],
        [Resident],
        Reply,
        "steer::inbox"
    ),
    row!(
        ["recall"],
        "recall [LANE]",
        OperatorOrLane,
        Read,
        [],
        [Resident],
        Reply,
        "lane::recall"
    ),
    row!(
        ["consent"],
        "consent",
        Operator,
        Read,
        [],
        [Resident, OperatorCapability],
        Reply,
        "consent::read"
    ),
    row!(
        ["consent", "allow"],
        "consent allow ID",
        Operator,
        Command,
        [ConsentGranted],
        [Resident, OperatorCapability],
        Reply,
        "consent::allow"
    ),
    row!(
        ["consent", "deny"],
        "consent deny ID",
        Operator,
        Command,
        [ConsentDenied],
        [Resident, OperatorCapability],
        Reply,
        "consent::deny"
    ),
    row!(
        ["consent", "policy"],
        "consent policy",
        Operator,
        Read,
        [],
        [Resident, OperatorCapability],
        Reply,
        "consent::policy"
    ),
    row!(
        ["taste"],
        "taste",
        Operator,
        Read,
        [],
        [Resident, OperatorCapability],
        Reply,
        "taste::read"
    ),
    row!(
        ["taste", "add"],
        "taste add TEXT",
        Operator,
        Command,
        [TaskAdded],
        [Resident, OperatorCapability],
        Reply,
        "taste::add"
    ),
    row!(
        ["taste", "drop"],
        "taste drop NAME",
        Operator,
        Command,
        [TaskAdded],
        [Resident, OperatorCapability],
        Reply,
        "taste::drop"
    ),
    row!(
        ["trust"],
        "trust",
        Operator,
        Read,
        [],
        [Resident, OperatorCapability],
        Reply,
        "spice::trust"
    ),
    row!(
        ["trust", "grant"],
        "trust grant DIGEST",
        Operator,
        Command,
        [TrustGranted],
        [Resident, OperatorCapability],
        Reply,
        "spice::trust_grant"
    ),
    row!(
        ["trust", "revoke"],
        "trust revoke DIGEST",
        Operator,
        Command,
        [TrustRevoked],
        [Resident, OperatorCapability],
        Reply,
        "spice::trust_revoke"
    ),
    row!(
        ["backup"],
        "backup OUTPUT",
        Operator,
        Recovery,
        [JournalBacked],
        [Resident, OperatorCapability, JournalLock],
        Reply,
        "journal::backup"
    ),
    row!(
        ["restore"],
        "restore BUNDLE [--apply=<digest>]",
        Operator,
        Recovery,
        [JournalRestored],
        [Resident, OperatorCapability, DigestOnMutation, JournalLock],
        Plan,
        "journal::restore"
    ),
    row!(
        ["journal"],
        "journal",
        Operator,
        Read,
        [],
        [Resident, OperatorCapability],
        Reply,
        "journal::read"
    ),
    row!(
        ["journal", "replay"],
        "journal replay [FOLD]",
        Operator,
        Read,
        [],
        [Resident, OperatorCapability],
        Reply,
        "fold::replay"
    ),
    row!(
        ["journal", "reindex"],
        "journal reindex",
        Operator,
        Recovery,
        [],
        [Resident, OperatorCapability, JournalLock],
        Reply,
        "journal::reindex"
    ),
    row!(
        ["journal", "verify"],
        "journal verify",
        Operator,
        Read,
        [],
        [Resident, OperatorCapability],
        Reply,
        "journal::verify"
    ),
    row!(
        ["fold"],
        "fold",
        Operator,
        Read,
        [],
        [Resident, OperatorCapability],
        Reply,
        "fold::read"
    ),
    row!(
        ["thread"],
        "thread REF",
        OperatorOrLane,
        Read,
        [],
        [Resident],
        Reply,
        "journal::thread"
    ),
    row!(
        ["canvas"],
        "canvas",
        Operator,
        Read,
        [],
        [Resident, OperatorCapability],
        Reply,
        "canvas::open"
    ),
    row!(
        ["release", "publish"],
        "release publish [--apply=<digest>]",
        Operator,
        Command,
        [ReleasePublished],
        [Resident, OperatorCapability, DigestOnMutation],
        Plan,
        "dist::publish"
    ),
    row!(
        ["run"],
        "run -- CMD",
        Lane,
        None,
        [],
        [Daemonless, Toolful, LaneCapability],
        Stream,
        "steer::run"
    ),
    row!(
        ["ack"],
        "ack KEY [--decline]",
        Lane,
        Command,
        [SteerAcked, SteerDeclined],
        [Resident, LaneCapability],
        Reply,
        "steer::ack"
    ),
    row!(
        ["config"],
        "config [KEY [VALUE]]",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "spice::config"
    ),
];

/// Build the one parser and help tree from [`CONTRACTS`].
pub(crate) fn command() -> Command {
    let mut root = Command::new("spice")
        .about("Repository-local coordination appliance")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .disable_help_subcommand(true)
        .arg(
            Arg::new("json")
                .long("json")
                .global(true)
                .action(ArgAction::SetTrue),
        );

    for child in child_names(&[]) {
        root = root.subcommand(branch(child, &[child]));
    }
    root
}

fn branch(name: &'static str, path: &[&'static str]) -> Command {
    let contract = contract_at(path);
    let children = child_names(path);
    let mut command = Command::new(name).disable_help_subcommand(true);

    if let Some(contract) = contract {
        command = command
            .override_usage(format!("spice {}", contract.syntax))
            .arg(
                Arg::new(RAW_ARGUMENTS)
                    .num_args(0..)
                    .allow_hyphen_values(true)
                    .trailing_var_arg(true),
            );
    } else {
        command = command
            .subcommand_required(true)
            .arg_required_else_help(true);
    }

    for child in children {
        let mut child_path = path.to_vec();
        child_path.push(child);
        command = command.subcommand(branch(child, &child_path));
    }
    command.subcommand_precedence_over_arg(true)
}

fn child_names(path: &[&str]) -> BTreeSet<&'static str> {
    CONTRACTS
        .iter()
        .filter(|contract| contract.path.starts_with(path) && contract.path.len() > path.len())
        .map(|contract| contract.path[path.len()])
        .collect()
}

fn contract_at(path: &[&str]) -> Option<&'static Contract> {
    CONTRACTS.iter().find(|contract| contract.path == path)
}

/// Recover the contract selected by the generated parser.
pub(crate) fn selected_contract(matches: &ArgMatches) -> Option<&'static Contract> {
    let mut path = Vec::new();
    let mut cursor = matches;
    while let Some((name, child)) = cursor.subcommand() {
        path.push(name);
        cursor = child;
    }
    contract_at(&path)
}

#[cfg(test)]
#[derive(Debug, Default, Eq, PartialEq)]
struct ClosureGaps {
    commands_without_contracts: BTreeSet<String>,
    contracts_without_commands: BTreeSet<String>,
}

#[cfg(test)]
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

#[cfg(test)]
fn collect_command_paths(command: &Command, path: &mut Vec<String>, leaves: &mut BTreeSet<String>) {
    for child in command.get_subcommands() {
        path.push(child.get_name().to_owned());
        if child
            .get_arguments()
            .any(|arg| arg.get_id() == RAW_ARGUMENTS)
        {
            leaves.insert(path.join(" "));
        }
        collect_command_paths(child, path, leaves);
        path.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            if contract.authority == Authority::Command {
                assert!(
                    contract.guards.contains(&Guard::Resident),
                    "command authority has no resident guard: {contract:?}",
                );
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
        let parser_with_orphan =
            command().subcommand(Command::new("orphan").arg(Arg::new(RAW_ARGUMENTS).num_args(0..)));
        let missing_contract = closure_gaps(&parser_with_orphan, CONTRACTS);
        assert_eq!(
            missing_contract.commands_without_contracts,
            BTreeSet::from(["orphan".to_owned()])
        );

        let orphan_contract = Contract {
            path: &["missing"],
            syntax: "missing",
            principal: Principal::Local,
            authority: Authority::None,
            facts: &[],
            guards: &[],
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
    fn every_registered_leaf_parses_and_renders_its_exact_usage() {
        let parser = command();
        for contract in CONTRACTS {
            let mut argv = vec!["spice"];
            argv.extend_from_slice(contract.path);
            let matches = parser
                .clone()
                .try_get_matches_from(argv)
                .unwrap_or_else(|error| panic!("{:?} parses: {error}", contract.path));
            assert_eq!(selected_contract(&matches), Some(contract));

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
}
