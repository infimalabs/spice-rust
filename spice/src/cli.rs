//! The generated command tree and its closed contract registry.

use std::collections::BTreeSet;

#[cfg(not(target_arch = "wasm32"))]
use std::ffi::{OsStr, OsString};
use std::time::Duration;

use clap::{Arg, ArgAction, ArgMatches, Command};
use wire::{CommandKind, FactKind};

mod mounts;
mod options;
mod syntax;
mod tools;

pub use mounts::{Mount, MountLoadError, MountRegistry, MountRegistryError, MountSelection};

use syntax::register_syntax;
#[cfg(test)]
use syntax::{is_optional, syntax_tokens};
pub use tools::ExternalTool;
/// The one compiled optional judge executable shared by resident and task adjudication.
pub const JUDGE_EXECUTABLE: &str = tools::JUDGE_EXECUTABLE;
use tools::{
    CALLER_COMMAND, DOCTOR_TOOLS, GATE_MUTATION_TOOLS, GATE_POLICY_TOOLS, INSTALL_SYMBOLS_TOOLS,
    NATIVE_CARGO_TOOL, NO_TOOLS, OCCUPANT_LAUNCH_TOOLS, PROOF_TOOLS, REFERENCE_TRANSACTION_TOOLS,
    RTK_QUERY_TOOLS, TASK_DONE_TOOLS, TASK_GIT_TOOLS, UP_TOOLS,
};
#[cfg(test)]
use tools::{ToolBounds, ToolCancellation, ToolNetwork, posture_violations};

/// The renderer flag, spelled once so its registration and its reader cannot
/// drift into naming two different arguments that merely agree today.
pub const JSON: &str = "json";
const MOUNT_ARGUMENT: &str = "MOUNT_ARGUMENTS";

/// Principal admitted to a command leaf.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Principal {
    Local,
    Operator,
    Lane,
    OperatorOrLane,
}

impl Principal {
    /// How a refusal names the principal a leaf admits.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Operator => "operator",
            Self::Lane => "lane",
            Self::OperatorOrLane => "operator or lane",
        }
    }
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
    /// The command either re-delivers an unchanged snapshot or commits movement.
    SnapshotOrCommitted,
    /// The command either hands off durable work or commits its terminal movement.
    DirectOrCommitted,
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

/// Temporary common bound for every app opening and command answer.
pub const APP_ANSWER_BOUND: Duration = Duration::from_secs(20);

/// Bound for an answer the resident composes from state it already holds.
const HELD_STATE_ANSWER: Duration = APP_ANSWER_BOUND;

/// Bound for an answer that first launches a process or publishes durable
/// objects, or waits for resident quiescence, all of which contend with every
/// other lane on the host.
const CONTENDED_ANSWER: Duration = APP_ANSWER_BOUND;

