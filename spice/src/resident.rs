use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use bus::{Bus, CommitOutcome, JournalCommit};
use journal::{FactCause, GuardError, JournalGuard, JournalWrite};
use lane::occupant::Seats;
use sill::{Acquisition, LockFile, LockName, Possession};
use spice::repository::Repository;
use tokio::sync::oneshot;
use wire::intent::{CommandAnswer, Intent, RecognitionAnswer, ReleasePublish, StateAnswer};
use wire::{
    Actor, CanonicalUuid, Command, CommandKind, Fault, FaultClass, Instant, Outcome, Producer,
    Refusal, Reply, ReplyPosition, ResidencyHead, ResidentState, Seq, SeqRange, TrustHead,
    TrustState, Witness,
};

type CommandResult<T> = Result<T, Outcome<()>>;

mod capture;
mod command_surface;
mod lane_recovery;
mod lane_surface;
mod policy_surface;
mod shutdown;
mod steering;
mod timers;

use shutdown::{current_residency, drive_shutdown};

include!("resident/runtime.rs");
include!("resident/release.rs");

#[cfg(test)]
#[path = "resident/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "resident/trust_generation.rs"]
mod trust_generation;
