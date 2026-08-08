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

mod client;
pub(crate) use client::*;
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
