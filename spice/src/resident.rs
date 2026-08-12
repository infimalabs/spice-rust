use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use bus::{Bus, CommitOutcome, JournalCommit};
use journal::{FactCause, GuardError, JournalGuard, JournalWrite};
use lane::occupant::Seats;
use sill::{Acquisition, LockFile, LockName, Possession};
use spice::repository::Repository;
use spice::resident_lock::holder_record;
use tokio::sync::oneshot;
use wire::intent::{CommandAnswer, Intent, RecognitionAnswer, ReleasePublish, StateAnswer};
use wire::{
    Actor, CanonicalUuid, Command, CommandKind, Fault, FaultClass, Instant, Outcome, Producer,
    Refusal, Reply, ReplyPosition, ResidencyHead, ResidentState, Seq, SeqRange, TrustHead,
    TrustState, Witness,
};

type CommandResult<T> = Result<T, Outcome<()>>;

pub(crate) const SIMULATED_VENDOR: &str = "simulated";
pub(crate) const SIMULATED_DISPOSITION: &str = "simulated";

fn is_simulated_lane(lane: &wire::LaneResidency) -> bool {
    lane.vendor.as_str() == SIMULATED_VENDOR && lane.disposition == SIMULATED_DISPOSITION
}

mod capture;
mod command_surface;
mod daemon;
mod effort_surface;
mod lane_recovery;
mod lane_surface;
mod lifecycle;
mod message_surface;
mod policy_surface;
mod shutdown;
mod steering;
mod taste_surface;
mod timers;

pub(crate) fn admit_steering_index(journal: &mut journal::Journal) -> CommandResult<()> {
    steering::admit_index(journal)
}

#[cfg(test)]
use message_surface::AgentMessagePublisher;
#[cfg(test)]
use message_surface::CaptureViews;
use message_surface::{CallPreviews, CaptureSurfaces, accept_capture_record};
use shutdown::{current_residency, drive_shutdown};

mod runtime;
pub(crate) use daemon::*;
pub(super) use runtime::*;
mod ingest_surface;
use ingest_surface::*;
mod release;
use release::*;

#[cfg(test)]
#[path = "resident/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "resident/trust_generation.rs"]
mod trust_generation;
