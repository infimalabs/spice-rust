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

#[derive(Default)]
struct CommandEffects {
    inbox_projection: Option<CanonicalUuid>,
}

impl CommandEffects {
    fn project_inbox(&mut self, lane: CanonicalUuid) {
        assert!(
            self.inbox_projection.replace(lane).is_none(),
            "one command cannot project more than one lane inbox"
        );
    }
}

struct CommandDispatch {
    outcome: Outcome<CommandAnswer>,
    effects: CommandEffects,
}

impl CommandDispatch {
    fn ordinary(outcome: Outcome<CommandAnswer>) -> Self {
        Self {
            outcome,
            effects: CommandEffects::default(),
        }
    }

    fn with_effects(outcome: Outcome<CommandAnswer>, effects: CommandEffects) -> Self {
        Self { outcome, effects }
    }

    fn into_parts(self) -> (Outcome<CommandAnswer>, CommandEffects) {
        (self.outcome, self.effects)
    }
}

pub(crate) const SIMULATED_VENDOR: &str = "simulated";
pub(crate) const SIMULATED_DISPOSITION: &str = "simulated";

fn is_simulated_lane(lane: &wire::LaneResidency) -> bool {
    lane.vendor.as_str() == SIMULATED_VENDOR && lane.disposition == SIMULATED_DISPOSITION
}

mod capture;
mod command_surface;
mod daemon;
mod inbox_projection;
mod lane_recovery;
mod lane_surface;
mod lifecycle;
mod message_surface;
mod policy_surface;
mod read_materializations;
mod settings_surface;
mod shutdown;
mod steering;
pub(crate) mod supervisor;
mod taste_surface;
mod timers;

pub(crate) fn admit_steering_index(journal: &mut journal::Journal) -> CommandResult<()> {
    steering::admit_index(journal)
}

#[cfg(test)]
use message_surface::AgentMessagePublisher;
#[cfg(test)]
use message_surface::CaptureViews;
#[cfg(test)]
use message_surface::MessageState;
use message_surface::{CaptureSurfaces, accept_capture_record};
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
