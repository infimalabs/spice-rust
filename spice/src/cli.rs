//! The generated command tree and its closed contract registry.

use std::collections::BTreeSet;

#[cfg(not(target_arch = "wasm32"))]
use std::ffi::OsString;

use clap::{Arg, ArgAction, ArgMatches, Command};
use wire::FactKind;

mod syntax;
mod tools;

use syntax::register_syntax;
#[cfg(test)]
use syntax::{is_optional, syntax_tokens};
pub(crate) use tools::ExternalTool;
use tools::{
    CALLER_COMMAND, DOCTOR_TOOLS, GATE_MUTATION_TOOLS, GATE_POLICY_TOOLS, NO_TOOLS, PROOF_TOOLS,
};
#[cfg(test)]
use tools::{ToolBounds, ToolCancellation, ToolNetwork, posture_violations};

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
pub(crate) enum JournalAccess {
    None,
    Inspect,
    Read,
    Command,
    Recovery,
    Resident,
}

/// Journal relationship a successful reply must report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReplyPositionLaw {
    /// The command is daemonless and has no journal position.
    Direct,
    /// The command answers from a read snapshot.
    Snapshot,
    /// The command commits authoritative state before answering.
    Committed,
}

impl JournalAccess {
    const fn reply_position(self) -> ReplyPositionLaw {
        match self {
            Self::None | Self::Inspect => ReplyPositionLaw::Direct,
            Self::Read => ReplyPositionLaw::Snapshot,
            Self::Command | Self::Recovery | Self::Resident => ReplyPositionLaw::Committed,
        }
    }
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
    pub(crate) journal: JournalAccess,
    pub(crate) reply_position: ReplyPositionLaw,
    pub(crate) facts: &'static [FactKind],
    pub(crate) guards: &'static [Guard],
    pub(crate) tools: &'static [ExternalTool],
    pub(crate) result: ResultClass,
    pub(crate) outcomes: &'static [ExitOutcome],
    pub(crate) battery: &'static str,
}

/// Whether one registered leaf has executable Rust dispatch today.
///
/// A gap names the public task that owns it. `Unaccounted` exists so a newly
/// registered leaf fails the succession check instead of quietly joining the
/// terminal release refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Landing {
    Landed,
    Gap(&'static str),
    Unaccounted,
}

macro_rules! row {
    (
        [$($path:literal),+], $syntax:literal, $principal:ident,
        $journal:ident => $position:ident,
        [$($fact:ident),*], [$($guard:ident),*], $result:ident, $battery:literal
    ) => {
        Contract {
            path: &[$($path),+],
            syntax: $syntax,
            principal: Principal::$principal,
            journal: JournalAccess::$journal,
            reply_position: ReplyPositionLaw::$position,
            facts: &[$(FactKind::$fact),*],
            guards: &[$(Guard::$guard),*],
            tools: NO_TOOLS,
            result: ResultClass::$result,
            outcomes: ExitOutcome::ALL,
            battery: $battery,
        }
    };
    (
        [$($path:literal),+], $syntax:literal, $principal:ident, $journal:ident,
        [$($fact:ident),*], [$($guard:ident),*], $result:ident, $battery:literal
    ) => {
        row!(
            [$($path),+], $syntax, $principal, $journal,
            [$($fact),*], [$($guard),*], NO_TOOLS, $result, $battery
        )
    };
    (
        [$($path:literal),+], $syntax:literal, $principal:ident, $journal:ident,
        [$($fact:ident),*], [$($guard:ident),*], $tools:expr, $result:ident, $battery:literal
    ) => {
        Contract {
            path: &[$($path),+],
            syntax: $syntax,
            principal: Principal::$principal,
            journal: JournalAccess::$journal,
            reply_position: JournalAccess::$journal.reply_position(),
            facts: &[$(FactKind::$fact),*],
            guards: &[$(Guard::$guard),*],
            tools: $tools,
            result: ResultClass::$result,
            outcomes: ExitOutcome::ALL,
            battery: $battery,
        }
    };
}

/// Every current leaf. Parser registration and help are generated from these rows.
const CONTRACTS: &[Contract] = &[
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
        DOCTOR_TOOLS,
        Reply,
        "spice::doctor"
    ),
    row!(
        ["version"],
        "version",
        Local,
        Inspect,
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
        CALLER_COMMAND,
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
        GATE_POLICY_TOOLS,
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
        GATE_POLICY_TOOLS,
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
        GATE_MUTATION_TOOLS,
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
        GATE_POLICY_TOOLS,
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
        PROOF_TOOLS,
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
        PROOF_TOOLS,
        Reply,
        "proof::replay"
    ),
    row!(
        ["release", "notes"],
        "release notes INPUT",
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
        "task review HANDLE [--finding TEXT] [--note TEXT]",
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
        [Offline, JournalLock],
        Reply,
        "journal::backup"
    ),
    row!(
        ["restore"],
        "restore BUNDLE [--apply=<digest>]",
        Operator,
        Recovery,
        [JournalRestored],
        [Offline, DigestOnMutation, JournalLock],
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
        Recovery => Snapshot,
        [],
        [Offline, JournalLock],
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
        CALLER_COMMAND,
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
        "config [KEY [VALUE]] [--apply=<digest>]",
        Local,
        None,
        [],
        [Daemonless, Offline, DigestOnMutation],
        Plan,
        "spice::config"
    ),
];

