use super::*;
/// Every current leaf. Parser registration and help are generated from these rows.
pub(super) const CONTRACTS: &[Contract] = &[
    row!(
        ["dev", "report-page"],
        "dev report-page REQUEST",
        OperatorOrLane,
        Read,
        [],
        [Resident],
        Reply,
        "report::read"
    ),
    local_direct!(
        ["demo"],
        "demo [--root DIR] [--reuse DIR] [--seed-only] [--preserve] [--open]",
        [Daemonless, Offline],
        Reply,
        "spice::demo"
    ),
    local_direct!(
        ["watch"],
        "watch [ROOT]",
        [Daemonless, Offline],
        Stream,
        "listen::watch"
    ),
    local_direct!(
        ["retro"],
        "retro [LANE|LOCUS|PATH]",
        [Daemonless, Offline],
        Stream,
        "listen::retro"
    ),
    local_direct!(
        ["session"],
        "session",
        [Daemonless, Offline],
        Reply,
        "rim::session_briefing"
    ),
    local_direct!(
        ["session", "briefing"],
        "session briefing [FILES...] [--start TIME] [--end TIME] [--contains TEXT] \
         [--turn-id ID...] [--tool NAME...] [--max-lines N] [--max-bytes N] \
         [--explain-pruning] [--deadline-seconds SECONDS]",
        [Daemonless, Offline],
        Reply,
        "rim::session_briefing"
    ),
    local_direct!(
        ["session", "commands"],
        "session commands [FILES...] [--limit N] [--newest-first] \
         [--since-compaction] [--failed] [--pipelines] [--noncanonical-pipelines] \
         [--summary] [--max-text N]",
        [Daemonless, Offline],
        Reply,
        "rim::session_commands"
    ),
    local_direct!(
        ["session", "context"],
        "session context SESSION [--lane UUID] [--vendor NAME] [--stream NAME] [--source UUID] \
         [--member N] [--offset N]",
        [Daemonless, Offline],
        Reply,
        "rim::session_context"
    ),
    local_direct!(
        ["session", "commits"],
        "session commits [FILES...] [--limit N]",
        [Daemonless, Offline],
        Reply,
        "rim::session_commits"
    ),
    local_direct!(
        ["session", "compactions"],
        "session compactions [FILES...] [--limit N]",
        [Daemonless, Offline],
        Reply,
        "rim::session_compactions"
    ),
    local_direct!(
        ["session", "messages"],
        "session messages [FILES...] [--start TIME] [--end TIME] [--contains TEXT] \
         [--turn-id ID...] [--side user|assistant...] \
         [--phase-kind prompt|commentary|final_answer...] [--flavor FLAVOR...] \
         [--limit N] [--oldest-first] [--max-text N]",
        [Daemonless, Offline],
        Reply,
        "rim::session_messages"
    ),
    local_direct!(
        ["session", "phases"],
        "session phases [FILES...] [--start TIME] [--end TIME] [--contains TEXT] \
         [--turn-id ID...] [--tool NAME...] [--limit N] [--examples N] [--max-text N]",
        [Daemonless, Offline],
        Reply,
        "rim::session_phases"
    ),
    local_direct!(
        ["session", "slices"],
        "session slices [FILES...] [--limit N] [--slice-id ID...] \
         [--view summary|full] [--max-text N]",
        [Daemonless, Offline],
        Reply,
        "rim::session_slices"
    ),
    local_direct!(
        ["session", "summary"],
        "session summary [FILES...] [--recent N]",
        [Daemonless, Offline],
        Reply,
        "rim::session_summary"
    ),
    local_direct!(
        ["session", "sweep"],
        "session sweep [FILES...] [--start TIME] [--end TIME] [--contains TEXT] \
         [--turn-id ID...] [--tool NAME...] [--count N] [--deadline-seconds SECONDS]",
        [Daemonless, Offline],
        Reply,
        "rim::session_sweep"
    ),
    local_direct!(
        ["session", "thread"],
        "session thread THREAD_ID",
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
        ["rtk", "gain"],
        "rtk gain AGENT_UUID",
        Local,
        None,
        [],
        [Daemonless, Toolful],
        RTK_QUERY_TOOLS,
        Reply,
        "spice::rtk_gain"
    ),
    row!(
        ["install"],
        "install [--package PACKAGE] [--apply=<digest>|--unapply]",
        Local,
        None,
        [],
        [Daemonless, Offline, DigestOnMutation],
        Plan,
        "spice::install"
    ),
    row!(
        ["install", "symbols"],
        "install symbols [--sample FILE]",
        Local,
        None,
        [],
        [Daemonless, Toolful],
        INSTALL_SYMBOLS_TOOLS,
        Reply,
        "spice::install_symbols"
    ),
    row!(
        ["dev", "cargo"],
        "dev cargo -- CARGO_ARGS...",
        Local,
        None,
        [],
        [Daemonless, Toolful],
        NATIVE_CARGO_TOOL,
        Stream,
        "cargo_isolation::dev_cargo"
    ),
    row!(
        ["dev", "xtask"],
        "dev xtask -- XTASK_ARGS...",
        Local,
        None,
        [],
        [Daemonless, Toolful],
        NATIVE_CARGO_TOOL,
        Stream,
        "cargo_isolation::dev_xtask"
    ),
    local_direct!(
        ["dev", "cargo-store"],
        "dev cargo-store [--cleanup]",
        [Daemonless, Offline],
        Reply,
        "cargo_isolation::store"
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
        Inspect,
        [],
        [Daemonless, Toolful],
        GATE_POLICY_TOOLS,
        Reply,
        "gate::run"
    ),
    row!(
        ["gate", "prove"],
        "gate prove [ACTION] [--missing]",
        Local,
        None,
        [],
        [Daemonless, Toolful],
        GATE_POLICY_TOOLS,
        Reply,
        "gate::prove"
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
        ["gate", "commit-msg"],
        "gate commit-msg MESSAGE_FILE",
        Local,
        None,
        [],
        [Daemonless, Offline],
        Reply,
        "gate::commit-message"
    ),
    row!(
        ["gate", "pre-commit"],
        "gate pre-commit [--profile-output PATH]",
        Local,
        None,
        [],
        [Daemonless, Toolful],
        GATE_POLICY_TOOLS,
        Reply,
        "gate::pre-commit"
    ),
    row!(
        ["gate", "selection"],
        "gate selection",
        Local,
        None,
        [],
        [Daemonless, Toolful],
        GATE_POLICY_TOOLS,
        Reply,
        "gate::selection"
    ),
    reference_transaction::CONTRACT,
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
        ["release", "seal"],
        "release seal",
        OperatorOrLane,
        Command,
        [ProofSealed],
        [Resident],
        Reply,
        "proof::manifest"
    ),
    row!(
        ["up"],
        "up [--foreground] [--host IP] [--port PORT] [--allow-insecure-bind] [--auth-token TOKEN] [--open]",
        Local,
        Resident,
        [JournalMigrated, TrustPended, UpSeated, ClaimRebound],
        [JournalLock, Toolful],
        UP_TOOLS,
        Stream,
        "spice::up"
    ),
    operator!(
        ["down"],
        "down",
        Command,
        [
            UpQuiescing,
            UpDraining,
            UpTerminating,
            UpKilling,
            UpVerifying,
            UpDark
        ],
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
    operator!(
        ["board", "handout"],
        "board handout OUTPUT [--at BORN]",
        Read,
        [],
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
        ["task", "show"],
        "task show HANDLE",
        OperatorOrLane,
        Read,
        [],
        [Resident],
        Reply,
        "board::task_show"
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
        ["task", "recover"],
        "task recover HANDLE",
        Lane,
        Command,
        [ClaimWon],
        [Resident, LaneCapability, Toolful],
        TASK_GIT_TOOLS,
        Reply,
        "board::recover"
    ),
    row!(
        ["task", "reclaim"],
        "task reclaim [HANDLE] [--lease-seconds SECONDS]",
        Lane,
        Command,
        [ClaimRenewed, ClaimRebound],
        [Resident, LaneCapability],
        Reply,
        "board::reclaim"
    ),
    row!(
        ["task", "add"],
        "task add TITLE [--project PROJECT] [--description DESCRIPTION] \
         [--acceptance ACCEPTANCE...] [--flow PHASES] [--requires-journal-schema SCHEMA] \
         [--requires-trust-grant DIGEST] [--origin ORIGIN] [--claim]",
        OperatorOrLane,
        Command,
        [TaskAdded, TaskNoted, ClaimWon],
        [Resident, Toolful],
        TASK_GIT_TOOLS,
        Reply,
        "board::task_add"
    ),
    row!(
        ["task", "capture"],
        "task capture [HANDLE] [--title TITLE] [--project PROJECT] [--origin ORIGIN] [--steal]",
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
        "task done [HANDLE] [--validation TEXT] [--operation OPERATION]",
        Lane,
        Command => DirectOrCommitted,
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
        [TaskReviewed, TaskAdded, TaskEdited, TaskNoted],
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
        [TaskDeferred, ClaimFreed],
        [Resident, Toolful],
        TASK_GIT_TOOLS,
        Reply,
        "board::task_defer"
    ),
    row!(
        ["task", "wake"],
        "task wake HANDLE [--into PROJECT] [--claim]",
        OperatorOrLane,
        Command,
        [TaskWoken, TaskEdited, ClaimWon],
        [Resident, Toolful],
        TASK_GIT_TOOLS,
        Reply,
        "board::task_wake"
    ),
    row!(
        ["task", "free"],
        "task free HANDLE",
        OperatorOrLane,
        Command,
        [ClaimFreed],
        [Resident, Toolful],
        TASK_GIT_TOOLS,
        Reply,
        "board::task_free"
    ),
    row!(
        ["task", "oops"],
        "task oops TEXT [--origin ORIGIN]",
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
        [TaskEdited, TaskNoted],
        [Resident],
        Reply,
        "board::task_modify"
    ),
    row!(
        ["task", "require"],
        "task require HANDLE [--journal-schema SCHEMA] [--trust-grant DIGEST] \
         [--appliance-after [VERSION]] [--clear]",
        OperatorOrLane,
        Command,
        [TaskEdited],
        [Resident],
        Reply,
        "board::task_require"
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
        "task artifact list HANDLE [--limit N] [--max-bytes N] [--after ARTIFACT]",
        OperatorOrLane,
        Read,
        [],
        [Daemonless],
        Reply,
        "artifact::list"
    ),
    row!(
        ["task", "artifact", "show"],
        "task artifact show HANDLE ARTIFACT [--offset N] [--bytes N] [--output PATH]",
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
        "doc PATH|- [--apply=<digest>]",
        OperatorOrLane,
        Command,
        [TaskAdded, TaskEdited, TaskNoted, DocApplied],
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
    operator!(["lane"], "lane", Read, [], Reply, "lane::read"),
    row!(
        ["lane", "list"],
        "lane list",
        OperatorOrLane,
        Read,
        [],
        [Resident],
        Reply,
        "lane::list"
    ),
    operator!(
        ["lane", "add"],
        "lane add [WORKTREE] [--team ID] [--name DISPLAY] \
         [--life Steer|Drive|Drain] [--vendor NAME] [--model MODEL] \
         [--effort low|medium|high|xhigh|max]",
        Command,
        [LaneSeated],
        Reply,
        "lane::add"
    ),
    operator!(
        ["lane", "drain"],
        "lane drain ID",
        Command,
        [
            LaneExited,
            LaneTimedOut,
            LaneCancelled,
            LaneSignalled,
            LaneDrained,
            LaneKilled,
            LaneDegraded
        ],
        Reply,
        "lane::drain"
    ),
    operator!(
        ["lane", "wake"],
        "lane wake ID",
        Command,
        [LaneSessionDeclared, LaneStarted, ClaimCarried],
        Reply,
        "lane::wake"
    ),
    operator!(
        ["lane", "model"],
        "lane model ID [MODEL]",
        Command,
        [LaneRetuned],
        Reply,
        "lane::model"
    ),
    operator!(
        ["lane", "remove"],
        "lane remove ID",
        Command,
        [LaneRemoved, ConsentLapsed],
        Reply,
        "lane::remove"
    ),
    operator!(
        ["lane", "purge"],
        "lane purge",
        Read,
        [],
        Reply,
        "lane::purge"
    ),
    operator!(
        ["lane", "import"],
        "lane import ID WORKTREE",
        Command,
        [LaneImported, LaneSessionDeclared, LaneStarted, ClaimCarried],
        Reply,
        "lane::import"
    ),
    operator!(
        ["steer"],
        "steer LANE TEXT",
        Command,
        [SteerSent],
        Reply,
        "steer::send"
    ),
    operator!(
        ["steer", "cancel"],
        "steer cancel ID",
        Command,
        [SteerCanceled],
        Reply,
        "steer::cancel"
    ),
    operator!(
        ["steer", "resend"],
        "steer resend ID",
        Command => SnapshotOrCommitted,
        [SteerEscalated],
        Reply,
        "steer::resend"
    ),
    operator!(
        ["steer", "recover"],
        "steer recover ID",
        Command,
        [SteerRecovered],
        Reply,
        "steer::recover"
    ),
    row!(
        ["tell"],
        "tell LANE TEXT",
        Lane,
        Command,
        [SteerSent],
        [Resident, LaneCapability],
        Reply,
        "steer::tell"
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
    operator!(["consent"], "consent", Read, [], Reply, "consent::read"),
    operator!(
        ["consent", "allow"],
        "consent allow ID",
        Command,
        [ConsentGranted],
        Reply,
        "consent::allow"
    ),
    operator!(
        ["consent", "deny"],
        "consent deny ID",
        Command,
        [ConsentDenied],
        Reply,
        "consent::deny"
    ),
    operator!(
        ["consent", "policy"],
        "consent policy",
        Read,
        [],
        Reply,
        "consent::policy"
    ),
    operator!(["taste"], "taste", Read, [], Reply, "taste::read"),
    operator!(
        ["taste", "add"],
        "taste add TEXT [--origin ORIGIN]",
        Command,
        [TaskAdded],
        Reply,
        "taste::add"
    ),
    operator!(
        ["taste", "drop"],
        "taste drop NAME [--origin ORIGIN]",
        Command,
        [TaskAdded],
        Reply,
        "taste::drop"
    ),
    operator!(
        ["taste", "mine"],
        "taste mine",
        Command,
        [TaskAdded],
        Reply,
        "taste::mine"
    ),
    operator!(["trust"], "trust", Read, [], Reply, "spice::trust"),
    operator!(
        ["trust", "pend"],
        "trust pend",
        Command,
        [TrustPended],
        Reply,
        "spice::trust_pend"
    ),
    operator!(
        ["trust", "grant"],
        "trust grant DIGEST",
        Command,
        [TrustGranted],
        Reply,
        "spice::trust_grant"
    ),
    operator!(
        ["trust", "revoke"],
        "trust revoke DIGEST",
        Command,
        [TrustRevoked],
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
    operator!(["journal"], "journal", Read, [], Reply, "journal::read"),
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
    operator!(
        ["journal", "verify"],
        "journal verify",
        Read,
        [],
        Reply,
        "journal::verify"
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
        "release publish [--notes-file FILE] [--release-commit REV] [--version VERSION] \
         [--action publish|tag-and-announce] [--apply=<digest>]",
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
