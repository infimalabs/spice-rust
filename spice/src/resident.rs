use std::fs;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::time::Duration;

use bus::{Bus, CommitOutcome, JournalCommit};
use journal::{FactCause, GuardError, JournalGuard, JournalWrite};
use lane::occupant::Seats;
use sill::{Acquisition, LockFile, LockName, Possession};
use spice::repository::Repository;
use tokio::sync::oneshot;
use wire::intent::{
    CommandAnswer, ConsentAnswer, ConsentDecide, Intent, RecognitionAnswer, ReleasePublish,
    StateAnswer, TrustAnswer, TrustRef,
};
use wire::{
    Actor, CanonicalUuid, Command, CommandKind, Fault, FaultClass, Instant, Outcome, Producer,
    Refusal, Reply, ReplyPosition, ResidencyHead, ResidentState, Seq, SeqRange, TrustHead,
    TrustState, Witness,
};

type CommandResult<T> = Result<T, Outcome<()>>;

mod shutdown;

use shutdown::{current_residency, drive_shutdown};

include!("resident/runtime.rs");
include!("resident/authority.rs");
include!("resident/release.rs");
include!("resident/tests.rs");