/// Bound for handing durable task-completion work to its resolution worker.
const TASK_COMPLETION_ANSWER: Duration = APP_ANSWER_BOUND;

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
        CommandKind::TaskDone => TASK_COMPLETION_ANSWER,
        CommandKind::LaneSeat
        | CommandKind::LaneState
        | CommandKind::ReleasePublish
        | CommandKind::UpChange => CONTENDED_ANSWER,
        CommandKind::BoardRead
        | CommandKind::ReportRead
        | CommandKind::TaskDetail
        | CommandKind::TaskRelations
        | CommandKind::TaskTimeline
        | CommandKind::BoardGraph
        | CommandKind::BoardHandout
        | CommandKind::DocLedger
        | CommandKind::WorkingGroupRead
        | CommandKind::WorkingGroupMove
        | CommandKind::WorkingGroupArrange
        | CommandKind::WorkingGroupDisband
        | CommandKind::WorkingGroupFast
        | CommandKind::DesignPropose
        | CommandKind::CandidateCritiqueRecord
        | CommandKind::CandidateCoverageRecord
        | CommandKind::PreparationDecide
        | CommandKind::CandidateWithdraw
        | CommandKind::OutputAdopt
        | CommandKind::DesignNudge
        | CommandKind::GlobalSettingsRead
        | CommandKind::GlobalSettingsSet
        | CommandKind::VendorCatalog
        | CommandKind::RoomPost
        | CommandKind::RoomConversationText
        | CommandKind::RoomLeave
        | CommandKind::RoomContribute
        | CommandKind::RoomContributionRead
        | CommandKind::RoomControl
        | CommandKind::TaskAdd
        | CommandKind::TaskOopsAdd
        | CommandKind::TaskEdit
        | CommandKind::TaskDelete
        | CommandKind::TaskClose
        | CommandKind::TaskConclude
        | CommandKind::TaskNote
        | CommandKind::TaskDefer
        | CommandKind::TaskWake
        | CommandKind::TaskFile
        | CommandKind::TaskDepends
        | CommandKind::TaskPhase
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
        | CommandKind::QuoteRead
        | CommandKind::SteerHistory
        | CommandKind::SteerResend
        | CommandKind::SteerRecover
        | CommandKind::SteerDispose
        | CommandKind::SteerCancel
        | CommandKind::ConsentRead
        | CommandKind::ConsentPolicy
        | CommandKind::ConsentDecide
        | CommandKind::TrustPend
        | CommandKind::TrustGrant
        | CommandKind::TrustRevoke
        | CommandKind::PrincipalDirectoryRead
        | CommandKind::PrincipalRead
        | CommandKind::PrincipalAdmit
        | CommandKind::PrincipalEdit
        | CommandKind::PrincipalGrant
        | CommandKind::PrincipalRevoke
        | CommandKind::PrincipalDeauthorize
        | CommandKind::LaneRead
        | CommandKind::LaneJobsRead
        | CommandKind::LaneJobRead
        | CommandKind::LaneJobStop
        | CommandKind::LaneRenewal
        | CommandKind::LaneRetune
        | CommandKind::LaneEffortRetune
        | CommandKind::LaneFastRetune
        | CommandKind::LaneArrange
        | CommandKind::TeamConfigure
        | CommandKind::TeamArrange
        | CommandKind::LaneRecall
        | CommandKind::TasteRecord
        | CommandKind::CanaryRecognize
        | CommandKind::VendorRecognize
        | CommandKind::CommitRecognize
        | CommandKind::ProofRecognize => HELD_STATE_ANSWER,
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
mod summaries;

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
/// not make its implementation real. The binary terminal branch uses this
/// classification to distinguish tracked work from a dispatch defect.
pub fn landing(path: &[&str]) -> Landing {
    if observation_landed(path) || repository_landed(path) || coordination_landed(path) {
        Landing::Landed
    } else {
        Landing::Unaccounted
    }
}

fn observation_landed(path: &[&str]) -> bool {
    matches!(
        path,
        ["app"]
            | ["demo"]
            | ["watch"]
            | ["retro"]
            | ["session"]
            | ["session", "briefing"]
            | ["session", "commands"]
            | ["session", "context"]
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
            | ["rtk", "gain"]
            | ["gate"]
            | ["gate", "prove"]
            | ["gate", "camp"]
            | ["gate", "commit-msg"]
            | ["gate", "kill"]
            | ["gate", "pre-commit"]
            | ["gate", "selection"]
            | ["gate", "reference-transaction"]
    )
}

fn repository_landed(path: &[&str]) -> bool {
    matches!(
        path,
        ["install"]
            | ["install", "symbols"]
            | ["dev", "cargo"]
            | ["dev", "xtask"]
            | ["dev", "cargo-store"]
            | ["dev", "cargo-footprint"]
            | ["dev", "report-page"]
            | ["lock"]
            | ["lock", "run"]
            | ["proof"]
            | ["proof", "replay"]
            | ["release", "notes"]
            | ["release", "seal"]
            | ["up"]
            | ["down"]
            | ["run"]
            | ["config"]
            | ["trust"]
            | ["trust", "pend"]
            | ["trust", "grant"]
            | ["trust", "revoke"]
            | ["backup"]
            | ["restore"]
            | ["journal"]
            | ["journal", "reindex"]
            | ["journal", "verify"]
            | ["thread"]
            | ["release", "publish"]
            | ["restore", "verify"]
    )
}

