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
        "watch [ROOT] [--follow] [--deadline-seconds SECONDS] [--after CURSOR] [--limit N] \
         [--max-bytes N] [--max-text N]",
        [Daemonless, Offline],
        Reply,
        "listen::watch"
    ),
    local_direct!(
        ["retro"],
        "retro [LANE|LOCUS|PATH] [--kind KIND...] [--limit N] [--max-bytes N] [--max-text N] \
         [--after CURSOR] [--export FILE]",
        [Daemonless, Offline],
        Reply,
        "listen::transcript_page"
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
        "session briefing [FILES...] [--deadline-seconds SECONDS]",
        [Daemonless, Offline],
        Reply,
        "rim::session_briefing"
    ),
    local_direct!(
        ["session", "commands"],
        "session commands [FILES...] [--since-compaction] [--failed] [--pipelines] \
         [--noncanonical-pipelines] [--summary] [--limit N] [--max-bytes N] [--max-text N] \
         [--after CURSOR] [--export FILE]",
        [Daemonless, Offline],
        Reply,
        "rim::session_commands"
    ),
    local_direct!(
        ["session", "context"],
        "session context SESSION [--lane UUID] [--vendor NAME] [--stream NAME] [--source UUID] \
         [--member N] [--offset N] [--limit N] [--max-bytes N] [--max-text N] \
         [--after CURSOR] [--export FILE]",
        [Daemonless, Offline],
        Reply,
        "rim::session_context"
    ),
    local_direct!(
        ["session", "commits"],
        "session commits [FILES...] [--limit N] [--max-bytes N] [--max-text N] \
         [--after CURSOR] [--export FILE]",
        [Daemonless, Offline],
        Reply,
        "rim::session_commits"
    ),
    local_direct!(
        ["session", "compactions"],
        "session compactions [FILES...] [--limit N] [--max-bytes N] [--max-text N] \
         [--after CURSOR] [--export FILE]",
        [Daemonless, Offline],
        Reply,
        "rim::session_compactions"
    ),
    local_direct!(
        ["session", "messages"],
        "session messages [FILES...] [--start TIME] [--end TIME] [--contains TEXT] \
         [--turn-id ID...] [--side user|assistant...] \
         [--phase-kind prompt|commentary|final_answer...] [--flavor FLAVOR...] \
         [--limit N] [--max-bytes N] [--max-text N] [--after CURSOR] [--export FILE]",
        [Daemonless, Offline],
        Reply,
        "rim::session_messages"
    ),
    local_direct!(
        ["session", "phases"],
        "session phases [FILES...] [--start TIME] [--end TIME] [--contains TEXT] \
         [--turn-id ID...] [--tool NAME...] [--examples N] [--limit N] [--max-bytes N] \
         [--max-text N] [--after CURSOR] [--export FILE]",
        [Daemonless, Offline],
        Reply,
        "rim::session_phases"
    ),
    local_direct!(
        ["session", "slices"],
        "session slices [FILES...] [--slice-id ID...] [--view summary|full] [--limit N] \
         [--max-bytes N] [--max-text N] [--after CURSOR] [--export FILE]",
        [Daemonless, Offline],
        Reply,
        "rim::session_slices"
    ),
    local_direct!(
        ["session", "summary"],
        "session summary [FILES...] [--limit N] [--max-bytes N] [--max-text N] \
         [--after CURSOR] [--export FILE]",
        [Daemonless, Offline],
        Reply,
        "rim::session_summary"
    ),
    local_direct!(
        ["session", "sweep"],
        "session sweep [FILES...] [--limit N] [--max-bytes N] [--max-text N] \
         [--after CURSOR] [--export FILE] [--deadline-seconds SECONDS]",
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
         [--turn-id ID...] [--tool NAME...] [--limit N] [--max-bytes N] [--max-text N] \
         [--after CURSOR] [--export FILE]",
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
         [--turn-id ID...] [--tool NAME...] [--view dialogue|full] [--limit N] \
         [--max-bytes N] [--max-text N] [--after CURSOR] [--export FILE]",
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
        "dev cargo [--verbose] -- CARGO_ARGS...",
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
        "dev xtask [--verbose] -- XTASK_ARGS...",
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
    local_direct!(
        ["dev", "cargo-footprint"],
        "dev cargo-footprint",
        [Daemonless, Offline],
        Reply,
        "cargo_output::footprint"
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
        "gate [--at REV] [--verbose]",
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
        "gate prove [ACTION] [--missing] [--verbose]",
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
        "gate camp [--verbose]",
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
        "gate kill [--verbose]",
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
        "release notes [--release-commit REV] [--output FILE]",
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
        "board [FAMILY] [--limit N] [--max-bytes N] [--max-text N] [--after HANDLE] \
         [--output FILE]",
        OperatorOrLane,
        Read,
        [],
        [Resident],
        Reply,
        "board::read"
    ),
    row!(
        ["board", "graph"],
        "board graph VIEW [--all] [--limit N] [--max-bytes N] [--max-text N] \
         [--output FILE] [--at BORN]",
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
        ["room", "post"],
        "room post HANDLE TEXT",
        OperatorOrLane,
        Command,
        [RoomPublished],
        [Resident],
        Reply,
        "room::post"
    ),
    row!(
        ["room", "history"],
        "room history HANDLE [--contributions] [--limit N] [--max-bytes N] [--after PUBLICATION] \
         [--export FILE]",
        OperatorOrLane,
        Read,
        [],
        [Daemonless],
        Reply,
        "room::history"
    ),
    operator!(
        ["room", "status"],
        "room status HANDLE",
        Read,
        [],
        Reply,
        "room::coordination-read"
    ),
    operator!(
        ["room", "start"],
        "room start HANDLE ADDRESS [--criterion CRITERION] [--contribution CONTRIBUTION] [--stop STOP]",
        Command,
        [SteerSent, RoomCoordinationStarted],
        Reply,
        "room::coordinate"
    ),
    operator!(
        ["room", "resume"],
        "room resume HANDLE ADDRESS [--criterion CRITERION] [--contribution CONTRIBUTION] [--stop STOP]",
        Command,
        [SteerSent, RoomCoordinationStarted],
        Reply,
        "room::coordinate"
    ),
    operator!(
        ["room", "put-aside"],
        "room put-aside HANDLE",
        Command => SnapshotOrCommitted,
        [SteerCanceled, SteerSent, RoomPutAside],
        Reply,
        "room::put-aside"
    ),
    row!(
        ["room", "leave"],
        "room leave HANDLE [--clause TEXT]",
        Lane,
        Command => SnapshotOrCommitted,
        [RoomLeft],
        [Resident, LaneCapability],
        Reply,
        "room::leave"
    ),
    row!(
        ["room", "contribute"],
        "room contribute HANDLE [--charge=C|--ask] [--question N] [--cite KEY...] TEXT",
        Lane,
        Command => SnapshotOrCommitted,
        [RoomContributed],
        [Resident, LaneCapability],
        Reply,
        "room::contribute"
    ),
    row!(
        ["room", "inbox"],
        "room inbox [--limit N] [--max-bytes N] [--after ITEM]",
        OperatorOrLane,
        Read,
        [],
        [Daemonless],
        Reply,
        "room::inbox"
    ),
    operator!(
        ["room", "ack"],
        "room ack ITEM... [--clause TEXT]",
        Command,
        [RoomDisposed],
        Reply,
        "room::control"
    ),
    operator!(
        ["room", "nak"],
        "room nak ITEM... [--clause TEXT]",
        Command,
        [RoomDisposed],
        Reply,
        "room::control"
    ),
    operator!(
        ["room", "reply"],
        "room reply ITEM TEXT",
        Command,
        [SteerSent, RoomPosted, RoomDisposed],
        Reply,
        "room::control"
    ),
    operator!(
        ["room", "recover"],
        "room recover ITEM",
        Command,
        [RoomReceiptRecovered],
        Reply,
        "room::control"
    ),
    operator!(
        ["room", "assess"],
        "room assess OBSERVATION VERDICT TEXT",
        Command,
        [RoomRelevanceAssessed],
        Reply,
        "room::control"
    ),
    row!(
        ["room", "relevance"],
        "room relevance HANDLE [--limit N] [--max-bytes N] [--after OBSERVATION]",
        OperatorOrLane,
        Read,
        [],
        [Daemonless],
        Reply,
        "room::relevance"
    ),
    row!(
        ["room", "awareness"],
        "room awareness HANDLE [--limit N] [--max-bytes N] [--after POSITION]",
        OperatorOrLane,
        Read,
        [],
        [Daemonless],
        Reply,
        "room::awareness"
    ),
    row!(
        ["group", "list"],
        "group list",
        OperatorOrLane,
        Read,
        [],
        [Resident],
        Reply,
        "group::read"
    ),
    operator!(
        ["group", "move"],
        "group move ANCHOR [--to GROUP] [--new]",
        Command,
        [WorkingGroupMoved],
        Reply,
        "group::move"
    ),
    operator!(
        ["group", "arrange"],
        "group arrange GROUPING...",
        Command,
        [WorkingGroupsArranged],
        Reply,
        "group::arrange"
    ),
    operator!(
        ["group", "disband"],
        "group disband GROUP",
        Command,
        [WorkingGroupsArranged],
        Reply,
        "group::disband"
    ),
    operator!(
        ["group", "stance"],
        "group stance GROUP STANCE",
        Command,
        [WorkingGroupsArranged],
        Reply,
        "group::stance"
    ),
    row!(
        ["preparation", "show"],
        "preparation show HANDLE [--limit N] [--max-bytes N] [--after POSITION]",
        OperatorOrLane,
        Read,
        [],
        [Daemonless],
        Reply,
        "preparation::show"
    ),
    row!(
        ["preparation", "propose"],
        "preparation propose [--design HANDLE] [--recommend conclude|collapse] \
         [--cite RELATION=KEY...] [--dry-run] JUSTIFICATION",
        OperatorOrLane,
        Command,
        [DesignProposed],
        [Resident],
        Reply,
        "preparation::propose"
    ),
    row!(
        ["preparation", "critique"],
        "preparation critique CRITIQUE_JSON",
        OperatorOrLane,
        Command,
        [CandidateCritiqued],
        [Resident],
        Reply,
        "preparation::critique"
    ),
    row!(
        ["preparation", "cover"],
        "preparation cover COVERAGE_JSON",
        OperatorOrLane,
        Command,
        [CandidateCovered],
        [Resident],
        Reply,
        "preparation::cover"
    ),
    row!(
        ["preparation", "withdraw"],
        "preparation withdraw WITHDRAWAL_JSON",
        OperatorOrLane,
        Command => SnapshotOrCommitted,
        [CandidateWithdrawn],
        [Resident],
        Reply,
        "preparation::withdraw"
    ),
    operator!(
        ["preparation", "decide"],
        "preparation decide DECISION_JSON",
        Command,
        [PreparationDecided, ExecutionAuthorized, TaskConcluded],
        Reply,
        "preparation::decide"
    ),
    operator!(
        ["preparation", "adopt"],
        "preparation adopt OUTPUT PREPARATION",
        Command,
        [OutputControlled],
        Reply,
        "preparation::adopt"
    ),
    operator!(
        ["preparation", "nudge"],
        "preparation nudge DESIGN",
        Command,
        [SteerSent, RoomPosted, DesignNudged],
        Reply,
        "preparation::nudge"
    ),
    row!(
        ["task", "list"],
        "task list [--all] [--project PROJECT] [--limit N] [--max-bytes N] \
         [--max-text N] [--after HANDLE] [--status pending|waiting|completed|deleted]",
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
        "task show HANDLE [--full] [--context] [--history] [--limit N] [--max-bytes N] \
         [--max-text N] [--after POSITION]",
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
        "task add TITLE [--pitch TEXT] [--project PROJECT] [--description DESCRIPTION] \
         [--acceptance ACCEPTANCE...] [--flow PHASES] [--depends HANDLE...] [--intent INTENT] [--preparation HANDLE] \
         [--requires-journal-schema SCHEMA] [--requires-trust-grant DIGEST] \
         [--origin ORIGIN] [--claim] [--draft]",
        OperatorOrLane,
        Command,
        [TaskAdded, TaskEdited, ClaimWon],
        [Resident, Toolful],
        TASK_GIT_TOOLS,
        Reply,
        "board::task_add"
    ),
    row!(
        ["task", "plan"],
        "task plan TITLE [--pitch TEXT] [--project PROJECT] [--description DESCRIPTION] \
         [--acceptance ACCEPTANCE...] [--flow PHASES] [--depends HANDLE...] [--preparation HANDLE] \
         [--requires-journal-schema SCHEMA] [--requires-trust-grant DIGEST] \
         [--origin ORIGIN] [--claim] [--draft]",
        OperatorOrLane,
        Command,
        [TaskAdded, TaskEdited, ClaimWon],
        [Resident, Toolful],
        TASK_GIT_TOOLS,
        Reply,
        "board::task_add"
    ),
    row!(
        ["task", "design"],
        "task design TITLE [--pitch TEXT] [--project PROJECT] [--description DESCRIPTION] \
         [--acceptance ACCEPTANCE...] [--flow PHASES] [--depends HANDLE...] [--preparation HANDLE] \
         [--requires-journal-schema SCHEMA] [--requires-trust-grant DIGEST] \
         [--origin ORIGIN] [--claim] [--draft]",
        OperatorOrLane,
        Command,
        [TaskAdded, TaskEdited, ClaimWon],
        [Resident, Toolful],
        TASK_GIT_TOOLS,
        Reply,
        "board::task_add"
    ),
    row!(
        ["task", "capture"],
        "task capture [HANDLE] [--title TITLE] [--pitch TEXT] [--project PROJECT] \
         [--origin ORIGIN] [--description DESCRIPTION] [--acceptance ACCEPTANCE...] \
         [--steal] [--restore]",
        OperatorOrLane,
        Command => DirectOrCommitted,
        [ClaimWon],
        [Resident, Toolful],
        TASK_GIT_TOOLS,
        Reply,
        "board::task_capture"
    ),
    row!(
        ["task", "done"],
        "task done [HANDLE] [--validation TEXT] [--operation OPERATION] \
         [--disposition KIND] [--cover REFERENCE]",
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
        [RoomPublished],
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
        "task wake HANDLE [--claim]",
        OperatorOrLane,
        Command,
        [TaskWoken, ClaimWon],
        [Resident, Toolful],
        TASK_GIT_TOOLS,
        Reply,
        "board::task_wake"
    ),
    row!(
        ["task", "file"],
        "task file HANDLE [--into PROJECT] [--preparation HANDLE] [--claim]",
        OperatorOrLane,
        Command,
        [TaskFiled, OutputControlled, ClaimWon],
        [Resident, Toolful],
        TASK_GIT_TOOLS,
        Reply,
        "board::task_file"
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
        "task oops TEXT [--origin ORIGIN] [--project PROJECT]",
        OperatorOrLane,
        Command,
        [TaskAdded],
        [Resident],
        Reply,
        "board::task_oops"
    ),
    row!(
        ["task", "depends"],
        "task depends HANDLE AFTER... [--remove]",
        OperatorOrLane,
        Command,
        [TaskEdited],
        [Resident],
        Reply,
        "board::task_depends"
    ),
    row!(
        ["task", "modify"],
        "task modify HANDLE [--title TITLE] [--pitch TEXT] [--description DESCRIPTION] \
         [--acceptance ACCEPTANCE...] [--priority PRIORITY] [--project PROJECT] \
         [--intent INTENT] [--clear-intent] [--flow PHASES]",
        OperatorOrLane,
        Command,
        [TaskEdited],
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
    operator!(
        ["task", "conclude"],
        "task conclude HANDLE [--done] [--collapse REASON]",
        Command,
        [TaskConcluded],
        Reply,
        "board::task_conclude"
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
        [TaskAdded, TaskEdited, RoomPublished, DocApplied],
        [Resident, DigestOnMutation],
        Plan,
        "board::doc"
    ),
    row!(
        ["doc", "ledger"],
        "doc ledger HANDLE [--max-bytes N] [--offset N] [--digest DIGEST] [--output FILE]",
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
        "lane list [--limit N] [--max-bytes N] [--after LANE]",
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
    row!(
        ["lane", "wake"],
        "lane wake ID",
        Operator,
        Command,
        [LaneSessionDeclared, LaneStarted, ClaimCarried],
        [Resident, OperatorCapability, Toolful],
        OCCUPANT_LAUNCH_TOOLS,
        Reply,
        "lane::wake"
    ),
    operator!(
        ["lane", "model"],
        "lane model ID [MODEL]",
        Command,
        [LaneRetuned, LaneRuntimeRetuned],
        Reply,
        "lane::model"
    ),
    operator!(
        ["lane", "effort"],
        "lane effort ID [EFFORT]",
        Command,
        [LaneEffortRetuned, LaneRuntimeRetuned],
        Reply,
        "lane::effort"
    ),
    operator!(
        ["lane", "fast"],
        "lane fast ID on|off",
        Command,
        [LaneFastRetuned, LaneRuntimeRetuned],
        Reply,
        "lane::fast"
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
    row!(
        ["lane", "import"],
        "lane import ID WORKTREE",
        Operator,
        Command,
        [LaneImported, LaneSessionDeclared, LaneStarted, ClaimCarried],
        [Resident, OperatorCapability, Toolful],
        OCCUPANT_LAUNCH_TOOLS,
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
        ["inbox"],
        "inbox [LANE] [--limit N] [--max-bytes N] [--max-text N] [--after ID]",
        OperatorOrLane,
        Read,
        [],
        [Resident],
        Reply,
        "steer::inbox"
    ),
    row!(
        ["quote"],
        "quote TARGET",
        OperatorOrLane,
        Read,
        [],
        [Resident],
        Reply,
        "quote::read"
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
    operator!(
        ["consent"],
        "consent [--limit N] [--max-bytes N] [--max-text N] [--after ID]",
        Read,
        [],
        Reply,
        "consent::read"
    ),
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
    operator!(
        ["taste"],
        "taste [--name NAME] [--limit N] [--max-bytes N] [--max-text N] [--after CURSOR]",
        Read,
        [],
        Reply,
        "taste::read"
    ),
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
    operator!(
        ["trust"],
        "trust [--limit N] [--max-bytes N] [--after DIGEST]",
        Read,
        [],
        Reply,
        "spice::trust"
    ),
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
    operator!(
        ["principal", "list"],
        "principal list",
        Read,
        [],
        Reply,
        "principal::read"
    ),
    operator!(
        ["principal", "admit"],
        "principal admit [--name NAME] [--email EMAIL] [--operator]",
        Command,
        [PrincipalAdmitted],
        Reply,
        "principal::admit"
    ),
    operator!(
        ["principal", "profile"],
        "principal profile PRINCIPAL [--name NAME] [--email EMAIL]",
        Command,
        [PrincipalProfiled],
        Reply,
        "principal::edit"
    ),
    operator!(
        ["principal", "role", "grant"],
        "principal role grant PRINCIPAL ROLE",
        Command,
        [PrincipalGranted],
        Reply,
        "principal::grant"
    ),
    operator!(
        ["principal", "role", "revoke"],
        "principal role revoke PRINCIPAL ROLE",
        Command,
        [PrincipalRevoked],
        Reply,
        "principal::revoke"
    ),
    operator!(
        ["principal", "deauthorize"],
        "principal deauthorize PRINCIPAL",
        Command,
        [PrincipalDeauthorized],
        Reply,
        "principal::deauthorize"
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
    operator!(
        ["journal"],
        "journal [--limit N] [--max-bytes N] [--max-text N] [--after SEQ]",
        Read,
        [],
        Reply,
        "journal::read"
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
        "thread REF [--limit N] [--max-bytes N] [--max-text N] [--after SEQ]",
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
         [--apply=<digest>]",
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
];
