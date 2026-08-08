//! Board, task, and authored-document commands over the sole resident door.

use std::io::Read as _;

use board::{Entry, Project, ProjectError, ProjectPolicy, SystemTaskRoute, View};
use bus::{Bus, CommitOutcome};
use clap::ArgMatches;
use journal::{FactCause, GuardError, JournalGuard, JournalRead, JournalWrite, PendingFact};
use spice::repository::Repository;
use wire::body::GitBoundary;
use wire::intent::{
    BoardAnswer, BoardGraph, BoardHandout, BoardProjectPolicy, BoardRead, BoardTask, ClaimRenew,
    ClaimWin, CommandAnswer, DocApply, DocLedger, Positioned, ReviewFollowup, TaskAdd, TaskDefer,
    TaskDelete, TaskDepends, TaskDone, TaskEdit, TaskNote, TaskOopsAdd, TaskRef, TaskReview,
    TextAnswer,
};
use wire::{
    Actor, ClaimDisposition, ClaimHead, Command, CommandKind, Digest, Domain, Fault, FaultClass,
    Instant, LeaseHead, Name, Outcome, Producer, Refusal, Reply, ReplyPosition, Seq, SeqRange,
    TaskHandle, TaskHead, WireVersion, Witness, WorkflowHead, body,
};

use crate::cli::Contract;

// These are source inclusions rather than `mod` declarations by decision,
// measured rather than assumed. `mod` would not cost the tests below their reach: a child module
// already reads its ancestors' private items, so `claim_plan` and
// `commit_claim` would stay callable. What the single namespace buys is
// narrower than it looks, and what converting costs is 126 items promoted to
// `pub(super)` and 25 fragments opened with `use super::*`, counted across this
// root, its test fragments, `resident` and `task_git`. What source inclusion
// costs is that `cargo fmt` walks modules and so exits zero over a drifted fragment;
// the formatters gate closes exactly that by naming every path it selects on
// the formatter's own command line, so a fragment is covered for being selected
// rather than for being reachable: the set the arm counts and the set the
// formatter opens are one set, and no fragment can drift unseen. The arms that
// walk the tree reach a fragment by path like any other file.
// Converting is also not a rename: a crate root resolves child modules against
// the directory holding the root file, so `mod vocabulary;` in
// `wire/tests/identity_law.rs` binds the 18-test `wire/tests/vocabulary.rs`
// rather than the one-test file beside it, with no compiler error.
include!("board_command/client.rs");
include!("board_command/client_document.rs");
include!("board_command/followup.rs");
include!("board_command/next_action.rs");
include!("board_command/mutation.rs");
include!("board_command/progress.rs");
include!("board_command/read_guard.rs");
include!("board_command/tests.rs");
