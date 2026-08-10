use super::*;

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
pub(super) const CONTRACTS: &[Contract] = &[
    row!(
        ["demo"],
        "demo [--root DIR] [--reuse DIR] [--seed-only] [--preserve] [--open]",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "spice::demo"
    ),
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
        ["session"],
        "session",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "rim::session_briefing"
    ),
    row!(
        ["session", "briefing"],
        "session briefing [FILES...] [--start TIME] [--end TIME] [--contains TEXT] \
         [--turn-id ID...] [--tool NAME...] [--max-lines N] [--max-bytes N] \
         [--explain-pruning] [--deadline-seconds SECONDS]",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "rim::session_briefing"
    ),
    row!(
        ["session", "commands"],
        "session commands [FILES...] [--limit N] [--newest-first] \
         [--since-compaction] [--failed] [--pipelines] [--noncanonical-pipelines] \
         [--summary] [--max-text N]",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "rim::session_commands"
    ),
    row!(
        ["session", "commits"],
        "session commits [FILES...] [--limit N]",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "rim::session_commits"
    ),
    row!(
        ["session", "compactions"],
        "session compactions [FILES...] [--limit N]",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "rim::session_compactions"
    ),
    row!(
        ["session", "messages"],
        "session messages [FILES...] [--start TIME] [--end TIME] [--contains TEXT] \
         [--turn-id ID...] [--side user|assistant...] \
         [--phase-kind prompt|commentary|final_answer...] [--flavor FLAVOR...] \
         [--limit N] [--oldest-first] [--max-text N]",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "rim::session_messages"
    ),
    row!(
        ["session", "phases"],
        "session phases [FILES...] [--start TIME] [--end TIME] [--contains TEXT] \
         [--turn-id ID...] [--tool NAME...] [--limit N] [--examples N] [--max-text N]",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "rim::session_phases"
    ),
    row!(
        ["session", "slices"],
        "session slices [FILES...] [--limit N] [--slice-id ID...] \
         [--view summary|full] [--max-text N]",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "rim::session_slices"
    ),
    row!(
        ["session", "summary"],
        "session summary [FILES...] [--recent N]",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "rim::session_summary"
    ),
    row!(
        ["session", "sweep"],
        "session sweep [FILES...] [--start TIME] [--end TIME] [--contains TEXT] \
         [--turn-id ID...] [--tool NAME...] [--count N] [--deadline-seconds SECONDS]",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "rim::session_sweep"
    ),
    row!(
        ["session", "thread"],
        "session thread THREAD_ID",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "rim::session_thread"
    ),
    row!(
        ["session", "timeline"],
        "session timeline [FILES...] [--start TIME] [--end TIME] [--contains TEXT] \
         [--turn-id ID...] [--tool NAME...] [--limit N] [--max-text N]",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "rim::session_timeline"
    ),
    row!(
        ["session", "tokens"],
        "session tokens [FILES...] [--by-file]",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "rim::session_tokens"
    ),
    row!(
        ["session", "turns"],
        "session turns [FILES...] [--start TIME] [--end TIME] [--contains TEXT] \
         [--turn-id ID...] [--tool NAME...] [--limit N] [--view dialogue|full]",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "rim::session_turns"
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
        ["gate", "findings"],
        "gate findings INSTRUMENT [--at REV]",
        OperatorOrLane,
        Command,
        [GateFindingsFiled, TaskAdded, TaskDeferred],
        [Resident, Toolful],
        GATE_POLICY_TOOLS,
        Reply,
        "gate::findings"
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
    // Published as `dev pre-commit` until this row moved. `dev` was the
    // predecessor's single `configure_dev_parser` family, which this port split
    // into `install` and `gate`; both successors are live, and this leaf was the
    // only one the split left behind. Its contract id and its handler already
    // named `gate`, so the retirement row for `dev` was accurate and the path
    // was the leftover -- the path moved rather than the row narrowing.
    row!(
        ["gate", "pre-commit"],
        "gate pre-commit",
        Local,
        None,
        [],
        [Daemonless, Toolful],
        GATE_POLICY_TOOLS,
        Reply,
        "gate::pre-commit"
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
        "release notes [--release-commit REV]",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "dist::release_notes"
    ),
    row!(
        ["up"],
        "up [--host HOST] [--port PORT] [--allow-insecure-bind] [--auth-token TOKEN] [--open]",
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
        ["task", "list"],
        "task list [--all] [--project PROJECT] [--limit N] \
         [--status pending|waiting|completed|deleted]",
        OperatorOrLane,
        Read,
        [],
        [Resident],
        Reply,
        "board::list"
    ),
    row!(
        ["task", "status"],
        "task status",
        OperatorOrLane,
        Read,
        [],
        [Resident],
        Reply,
        "board::status"
    ),
    row!(
        ["task", "effort"],
        "task effort [HANDLE]",
        OperatorOrLane,
        Read,
        [],
        [Resident],
        Reply,
        "fold::task_effort"
    ),
    row!(
        ["task", "next"],
        "task next",
        OperatorOrLane,
        Command,
        [ClaimWon],
        [Resident, Toolful],
        TASK_GIT_TOOLS,
        Reply,
        "board::next"
    ),
    row!(
        ["task", "reclaim"],
        "task reclaim [HANDLE] [--lease-seconds SECONDS]",
        Lane,
        Command,
        [ClaimRenewed],
        [Resident, LaneCapability],
        Reply,
        "board::reclaim"
    ),
    row!(
        ["task", "add"],
        "task add TITLE [--project PROJECT] [--description DESCRIPTION] \
         [--acceptance ACCEPTANCE...] [--origin ORIGIN]",
        OperatorOrLane,
        Command,
        [TaskAdded],
        [Resident],
        Reply,
        "board::task_add"
    ),
    row!(
        ["task", "capture"],
        "task capture [HANDLE] [--title TITLE] [--project PROJECT] [--steal]",
        OperatorOrLane,
        Command,
        [ClaimWon],
        [Resident, Toolful],
        TASK_GIT_TOOLS,
        Reply,
        "board::task_capture"
    ),
    row!(
        ["task", "done"],
        "task done [--validation TEXT]",
        Lane,
        Command,
        [TaskDone, TaskPhased],
        [Resident, LaneCapability, Toolful],
        TASK_DONE_TOOLS,
        Reply,
        "board::task_done"
    ),
    row!(
        ["task", "review"],
        "task review HANDLE [--finding TEXT] [--note TEXT] \
         [--then FOLLOWUP...] [--followup HANDLE...]",
        Lane,
        Command,
        [TaskReviewed, TaskAdded, TaskEdited],
        [Resident, LaneCapability, Toolful],
        TASK_GIT_TOOLS,
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
        ["task", "modify"],
        "task modify HANDLE [--title TITLE] [--description DESCRIPTION] \
         [--acceptance ACCEPTANCE...] [--priority PRIORITY] [--project PROJECT]",
        OperatorOrLane,
        Command,
        [TaskEdited],
        [Resident],
        Reply,
        "board::task_modify"
    ),
    row!(
        ["task", "delete"],
        "task delete HANDLE REASON [--force-claimed]",
        OperatorOrLane,
        Command,
        [TaskDeleted],
        [Resident],
        Reply,
        "board::task_delete"
    ),
    row!(
        ["task", "artifact", "add"],
        "task artifact add HANDLE PATH [--name NAME] [--type TYPE] [--retention RETENTION]",
        OperatorOrLane,
        Read,
        [],
        [Daemonless],
        Reply,
        "artifact::add"
    ),
    row!(
        ["task", "artifact", "list"],
        "task artifact list HANDLE",
        OperatorOrLane,
        Read,
        [],
        [Daemonless],
        Reply,
        "artifact::list"
    ),
    row!(
        ["task", "artifact", "show"],
        "task artifact show HANDLE ARTIFACT",
        OperatorOrLane,
        Read,
        [],
        [Daemonless],
        Reply,
        "artifact::show"
    ),
    row!(
        ["task", "artifact", "prune"],
        "task artifact prune [--older-than DURATION] [--apply]",
        OperatorOrLane,
        Read,
        [],
        [Daemonless],
        Reply,
        "artifact::prune"
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
        "lane add [WORKTREE] [--lineage ID] [--name DISPLAY] \
         [--life Steer|Drive|Drain] [--vendor NAME] [--resume SESSION]",
        Operator,
        Command,
        [LaneSeated, LaneSessionDeclared, LaneStarted],
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
        [LaneSessionDeclared, LaneStarted, ClaimCarried],
        [Resident, OperatorCapability],
        Reply,
        "lane::wake"
    ),
    row!(
        ["lane", "import"],
        "lane import ID WORKTREE",
        Operator,
        Command,
        [LaneImported, LaneSessionDeclared, LaneStarted, ClaimCarried],
        [Resident, OperatorCapability],
        Reply,
        "lane::import"
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
        ["steer", "resend"],
        "steer resend ID",
        Operator,
        Command => SnapshotOrCommitted,
        [SteerEscalated],
        [Resident, OperatorCapability],
        Reply,
        "steer::resend"
    ),
    row!(
        ["steer", "recover"],
        "steer recover ID",
        Operator,
        Command,
        [SteerRecovered],
        [Resident, OperatorCapability],
        Reply,
        "steer::recover"
    ),
    row!(
        ["steer", "repair"],
        "steer repair LANE",
        Operator,
        Recovery => Snapshot,
        [],
        [Resident, OperatorCapability],
        Reply,
        "steer::repair"
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
        ["hook"],
        "hook HARNESS",
        Lane,
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
        ["release", "publish"],
        "release publish [--notes-file FILE] [--release-commit REV] [--apply=<digest>]",
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
    row!(
        ["config", "migrate-tasks"],
        "config migrate-tasks EXPORT",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "spice::task_config_migration"
    ),
];
