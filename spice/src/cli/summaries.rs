//! One-line help rows for every registered command path.

/// The help row of a path only repository configuration mounts.
pub(super) const MOUNTED_SUMMARY: &str = "Run a command this repository's configuration mounts";

/// The one-line help row of each registered command path, keyed by its
/// space-separated words.
const SUMMARIES: &[(&str, &str)] = &[
    ("app", "Open the console in a native window"),
    (
        "backup",
        "Write a backup bundle of the journal and task artifacts",
    ),
    ("board", "Read the task board, its graphs, and handouts"),
    ("board graph", "Render one board relationship graph view"),
    (
        "board handout",
        "Compose one deterministic board handout for an output",
    ),
    ("config", "Read or change repository configuration"),
    ("consent", "Review and decide pending consent requests"),
    ("consent allow", "Allow one pending consent request"),
    ("consent deny", "Deny one pending consent request"),
    ("consent policy", "Read the tracked consent policy"),
    (
        "demo",
        "Serve the installed appliance from an isolated demo repository",
    ),
    (
        "dev",
        "Run repository Cargo and xtask commands with the admitted Cargo home",
    ),
    ("dev cargo", "Run Cargo with the admitted Cargo home"),
    (
        "dev cargo-footprint",
        "Report this worktree's Cargo output footprint and its bound",
    ),
    (
        "dev cargo-store",
        "Inspect the owned Cargo store, or reclaim expired state",
    ),
    (
        "dev report-page",
        "Read one bounded historical evidence page for the exporter",
    ),
    (
        "dev xtask",
        "Run a repository xtask with the admitted Cargo home",
    ),
    (
        "doc",
        "Apply an authored task document, or read a task's ledger",
    ),
    (
        "doc ledger",
        "Emit one task's canonical authored-document ledger",
    ),
    ("doctor", "Report appliance and repository health"),
    ("down", "Drain the resident and release its lock"),
    (
        "gate",
        "Prove commit checks and run the Git hook entry points",
    ),
    (
        "gate camp",
        "Audit measurements camping at shared fixed bounds",
    ),
    (
        "gate commit-msg",
        "Check a commit message (Git commit-msg hook)",
    ),
    ("gate kill", "Run the mutation kill audit"),
    (
        "gate pre-commit",
        "Check the staged change (Git pre-commit hook)",
    ),
    ("gate prove", "Prove the staged change's gate actions"),
    (
        "gate reference-transaction",
        "Guard reference updates (Git reference-transaction hook)",
    ),
    (
        "gate selection",
        "Run this source repository's strict change selection",
    ),
    ("group", "Group Designs and set each group's stance"),
    (
        "group arrange",
        "Restate the order and membership of every group",
    ),
    (
        "group disband",
        "Disband one group, leaving its Designs ungrouped",
    ),
    ("group list", "List working groups and their stances"),
    ("group move", "Move one Design into a group or a new one"),
    ("group stance", "Set one group's stance: paused or running"),
    ("inbox", "Read pending steering for a lane"),
    (
        "install",
        "Preview, apply, or reverse this repository's installation",
    ),
    (
        "install symbols",
        "Verify installed debug symbols, or symbolicate a sample",
    ),
    ("journal", "Inspect, verify, or reindex the event journal"),
    (
        "journal reindex",
        "Rebuild the derived thread and room-authority indexes",
    ),
    (
        "journal verify",
        "Verify every journal authority plane in full",
    ),
    ("lane", "Add, wake, drain, and list agent lanes"),
    ("lane add", "Seat a new lane in a worktree"),
    (
        "lane drain",
        "End a lane's occupant gracefully, leaving its seat vacant",
    ),
    (
        "lane effort",
        "Read, set, or --clear the effort one lane launches with",
    ),
    ("lane fast", "Turn fast mode on or off for one lane"),
    ("lane import", "Wake one vacant lane into another worktree"),
    (
        "lane list",
        "List lanes with their claims and peer addresses",
    ),
    (
        "lane model",
        "Read, set, or --clear the model one lane launches with",
    ),
    (
        "lane purge",
        "Retire residue of unseated lanes and stale rack extractions",
    ),
    ("lane remove", "Remove one vacant, claim-free lane"),
    ("lane wake", "Launch a new occupant into a vacant lane seat"),
    ("lock", "Report resource locks, or run a command under one"),
    ("lock run", "Run a command while holding a named lock"),
    (
        "preparation",
        "Propose, critique, and decide Design preparations",
    ),
    (
        "preparation adopt",
        "Adopt existing pending work under preparation control",
    ),
    (
        "preparation cover",
        "Record a source-backed review coverage statement",
    ),
    (
        "preparation critique",
        "Record a source-backed critique of a Design proposal",
    ),
    (
        "preparation decide",
        "Record the operator's decision on a Design proposal",
    ),
    (
        "preparation nudge",
        "Push one open Design to proposal and message the best-matching free lane",
    ),
    (
        "preparation propose",
        "Record a Design pass proposal and release its claim",
    ),
    ("preparation show", "Page one task's preparation authority"),
    (
        "preparation withdraw",
        "Withdraw an undecided proposal you authored",
    ),
    (
        "principal",
        "Admit human principals and manage their profiles and roles",
    ),
    ("principal admit", "Admit a new human principal"),
    (
        "principal deauthorize",
        "Retire the capability a principal's admission issued",
    ),
    ("principal list", "List human principals"),
    ("principal profile", "Change a principal's name or email"),
    ("principal role", "Grant or revoke principal roles"),
    ("principal role grant", "Grant one role to a principal"),
    ("principal role revoke", "Revoke one role from a principal"),
    (
        "proof",
        "Seal or replay the compiled target's detached proof",
    ),
    (
        "proof replay",
        "Re-execute and verify the existing detached proof",
    ),
    (
        "quote",
        "Resolve a quote target to its recorded message and author",
    ),
    (
        "recall",
        "Read one lane's continuity account, or every lane's",
    ),
    (
        "release",
        "Write release notes, then seal and publish release packages",
    ),
    ("release notes", "Write release notes for a release commit"),
    ("release publish", "Preview or publish a sealed release"),
    (
        "release seal",
        "Compose the release manifest after proof and record it",
    ),
    ("resource", "Read spice:// resources from a shell"),
    (
        "resource read",
        "Print exactly what a lane's host frame carries for one spice:// URL",
    ),
    ("restore", "Verify or restore a journal backup bundle"),
    (
        "restore verify",
        "Verify a backup bundle without restoring it",
    ),
    ("retro", "Page preserved agent transcripts, newest first"),
    ("room", "Work in task rooms: post, reply, assess, and leave"),
    (
        "room ack",
        "Acknowledge room delivery items as the operator",
    ),
    ("room assess", "Assess one room relevance observation"),
    (
        "room awareness",
        "Page a Design's awareness requests and maturity",
    ),
    (
        "room contribute",
        "Contribute one typed record to an open Design's room",
    ),
    ("room history", "Page one task room's publications"),
    ("room inbox", "Page open room items awaiting the operator"),
    ("room leave", "Stop listening to a room, with a clause"),
    ("room nak", "Decline room delivery items as the operator"),
    ("room post", "Publish one observation to a task's room"),
    ("room recover", "Recover one lapsed room item"),
    ("room relevance", "Page a room's relevance observations"),
    ("room reply", "Reply to a lane's room item as the operator"),
    ("rtk", "Query an agent's RTK command history"),
    ("rtk gain", "Run the RTK gain query for one agent"),
    ("run", "Run one command through supervised-shell routing"),
    (
        "session",
        "Read agent sessions: briefings, turns, and transcripts",
    ),
    ("session briefing", "Print a session briefing"),
    (
        "session commands",
        "Page shell commands from session transcripts",
    ),
    (
        "session commits",
        "Page commits made in session transcripts",
    ),
    (
        "session compactions",
        "Page context compactions, newest first",
    ),
    (
        "session context",
        "Read the transcript around one recorded source position",
    ),
    ("session messages", "Page user and assistant messages"),
    (
        "session phases",
        "Page runs of turns doing the same kind of work",
    ),
    ("session slices", "Page compaction slices, newest first"),
    ("session summary", "Summarize session transcripts"),
    (
        "session sweep",
        "Summarize each compaction window, newest first",
    ),
    (
        "session thread",
        "Report one thread's transcripts, turns, and compactions",
    ),
    (
        "session timeline",
        "Page turns and compactions, newest first",
    ),
    (
        "session tokens",
        "Count token use across session transcripts",
    ),
    ("session turns", "Page turns, newest first"),
    (
        "steer",
        "Send steering to a lane, or cancel, resend, or recover it",
    ),
    ("steer cancel", "Withdraw one live directive"),
    (
        "steer recover",
        "Restore one lapsed directive under its identity",
    ),
    ("steer resend", "Re-deliver one live directive"),
    ("task", "Create, claim, complete, review, and read tasks"),
    ("task add", "Create a task, optionally claiming it"),
    (
        "task artifact",
        "Attach, list, show, remove, and prune task artifacts",
    ),
    ("task artifact add", "Attach one file to a task"),
    ("task artifact list", "Page one task's artifacts"),
    (
        "task artifact prune",
        "Preview or prune expired task artifacts",
    ),
    (
        "task artifact remove",
        "Remove one artifact from an unsettled task",
    ),
    ("task artifact show", "Read part of one task artifact"),
    (
        "task capture",
        "Claim a task, adopting loose commits and dirty work",
    ),
    (
        "task conclude",
        "Conclude an open, unheld Design done or collapsed",
    ),
    ("task defer", "Defer a task until a time"),
    ("task delete", "Withdraw a task with a recorded reason"),
    (
        "task depends",
        "Add or remove dependency edges between tasks",
    ),
    ("task design", "Create a Design, optionally claiming it"),
    ("task done", "Complete the phase your claim holds"),
    (
        "task file",
        "File a Draft as ordinary work, optionally claiming it",
    ),
    ("task free", "Release your claim on a task"),
    ("task list", "Page tasks"),
    ("task modify", "Edit an unsettled task's statement"),
    (
        "task next",
        "Claim the next allocated task, or report the one you hold",
    ),
    ("task note", "Append an observation to a task"),
    ("task oops", "File an appliance friction report as a Plan"),
    ("task plan", "Create a Plan, optionally claiming it"),
    ("task reclaim", "Renew your live claim's lease"),
    (
        "task require",
        "Set a task's trust, schema, appliance, and evidence requirements",
    ),
    ("task review", "Record a review verdict"),
    ("task show", "Read one task's statement, state, and history"),
    ("task status", "Summarize your claim and the board"),
    (
        "task wake",
        "Clear a task's deferral, optionally claiming it",
    ),
    ("taste", "Read tastes and file curation proposals"),
    ("taste add", "File a proposal to add a taste"),
    ("taste drop", "File a proposal to drop a taste"),
    ("thread", "Read the journal facts of one thread"),
    (
        "trust",
        "Grant, pend, or revoke trust in executable configuration",
    ),
    ("trust grant", "Approve one exact executable digest"),
    (
        "trust pend",
        "Quarantine this worktree's executable configuration digest",
    ),
    (
        "trust revoke",
        "Withdraw approval from one exact executable digest",
    ),
    ("up", "Start the resident and print its endpoint"),
    ("version", "Print the appliance and app versions"),
    ("watch", "Snapshot or follow the live transcript"),
];

/// The registered help row of `path`, absent for a path no row names.
pub(super) fn summary(path: &[String]) -> Option<&'static str> {
    SUMMARIES
        .iter()
        .find(|(key, _)| key.split(' ').eq(path.iter().map(String::as_str)))
        .map(|(_, summary)| *summary)
}
