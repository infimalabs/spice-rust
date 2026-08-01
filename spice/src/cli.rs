//! The generated command tree and its closed contract registry.

use std::collections::BTreeSet;

use clap::{Arg, ArgAction, ArgMatches, Command, builder::PossibleValuesParser};
use wire::FactKind;

/// The renderer flag, spelled once so its registration and its reader cannot
/// drift into naming two different arguments that merely agree today.
const JSON: &str = "json";

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
    command_from(CONTRACTS)
}

fn command_from(contracts: &[Contract]) -> Command {
    let mut root = Command::new("spice")
        .about("Repository-local coordination appliance")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .disable_help_subcommand(true)
        .arg(
            Arg::new(JSON)
                .long(JSON)
                .global(true)
                .action(ArgAction::SetTrue),
        );

    for child in child_names(contracts, &[]) {
        root = root.subcommand(branch(contracts, child, &[child]));
    }
    root
}

fn branch(contracts: &[Contract], name: &'static str, path: &[&'static str]) -> Command {
    let contract = contract_at(contracts, path);
    let children = child_names(contracts, path);
    let mut command = Command::new(name).disable_help_subcommand(true);

    if let Some(contract) = contract {
        command = command.override_usage(format!("spice {}", contract.syntax));
        command = register_syntax(command, contract).unwrap_or_else(|error| {
            panic!("invalid CLI contract syntax {:?}: {error}", contract.path)
        });
    } else {
        command = command
            .subcommand_required(true)
            .arg_required_else_help(true);
    }

    for child in children {
        let mut child_path = path.to_vec();
        child_path.push(child);
        command = command.subcommand(branch(contracts, child, &child_path));
    }
    command.subcommand_precedence_over_arg(true)
}

fn child_names(contracts: &[Contract], path: &[&str]) -> BTreeSet<&'static str> {
    contracts
        .iter()
        .filter(|contract| contract.path.starts_with(path) && contract.path.len() > path.len())
        .map(|contract| contract.path[path.len()])
        .collect()
}

fn contract_at<'a>(contracts: &'a [Contract], path: &[&str]) -> Option<&'a Contract> {
    contracts.iter().find(|contract| contract.path == path)
}

fn register_syntax(mut command: Command, contract: &Contract) -> Result<Command, String> {
    let path = contract.path.join(" ");
    let Some(tail) = contract.syntax.strip_prefix(path.as_str()) else {
        return Err(format!("does not begin with registered path {path:?}"));
    };
    if !tail.is_empty() && !tail.starts_with(' ') {
        return Err(format!("does not delimit registered path {path:?}"));
    }

    let tokens = syntax_tokens(tail.trim())?;
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        if token == "--" {
            let Some(value) = tokens.get(index + 1).copied() else {
                return Err("trailing -- has no command operand".to_owned());
            };
            if index + 2 != tokens.len() || is_optional(value) || value.starts_with("--") {
                return Err("-- must be followed by one final command operand".to_owned());
            }
            command = command.arg(positional(value, true, true)?);
            index += 2;
            continue;
        }

        if let Some(group) = optional_contents(token) {
            command = register_optional_group(command, group)?;
        } else if token.starts_with("--") {
            return Err(format!("required option syntax is unsupported: {token}"));
        } else {
            command = command.arg(positional(token, true, false)?);
        }
        index += 1;
    }
    Ok(command)
}

