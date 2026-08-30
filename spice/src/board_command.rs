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
    ClaimWin, CommandAnswer, DocApply, DocLedger, GateFindings, Positioned, ReviewFollowup,
    TaskAdd, TaskDefer, TaskDelete, TaskDepends, TaskDone, TaskEdit, TaskNote, TaskOopsAdd,
    TaskRef, TaskReview, TaskWake, TasteProposalAdd, TextAnswer,
};
use wire::{
    Actor, ClaimDisposition, ClaimHead, Command, CommandKind, Digest, Domain, Fault, FaultClass,
    Instant, LeaseHead, Name, Outcome, Producer, Refusal, Reply, ReplyPosition, Seq, SeqRange,
    TaskHandle, TaskHead, TaskStamp, WireVersion, Witness, WorkflowHead, body,
};

use crate::cli::Contract;

pub(crate) const AUTOMATIC_PROVISIONAL_SECONDS: i64 = 60;
pub(crate) const AUTOMATIC_HEARTBEAT_SECONDS: i64 = 20;
pub(crate) const AUTOMATIC_ROLLING_SECONDS: i64 = 100;

// `mod` rather than `include!`, which reverses the decision EFFORT-1kKrXfRq
// recorded here. That decision's measurements survive the reversal: a child
// module does already read its ancestors' private items, so the guard tests
// below keep calling `claim_plan` and `commit_claim` unchanged.
//
// A `tests/` root takes one extra precaution rather than an exemption. Every
// file directly under `tests/` is its own crate root, and a root resolves
// `mod foo;` against its own directory, so a bare `mod` there binds the
// sibling integration target `tests/foo.rs` instead of the intended child
// `tests/<root>/foo.rs` -- silently, with no compiler error. Pinning each
// child with `#[path]` keeps it a split rather than a rename, which is how
// `wire/tests/identity_law.rs` converted its four fragments here: a bare
// `mod vocabulary;` in that root would have bound the 18-test sibling
// `wire/tests/vocabulary.rs` rather than the one-test child
// `wire/tests/identity_law/vocabulary.rs` one directory below.
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
mod automatic_lifecycle;
pub(crate) use automatic_lifecycle::{
    AutomaticClaim, claim_automatic, is_automatic_claim, release_automatic,
};
mod mutation;
use mutation::*;
mod progress;
use progress::*;
mod read_guard;
use read_guard::*;

pub(crate) fn parse_origin(
    matches: &ArgMatches,
) -> Result<Option<wire::Origin>, Outcome<CommandAnswer>> {
    matches
        .get_one::<String>("origin")
        .map(|written| {
            written.parse::<wire::Origin>().map_err(|error| {
                refusal(
                    "task::origin-invalid",
                    Some(written.clone()),
                    &error.to_string(),
                )
            })
        })
        .transpose()
}

#[cfg(test)]
mod tests;