fn coordination_landed(path: &[&str]) -> bool {
    matches!(
        path,
        ["lane"]
            | ["lane", "list"]
            | ["lane", "add"]
            | ["lane", "drain"]
            | ["lane", "wake"]
            | ["lane", "model"]
            | ["lane", "effort"]
            | ["lane", "fast"]
            | ["lane", "remove"]
            | ["lane", "import"]
            | ["lane", "purge"]
            | ["recall"]
            | ["steer"]
            | ["steer", "cancel"]
            | ["steer", "resend"]
            | ["steer", "recover"]
            | ["inbox"]
            | ["quote"]
            | ["consent"]
            | ["consent", "allow"]
            | ["consent", "deny"]
            | ["consent", "policy"]
            | ["board"]
            | ["board", "graph"]
            | ["board", "handout"]
            | ["room", "post"]
            | ["room", "history"]
            | ["room", "leave"]
            | ["room", "contribute"]
            | ["room", "inbox"]
            | ["room", "result"]
            | ["room", "ack"]
            | ["room", "nak"]
            | ["room", "answer"]
            | ["room", "reply"]
            | ["room", "recover"]
            | ["room", "assess"]
            | ["room", "relevance"]
            | ["room", "awareness"]
            | ["room", "decline"]
            | ["room", "cancel"]
            | ["group", "list"]
            | ["group", "move"]
            | ["group", "arrange"]
            | ["group", "disband"]
            | ["principal", "list"]
            | ["principal", "admit"]
            | ["principal", "profile"]
            | ["principal", "role", "grant"]
            | ["principal", "role", "revoke"]
            | ["principal", "deauthorize"]
            | ["preparation", "propose"]
            | ["preparation", "show"]
            | ["preparation", "critique"]
            | ["preparation", "cover"]
            | ["preparation", "decide"]
            | ["preparation", "withdraw"]
            | ["preparation", "adopt"]
            | ["preparation", "nudge"]
            | ["task", "list"]
            | ["task", "status"]
            | ["task", "show"]
            | ["task", "next"]
            | ["task", "reclaim"]
            | ["task", "add"]
            | ["task", "plan"]
            | ["task", "design"]
            | ["task", "capture"]
            | ["task", "done"]
            | ["task", "review"]
            | ["task", "note"]
            | ["task", "defer"]
            | ["task", "wake"]
            | ["task", "file"]
            | ["task", "free"]
            | ["task", "oops"]
            | ["task", "depends"]
            | ["task", "modify"]
            | ["task", "require"]
            | ["task", "delete"]
            | ["task", "conclude"]
            | ["task", "artifact", "add"]
            | ["task", "artifact", "list"]
            | ["task", "artifact", "show"]
            | ["task", "artifact", "prune"]
            | ["task", "artifact", "remove"]
            | ["doc"]
            | ["doc", "ledger"]
            | ["taste"]
            | ["taste", "add"]
            | ["taste", "drop"]
    )
}

/// Build the one parser and help tree from the registry [`contracts()`] returns.
pub fn command() -> Command {
    command_from(contracts(), &[])
}

/// The bounded-output policy every read surface follows; see `wire::output`.
const OUTPUT_POLICY: &str = "\
Output: reads show current, relevant state by default. History and detail are
explicit flags, and every read stays bounded: --limit, --max-bytes, and
--max-text raise detail only up to the surface maximum. Withheld rows or text
are always counted, with the exact command that continues after them. Complete
documents and evidence are exported to a named file with their digest, never
streamed. Mutation replies are complete and concise.";

/// Finite watch defaults and explicit follow bounds.
const WATCH_HELP: &str = "\
Bare watch emits one bounded current transcript snapshot and exits. --follow
requires --deadline-seconds SECONDS (positive, at most 300), coalesces filesystem
wakes to at most ten per second, and stops at the deadline, 3000 wakes, or the
aggregate row/byte limit. Every successful result ends with a watch.v1 resume
cursor; --after CURSOR resumes without repeating an admitted record. Source
replacement and history skipped outside the bounded tail are explicit observations.";

/// How `task artifact list` pages its citations.
const ARTIFACT_LIST_HELP: &str = "\
Lists the task's artifacts newest first: 20 by default, at most 500 (--limit),
within 8 KiB of encoded rows by default and 128 KiB at most (--max-bytes). A
partial page states how many it withheld and the exact command that continues
at the first withheld artifact (--after ARTIFACT, inclusive).";

/// How `task artifact show` reads a payload without flooding a terminal.
const ARTIFACT_SHOW_HELP: &str = "\
Shows the artifact's metadata and a bounded window of its payload, after
proving the payload against its retained digest. Text shows its first 4096
bytes; a non-text payload shows only its store path. --offset and --bytes
(1 to 16384) read a window anywhere: text on character boundaries, other
payloads as hex. A window that stops short names the command for the next.
No form writes raw payload bytes to the terminal.

--output PATH writes the exact whole payload to PATH atomically, verifies the
written file against the retained digest, and reports its path, size, and
digest without echoing contents. It is never truncated; '-' is refused.";

/// Selection, claim, and dry-board semantics for the allocator entry point.
const TASK_NEXT_HELP: &str = "\
Returns this lane's current assignment or pending private Design request first.
Otherwise claims the allocator's next eligible task. When no task is eligible,
reports a bounded board summary and leaves state unchanged.";

