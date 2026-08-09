//! The generated command tree and its closed contract registry.

use std::collections::BTreeSet;

#[cfg(not(target_arch = "wasm32"))]
use std::ffi::OsString;
use std::time::Duration;

use clap::{Arg, ArgAction, ArgMatches, Command};
use wire::{CommandKind, FactKind};

mod syntax;
mod tools;

use syntax::register_syntax;
#[cfg(test)]
use syntax::{is_optional, syntax_tokens};
pub use tools::ExternalTool;
use tools::{
    CALLER_COMMAND, DOCTOR_TOOLS, GATE_MUTATION_TOOLS, GATE_POLICY_TOOLS, NO_TOOLS, PROOF_TOOLS,
    TASK_DONE_TOOLS, TASK_GIT_TOOLS,
};
#[cfg(test)]
use tools::{
    LEARNING_JUDGE_EXECUTABLE, ToolBounds, ToolCancellation, ToolNetwork, posture_violations,
};

/// The renderer flag, spelled once so its registration and its reader cannot
/// drift into naming two different arguments that merely agree today.
const JSON: &str = "json";

/// Principal admitted to a command leaf.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Principal {
    Local,
    Operator,
    Lane,
    OperatorOrLane,
}

/// Relationship a leaf has with the authoritative journal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalAccess {
    None,
    Inspect,
    Read,
    Command,
    Recovery,
    Resident,
}

/// Journal relationship a successful reply must report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplyPositionLaw {
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
pub enum Guard {
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
pub enum ResultClass {
    Reply,
    Plan,
    Stream,
}

/// Public process outcomes shared by every command renderer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitOutcome {
    Reply,
    Refusal,
    Fault,
}

impl ExitOutcome {
    pub const ALL: &[Self] = &[Self::Reply, Self::Refusal, Self::Fault];

    #[cfg(test)]
    const fn code(self) -> i32 {
        match self {
            Self::Reply => 0,
            Self::Refusal => 2,
            Self::Fault => 1,
        }
    }
}

/// Bound for an answer the resident composes from state it already holds.
const HELD_STATE_ANSWER: Duration = Duration::from_secs(3);

/// Bound for an answer that first launches a process or publishes durable
/// objects, both of which contend with every other lane on the host.
const CONTENDED_ANSWER: Duration = Duration::from_secs(20);

/// How long the native client blocks for one command's answer.
///
/// This is the registry. The match is exhaustive, so a command added without a
/// stated bound fails to compile: the bound cannot be inherited or guessed,
/// only stated. A command reaches the contended bound when its answer waits on
/// work the host schedules against every other lane, and an answer that has not
/// arrived by then is reported as an elapsed deadline rather than as a peer
/// that failed.
#[must_use]
pub const fn answer_bound(kind: CommandKind) -> Duration {
    match kind {
        CommandKind::LaneSeat | CommandKind::LaneState | CommandKind::ReleasePublish => {
            CONTENDED_ANSWER
        }
        CommandKind::BoardRead
        | CommandKind::BoardGraph
        | CommandKind::BoardHandout
        | CommandKind::DocLedger
        | CommandKind::EffortRead
        | CommandKind::TaskAdd
        | CommandKind::TaskOopsAdd
        | CommandKind::GateFindings
        | CommandKind::TaskEdit
        | CommandKind::TaskDelete
        | CommandKind::TaskNote
        | CommandKind::TaskDefer
        | CommandKind::TaskWake
        | CommandKind::TaskDepends
        | CommandKind::TaskPhase
        | CommandKind::TaskDone
        | CommandKind::TaskReview
        | CommandKind::TaskAssign
        | CommandKind::ClaimWin
        | CommandKind::ClaimCarry
        | CommandKind::ClaimRenew
        | CommandKind::ClaimFree
        | CommandKind::ClaimLapse
        | CommandKind::DocApply
        | CommandKind::SteerSend
        | CommandKind::SteerInbox
        | CommandKind::SteerResend
        | CommandKind::SteerDispose
        | CommandKind::SteerCancel
        | CommandKind::ConsentRead
        | CommandKind::ConsentPolicy
        | CommandKind::ConsentDecide
        | CommandKind::TrustPend
        | CommandKind::TrustGrant
        | CommandKind::TrustRevoke
        | CommandKind::LaneRead
        | CommandKind::LaneArrange
        | CommandKind::LineageConfigure
        | CommandKind::LineageArrange
        | CommandKind::LaneRecall
        | CommandKind::TasteRecord
        | CommandKind::CanaryRecognize
        | CommandKind::VendorRecognize
        | CommandKind::CommitRecognize
        | CommandKind::ProofRecognize
        | CommandKind::UpChange => HELD_STATE_ANSWER,
    }
}

