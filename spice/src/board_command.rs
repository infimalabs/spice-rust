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

// `mod` rather than `include!`, which reverses the decision EFFORT-1kKrXfRq
// recorded here. That decision's measurements survive the reversal: a child
// module does already read its ancestors' private items, so the guard tests
// below keep calling `claim_plan` and `commit_claim` unchanged, and converting
// a *test* root would be a rename rather than a split, which is why
// `wire/tests/` is left alone.
//
// What it weighed against conversion was that the formatters gate covers a
// fragment regardless, by naming every path it selects on the formatter's own
// command line. That holds, and it is the gate's alone: stock
// `cargo fmt --all --check` stays blind, still open as GATE-1kKs5TnZ, and the
// fragment count kept by hand in that arm's own doc was filed as drifted twice,
// GATE-1kKs7gfb and GATE-1kKsGmbV. `mod` closes all three by construction
// instead of by an enumeration something has to keep current.
mod client;
pub(super) use client::*;
mod client_document;
use client_document::*;
mod followup;
use followup::*;
mod next_action;
use next_action::*;
mod mutation;
use mutation::*;
mod progress;
use progress::*;
mod read_guard;
use read_guard::*;
#[cfg(test)]
mod tests;
