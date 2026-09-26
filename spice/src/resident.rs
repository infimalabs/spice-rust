use std::collections::VecDeque;
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
    inbox_delivery: Option<CanonicalUuid>,
}

impl CommandEffects {
    fn notify_inbox(&mut self, lane: CanonicalUuid) {
        assert!(
            self.inbox_delivery.replace(lane).is_none(),
            "one command cannot notify more than one lane inbox"
        );
    }
}

enum InboxDeliveries {
    Exact(VecDeque<CanonicalUuid>),
    Full { witness: Option<Seq>, next: usize },
}

enum InboxDelivery {
    Exact(CanonicalUuid),
    Full {
        lane: CanonicalUuid,
        witness: Seq,
        last: bool,
    },
}

impl InboxDelivery {
    fn lane(&self) -> CanonicalUuid {
        match self {
            Self::Exact(lane) | Self::Full { lane, .. } => *lane,
        }
    }
}

impl InboxDeliveries {
    fn new() -> Self {
        Self::Exact(VecDeque::with_capacity(
            canvas::axum::Control::FAIR_CYCLE_LIMIT,
        ))
    }

    fn is_empty(&self) -> bool {
        matches!(self, Self::Exact(lanes) if lanes.is_empty())
    }

    fn insert(&mut self, lane: CanonicalUuid) {
        if let Self::Full { witness, next } = self {
            *witness = None;
            *next = 0;
            return;
        }
        let Self::Exact(lanes) = self else {
            unreachable!("full delivery state returned above")
        };
        if !lanes.contains(&lane) {
            if lanes.len() == canvas::axum::Control::FAIR_CYCLE_LIMIT {
                *self = Self::Full {
                    witness: None,
                    next: 0,
                };
            } else {
                lanes.push_back(lane);
            }
        }
    }

    fn next(&mut self, residency: &journal::JournalHead<ResidencyHead>) -> Option<InboxDelivery> {
        match self {
            Self::Exact(lanes) => lanes.pop_front().map(InboxDelivery::Exact),
            Self::Full { witness, next } => {
                if *witness != Some(residency.position()) {
                    *witness = Some(residency.position());
                    *next = 0;
                }
                let Some(lane) = residency.body().lanes.get(*next).map(|lane| lane.lane) else {
                    *self = Self::new();
                    return None;
                };
                Some(InboxDelivery::Full {
                    lane,
                    witness: residency.position(),
                    last: *next + 1 == residency.body().lanes.len(),
                })
            }
        }
    }

    fn complete(&mut self, delivery: InboxDelivery) {
        match delivery {
            InboxDelivery::Exact(_) => {}
            InboxDelivery::Full { witness, last, .. } => {
                let Self::Full {
                    witness: current,
                    next,
                } = self
                else {
                    unreachable!("a full delivery turn completes against full state")
                };
                debug_assert_eq!(*current, Some(witness));
                if last {
                    *self = Self::new();
                } else {
                    *next += 1;
                }
            }
        }
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
mod consent_surface;
mod daemon;
mod lane_recovery;
mod lane_surface;
mod lifecycle;
mod log;
mod message_surface;
mod narration;
mod policy_surface;
mod read_materializations;
mod settings_surface;
mod shutdown;
mod steering;
pub(crate) mod supervisor;
mod taste_surface;
mod timers;
mod vendor_surface;

pub(crate) fn admit_steering_index(journal: &mut journal::Journal) -> CommandResult<()> {
    steering::admit_index(journal)
}

#[cfg(test)]
use message_surface::AgentMessagePublisher;
#[cfg(test)]
use message_surface::CaptureViews;
#[cfg(test)]
use message_surface::MessageState;
pub(crate) use message_surface::quote_agent_prose;
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