/// The complete registry in the order used by help and Doctor rendering.
///
/// Returning the static rows performs no discovery or inspection. The Doctor
/// surface adds observations later without gaining a second command list.
pub(crate) const fn contracts() -> &'static [Contract] {
    CONTRACTS
}

/// Classify executable succession independently from parser registration.
///
/// This second closed table is intentional: adding a command contract does
/// not make its implementation real. The closure test requires every current
/// leaf to appear here, while the binary terminal branch uses the same
/// classification to distinguish tracked work from a dispatch defect.
pub(crate) fn landing(path: &[&str]) -> Landing {
    match path {
        ["watch"]
        | ["retro"]
        | ["doctor"]
        | ["version"]
        | ["gate"]
        | ["gate", "camp"]
        | ["gate", "kill"]
        | ["gate", "receipt"]
        | ["install"]
        | ["lock"]
        | ["lock", "run"]
        | ["proof"]
        | ["proof", "replay"]
        | ["release", "notes"]
        | ["up"]
        | ["down"]
        | ["run"]
        | ["config"]
        | ["trust"]
        | ["trust", "grant"]
        | ["trust", "revoke"]
        | ["backup"]
        | ["restore"]
        | ["journal"]
        | ["journal", "replay"]
        | ["journal", "reindex"]
        | ["journal", "verify"]
        | ["fold"]
        | ["thread"]
        | ["canvas"]
        | ["release", "publish"]
        | ["restore", "verify"]
        | ["steer"]
        | ["steer", "cancel"]
        | ["inbox"]
        | ["ack"]
        | ["consent"]
        | ["consent", "allow"]
        | ["consent", "deny"]
        | ["consent", "policy"]
        | ["board"]
        | ["board", "graph"]
        | ["board", "handout"]
        | ["task", "next"]
        | ["task", "add"]
        | ["task", "done"]
        | ["task", "review"]
        | ["task", "note"]
        | ["task", "defer"]
        | ["task", "wake"]
        | ["task", "free"]
        | ["task", "oops"]
        | ["task", "depends"]
        | ["doc"]
        | ["doc", "ledger"] => Landing::Landed,
        ["lane"] | ["lane", "add"] | ["lane", "drain"] | ["lane", "wake"] | ["recall"] => {
            Landing::Gap("COORDIN-1kJQlDjc")
        }
        ["taste"] | ["taste", "add"] | ["taste", "drop"] => Landing::Gap("POLICY-1kJQlFB6"),
        _ => Landing::Unaccounted,
    }
}

/// Build the one parser and help tree from [`CONTRACTS`].
pub(crate) fn command() -> Command {
    command_from(contracts())
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
    if !children.is_empty() {
        command = command.subcommand_negates_reqs(true);
    }

    if let Some(contract) = contract {
        command = command.override_usage(format!("spice {}", contract.syntax));
        command = register_syntax(command, contract).unwrap_or_else(|error| {
            panic!("invalid CLI contract syntax {:?}: {error}", contract.path)
        });
        if !children.is_empty() {
            command = command.subcommand_negates_reqs(true);
        }
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

/// Recover one registered contract for an internal typed client preflight.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn contract(path: &[&str]) -> Option<&'static Contract> {
    contract_at(contracts(), path)
}

/// Recover the contract selected by the generated parser.
pub(crate) fn selected_contract(matches: &ArgMatches) -> Option<&'static Contract> {
    let mut path = Vec::new();
    let mut cursor = matches;
    while let Some((name, child)) = cursor.subcommand() {
        path.push(name);
        cursor = child;
    }
    contract_at(contracts(), &path)
}

/// Recover the leaf matches selected by the generated parser.
pub(crate) fn selected_matches(mut matches: &ArgMatches) -> &ArgMatches {
    while let Some((_, child)) = matches.subcommand() {
        matches = child;
    }
    matches
}

/// Recover the command passed through the root `run` leaf.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn run_command(matches: &ArgMatches) -> Option<Vec<OsString>> {
    matches
        .subcommand_matches("run")?
        .get_many::<OsString>("CMD")
        .map(|values| values.cloned().collect())
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
mod tests;