fn syntax_tokens(input: &'static str) -> Result<Vec<&'static str>, String> {
    let mut tokens = Vec::new();
    let mut depth = 0_u32;
    let mut start = None;

    for (offset, character) in input.char_indices() {
        if character.is_whitespace() && depth == 0 {
            if let Some(token_start) = start.take() {
                tokens.push(&input[token_start..offset]);
            }
            continue;
        }
        if start.is_none() {
            start = Some(offset);
        }
        match character {
            '[' => depth = depth.saturating_add(1),
            ']' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| format!("unmatched ] at byte {offset}"))?;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err("unclosed [ group".to_owned());
    }
    if let Some(token_start) = start {
        tokens.push(&input[token_start..]);
    }
    Ok(tokens)
}

fn is_optional(token: &str) -> bool {
    token.starts_with('[') && token.ends_with(']')
}

fn optional_contents(token: &'static str) -> Option<&'static str> {
    token.strip_prefix('[')?.strip_suffix(']')
}

fn register_optional_group(mut command: Command, group: &'static str) -> Result<Command, String> {
    let tokens = syntax_tokens(group)?;
    let Some(first) = tokens.first().copied() else {
        return Err("empty optional group".to_owned());
    };

    if first.starts_with("--") {
        if tokens.len() == 1 && first.contains("|--") {
            let alternatives = first.split('|').collect::<Vec<_>>();
            let names = alternatives
                .iter()
                .map(|alternative| option_name(alternative))
                .collect::<Result<Vec<_>, _>>()?;
            for (alternative, name) in alternatives.into_iter().zip(names.iter().copied()) {
                let conflicts = names.iter().copied().filter(|other| *other != name);
                command = command.arg(option(&[alternative])?.conflicts_with_all(conflicts));
            }
            return Ok(command);
        }
        return Ok(command.arg(option(&tokens)?));
    }

    for token in tokens {
        if let Some(nested) = optional_contents(token) {
            let nested_tokens = syntax_tokens(nested)?;
            if nested_tokens.len() != 1 {
                return Err(format!(
                    "nested optional group must contain one operand: {token}"
                ));
            }
            command = command.arg(positional(nested_tokens[0], false, false)?);
        } else if token.starts_with("--") {
            return Err(format!(
                "option follows positional in optional group: {group}"
            ));
        } else {
            command = command.arg(positional(token, false, false)?);
        }
    }
    Ok(command)
}

fn positional(syntax: &'static str, required: bool, trailing: bool) -> Result<Arg, String> {
    let (name, variadic) = match syntax.strip_suffix("...") {
        Some(name) => (name, true),
        None => (syntax, false),
    };
    if name.is_empty() || name.starts_with('-') {
        return Err(format!("invalid positional operand: {syntax}"));
    }

    let mut argument = Arg::new(name).value_name(name).required(required);
    if variadic || trailing {
        argument = argument.num_args(1..);
    }
    if trailing {
        argument = argument.last(true).allow_hyphen_values(true);
    }
    Ok(argument)
}

fn option(parts: &[&'static str]) -> Result<Arg, String> {
    let option_syntax = parts
        .first()
        .copied()
        .ok_or_else(|| "option group is empty".to_owned())?;
    let name = option_name(option_syntax)?;
    let mut argument = Arg::new(name).long(name);

    if let Some((_, value)) = option_syntax.split_once('=') {
        if parts.len() != 1 || value.is_empty() {
            return Err(format!("invalid equals option syntax: {option_syntax}"));
        }
        return Ok(argument
            .value_name(value)
            .num_args(1)
            .require_equals(true)
            .action(ArgAction::Set));
    }

    match parts {
        [_] => Ok(argument.action(ArgAction::SetTrue)),
        [_, value] if !value.starts_with('-') && !is_optional(value) => {
            argument = argument
                .value_name(*value)
                .num_args(1)
                .action(ArgAction::Set);
            if value.contains('|') && value.chars().any(char::is_lowercase) {
                argument = argument.value_parser(PossibleValuesParser::new(value.split('|')));
            }
            Ok(argument)
        }
        _ => Err(format!("invalid option group: {}", parts.join(" "))),
    }
}

fn option_name(option: &'static str) -> Result<&'static str, String> {
    let body = option
        .strip_prefix("--")
        .ok_or_else(|| format!("long option must begin with --: {option}"))?;
    let name = body.split_once('=').map_or(body, |pair| pair.0);
    if name.is_empty() {
        return Err("long option has no name".to_owned());
    }
    Ok(name)
}

/// Recover the contract selected by the generated parser.
pub(crate) fn selected_contract(matches: &ArgMatches) -> Option<&'static Contract> {
    let mut path = Vec::new();
    let mut cursor = matches;
    while let Some((name, child)) = cursor.subcommand() {
        path.push(name);
        cursor = child;
    }
    contract_at(CONTRACTS, &path)
}

/// Whether the caller asked for the JSON rendering.
///
/// The flag is global, so clap admits it anywhere a renderer flag can be
/// meant and reports it from the root regardless of where it was written. The
/// one place it is not a renderer flag is after a trailing `--`, where the
/// operand is the caller's own command and its flags are that command's, not
/// spice's.
pub(crate) fn json_selected(matches: &ArgMatches) -> bool {
    matches.get_flag(JSON)
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
        if !child.is_subcommand_required_set() {
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

        for argv in [
            &["spice", "doctor", "garbage"][..],
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
    /// caller who asked for a document, and that caller learns of it by
    /// failing to parse what arrived.
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
    /// The delimiter is found through the same tokeniser registration reads
    /// the row with, so a test cannot come to a different conclusion about
    /// where one leaf's words end from the one the parser was built on.
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

    /// Every command delimiter in the registry ends spice's words and begins
    /// the caller's.
    ///
    /// This walks the rows rather than naming `run`, because the property
    /// belongs to the delimiter and a second leaf that admits a child command
    /// would otherwise inherit the claim without ever being asked. It fails in
    /// the opposite direction from the placement law above: a delimiter spice
    /// reads past both silently changes its own rendering and hands the child
    /// a shorter command than the operator typed, and the child is an
    /// arbitrary tool that is as likely as spice to have a flag by that name.
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
}