/// Initial framing for one open Design.
const TASK_DESIGN_HELP: &str = "\
A Design is itself a question: phrase TITLE as the question under Design.
Each --acceptance value is one open question for evidence and proposals to
answer, not an implementation completion criterion.";

/// What `preparation propose` assembles and what its holder still authors.
const PREPARATION_PROPOSE_HELP: &str = "\
Ends the pass of the open Design this capability holds (--design HANDLE when it
holds several) and releases that claim; the Design stays open for the
operator's decision. The proposal is assembled, not typed: its outputs are the
pending tasks the pass built under the claim that no decision has resolved,
its governed inputs their other pending dependencies, and its references every
contribution in the Design's room (for, against, evidence, or question by its
kind) plus every earlier decision. JUSTIFICATION is the holder's own argument.
A pass that built nothing recommends --recommend conclude or collapse; one
that built outputs recommends that work. --cite RELATION=KEY adds a source
(the keys room contribute --cite takes, or any publication stamp the room
prints) or relabels an assembled one; RELATION is one lower-case word.
--dry-run prints the proposal and records nothing. The Design, its outputs and
inputs, and its room are witnessed, so any of them moving before the write
refuses it.";

/// Usage notes for a leaf whose bounds its syntax line cannot carry.
fn leaf_help(path: &[&str]) -> Option<&'static str> {
    match path {
        ["watch"] => Some(WATCH_HELP),
        ["task", "next"] => Some(TASK_NEXT_HELP),
        ["task", "design"] => Some(TASK_DESIGN_HELP),
        ["preparation", "propose"] => Some(PREPARATION_PROPOSE_HELP),
        ["task", "artifact", "list"] => Some(ARTIFACT_LIST_HELP),
        ["task", "artifact", "show"] => Some(ARTIFACT_SHOW_HELP),
        _ => None,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn product_version() -> String {
    crate::version::product().to_string()
}

#[cfg(target_arch = "wasm32")]
fn product_version() -> String {
    dist::current_tracked_version()
        .expect("the tracked product version is validated by the repository gate")
        .product(None, dist::Build::Dev)
        .to_string()
}

fn command_from(contracts: &[Contract], mounts: &[Mount]) -> Command {
    let mut root = Command::new("spice")
        .about("Repository-local coordination appliance")
        .after_help(OUTPUT_POLICY)
        .version(product_version())
        .subcommand_required(true)
        .arg_required_else_help(true)
        .disable_help_subcommand(true)
        .arg(
            Arg::new(JSON)
                .long(JSON)
                .global(true)
                .action(ArgAction::SetTrue),
        );
    root = options::describe(root, &[])
        .unwrap_or_else(|error| panic!("invalid root option help: {error}"));

    for child in child_names(contracts, mounts, &[]) {
        root = root.subcommand(summarized(contracts, mounts, &[child]));
    }
    root
}

fn branch(contracts: &[Contract], mounts: &[Mount], name: &str, path: &[String]) -> Command {
    let borrowed = path.iter().map(String::as_str).collect::<Vec<_>>();
    let contract = contract_at(contracts, &borrowed);
    let mount = mount_at(mounts, &borrowed);
    let children = child_names(contracts, mounts, &borrowed);
    let mut command = Command::new(name.to_owned()).disable_help_subcommand(true);
    if !children.is_empty() {
        command = command.subcommand_negates_reqs(true);
    }

    if let Some(contract) = contract {
        command = command.override_usage(format!("spice {}", contract.syntax));
        if let Some(help) = leaf_help(&borrowed) {
            command = command.after_help(help);
        }
        command = register_syntax(command, contract).unwrap_or_else(|error| {
            panic!("invalid CLI contract syntax {:?}: {error}", contract.path)
        });
        command = options::describe(command, contract.path)
            .unwrap_or_else(|error| panic!("invalid CLI option help {:?}: {error}", contract.path));
        if !children.is_empty() {
            command = command.subcommand_negates_reqs(true);
        }
    } else if mount.is_some() {
        command = command.disable_help_flag(true).trailing_var_arg(true).arg(
            Arg::new(MOUNT_ARGUMENT)
                .num_args(0..)
                .allow_hyphen_values(true)
                .value_parser(clap::builder::OsStringValueParser::new()),
        );
    } else {
        command = command
            .subcommand_required(true)
            .arg_required_else_help(true);
    }

    for child in children {
        let mut child_path = path.to_vec();
        child_path.push(child);
        command = command.subcommand(summarized(contracts, mounts, &child_path));
    }
    command.subcommand_precedence_over_arg(true)
}

/// One branch under its help row; a registered path without one refuses.
fn summarized(contracts: &[Contract], mounts: &[Mount], path: &[String]) -> Command {
    let registered = contracts.iter().any(|contract| {
        contract.path.len() >= path.len()
            && contract
                .path
                .iter()
                .zip(path)
                .all(|(word, name)| *word == name.as_str())
    });
    let summary = match summaries::summary(path) {
        Some(summary) => summary,
        None if registered => panic!("registered command {path:?} has no help summary"),
        None => summaries::MOUNTED_SUMMARY,
    };
    let name = path.last().expect("a branch path names its command");
    branch(contracts, mounts, name, path).about(summary)
}

fn child_names(contracts: &[Contract], mounts: &[Mount], path: &[&str]) -> BTreeSet<String> {
    contracts
        .iter()
        .filter(|contract| contract.path.starts_with(path) && contract.path.len() > path.len())
        .map(|contract| contract.path[path.len()].to_owned())
        .chain(
            mounts
                .iter()
                .filter(|mount| {
                    mount.path().len() > path.len()
                        && mount
                            .path()
                            .iter()
                            .map(String::as_str)
                            .zip(path.iter().copied())
                            .all(|(left, right)| left == right)
                })
                .map(|mount| mount.path()[path.len()].clone()),
        )
        .collect()
}

fn mount_at<'a>(mounts: &'a [Mount], path: &[&str]) -> Option<&'a Mount> {
    mounts.iter().find(|mount| {
        mount.path().len() == path.len()
            && mount
                .path()
                .iter()
                .map(String::as_str)
                .eq(path.iter().copied())
    })
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
    let path = selected_path(matches);
    contract_at(contracts(), &path)
}

