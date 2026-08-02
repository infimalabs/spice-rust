//! Board, task, and authored-document commands over the sole resident door.

use std::io::Read as _;

use board::{Entry, Project, ProjectError, ProjectPolicy, SystemTaskRoute, View};
use bus::{Bus, CommitOutcome};
use clap::ArgMatches;
use journal::{FactCause, GuardError, JournalGuard, JournalRead, JournalWrite, PendingFact};
use spice::repository::Repository;
use wire::body::GitBoundary;
use wire::intent::{
    BoardAnswer, BoardGraph, BoardHandout, BoardRead, BoardTask, ClaimWin, CommandAnswer, DocApply,
    DocLedger, Positioned, TaskAdd, TaskDefer, TaskDepends, TaskDone, TaskNote, TaskOopsAdd,
    TaskRef, TaskReview, TextAnswer,
};
use wire::{
    Actor, ClaimDisposition, ClaimHead, Command, CommandKind, Digest, Domain, Fault, FaultClass,
    Instant, LeaseHead, Name, Outcome, Producer, Refusal, Reply, ReplyPosition, Seq, SeqRange,
    TaskHandle, TaskHead, WireVersion, Witness, WorkflowHead, body,
};

use crate::cli::Contract;

include!("board_command/client.rs");
include!("board_command/mutation.rs");
include!("board_command/progress.rs");
include!("board_command/read_guard.rs");
include!("board_command/tests.rs");