/// One generated CLI contract row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Contract {
    pub path: &'static [&'static str],
    pub syntax: &'static str,
    pub principal: Principal,
    pub journal: JournalAccess,
    pub reply_position: ReplyPositionLaw,
    pub facts: &'static [FactKind],
    pub guards: &'static [Guard],
    pub tools: &'static [ExternalTool],
    pub result: ResultClass,
    pub outcomes: &'static [ExitOutcome],
    pub battery: &'static str,
}

/// Whether one registered leaf has executable Rust dispatch today.
///
/// A gap names the public task that owns it. `Unaccounted` exists so a newly
/// registered leaf fails the landing check instead of quietly joining the
/// terminal release refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Landing {
    Landed,
    /// A current public task owns this intentionally visible terminal refusal.
    GapOwned(&'static str),
    Unaccounted,
}

mod contracts;
use contracts::*;

/// The complete registry in the order used by help and Doctor rendering.
///
/// Returning the static rows performs no discovery or inspection. The Doctor
/// surface adds observations later without gaining a second command list.
pub const fn contracts() -> &'static [Contract] {
    CONTRACTS
}

/// Classify executable landing independently from parser registration.
///
/// This second closed table is intentional: adding a command contract does
/// not make its implementation real. The closure test requires every current
/// leaf to appear here, while the binary terminal branch uses the same
/// classification to distinguish tracked work from a dispatch defect.
pub fn landing(path: &[&str]) -> Landing {
    match path {
        ["demo"]
        | ["watch"]
        | ["retro"]
        | ["session"]
        | ["session", "briefing"]
        | ["session", "commands"]
        | ["session", "commits"]
        | ["session", "compactions"]
        | ["session", "messages"]
        | ["session", "phases"]
        | ["session", "slices"]
        | ["session", "summary"]
        | ["session", "sweep"]
        | ["session", "thread"]
        | ["session", "timeline"]
        | ["session", "tokens"]
        | ["session", "turns"]
        | ["doctor"]
        | ["version"]
        | ["gate"]
        | ["gate", "findings"]
        | ["gate", "camp"]
        | ["gate", "kill"]
        | ["gate", "pre-commit"]
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
        | ["config", "migrate-tasks"]
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
        | ["task", "effort"]
        | ["thread"]
        | ["release", "publish"]
        | ["restore", "verify"]
        | ["lane"]
        | ["lane", "add"]
        | ["lane", "drain"]
        | ["lane", "wake"]
        | ["lane", "import"]
        | ["recall"]
        | ["steer"]
        | ["steer", "cancel"]
        | ["steer", "recover"]
        | ["inbox"]
        | ["hook"]
        | ["ack"]
        | ["consent"]
        | ["consent", "allow"]
        | ["consent", "deny"]
        | ["consent", "policy"]
        | ["board"]
        | ["board", "graph"]
        | ["board", "handout"]
        | ["task", "list"]
        | ["task", "status"]
        | ["task", "next"]
        | ["task", "reclaim"]
        | ["task", "add"]
        | ["task", "capture"]
        | ["task", "done"]
        | ["task", "review"]
        | ["task", "note"]
        | ["task", "defer"]
        | ["task", "wake"]
        | ["task", "free"]
        | ["task", "oops"]
        | ["task", "depends"]
        | ["task", "modify"]
        | ["task", "delete"]
        | ["task", "artifact", "add"]
        | ["task", "artifact", "list"]
        | ["task", "artifact", "show"]
        | ["task", "artifact", "prune"]
        | ["doc"]
        | ["doc", "ledger"]
        | ["taste"]
        | ["taste", "add"]
        | ["taste", "drop"] => Landing::Landed,
        _ => Landing::Unaccounted,
    }
}

/// Build the one parser and help tree from the registry [`contracts()`] returns.
pub fn command() -> Command {
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
pub fn contract(path: &[&str]) -> Option<&'static Contract> {
    contract_at(contracts(), path)
}

/// Recover the contract selected by the generated parser.
pub fn selected_contract(matches: &ArgMatches) -> Option<&'static Contract> {
    let mut path = Vec::new();
    let mut cursor = matches;
    while let Some((name, child)) = cursor.subcommand() {
        path.push(name);
        cursor = child;
    }
    contract_at(contracts(), &path)
}

/// Recover the leaf matches selected by the generated parser.
pub fn selected_matches(mut matches: &ArgMatches) -> &ArgMatches {
    while let Some((_, child)) = matches.subcommand() {
        matches = child;
    }
    matches
}

/// Recover the command passed through the root `run` leaf.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_command(matches: &ArgMatches) -> Option<Vec<OsString>> {
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
pub fn json_selected(matches: &ArgMatches) -> bool {
    matches.get_flag(JSON)
}

#[cfg(test)]
mod tests;