fn selected_path(matches: &ArgMatches) -> Vec<&str> {
    let mut path = Vec::new();
    let mut cursor = matches;
    while let Some((name, child)) = cursor.subcommand() {
        path.push(name);
        cursor = child;
    }
    path
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

/// Whether a raw native invocation requests JSON before repository command
/// mounts have been loaded and the complete parser can exist.
///
/// This bootstrap read is used only to render a mount-loading failure. The
/// first `--` ends Spice's arguments, exactly as it does for the generated
/// parser, so a child command's own `--json` remains its operand.
#[cfg(not(target_arch = "wasm32"))]
pub fn bootstrap_json_selected(arguments: impl IntoIterator<Item = OsString>) -> bool {
    let requested = format!("--{JSON}");
    for argument in arguments.into_iter().skip(1) {
        if argument == OsStr::new("--") {
            return false;
        }
        if argument == OsStr::new(&requested) {
            return true;
        }
    }
    false
}

/// Whether the raw native invocation selects a top-level configuration owner.
///
/// Config and Doctor own typed configuration refusal and diagnosis, so their
/// bootstrap paths must be recognizable before repository command mounts can
/// be loaded. Only the first non-global argument is considered; a nested or
/// child-command token has no authority to change mount loading.
/// This is only a routing hint; the generated parser remains the syntax authority.
#[cfg(not(target_arch = "wasm32"))]
pub fn bootstrap_configuration_owner_selected(
    arguments: impl IntoIterator<Item = OsString>,
) -> bool {
    bootstrap_leaf_selected(arguments, &["config", "doctor"])
}

/// Whether the raw native invocation selects the top-level Doctor command.
#[cfg(not(target_arch = "wasm32"))]
pub fn bootstrap_doctor_selected(arguments: impl IntoIterator<Item = OsString>) -> bool {
    bootstrap_leaf_selected(arguments, &["doctor"])
}

/// Whether the raw native invocation selects the top-level Lock command.
#[cfg(not(target_arch = "wasm32"))]
pub fn bootstrap_lock_selected(arguments: impl IntoIterator<Item = OsString>) -> bool {
    bootstrap_leaf_selected(arguments, &["lock"])
}

#[cfg(not(target_arch = "wasm32"))]
fn bootstrap_leaf_selected(arguments: impl IntoIterator<Item = OsString>, leaves: &[&str]) -> bool {
    let json = format!("--{JSON}");
    for argument in arguments.into_iter().skip(1) {
        if argument == OsStr::new("--") {
            return false;
        }
        if argument == OsStr::new(&json) {
            continue;
        }
        return leaves.iter().any(|leaf| argument == OsStr::new(leaf));
    }
    false
}

#[cfg(test)]
mod tests;
