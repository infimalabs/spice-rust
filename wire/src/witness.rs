//! Named predicates over the subjects a command relies on.
//!
//! A command witnesses only its own subjects. A global journal position is
//! forbidden here: unrelated traffic cannot lose a race because it is not
//! witnessed. Whole-snapshot reads are the only place a global `seq` appears.

use std::fmt;
use std::str::FromStr;

use serde::de::{Deserializer, Error as DeError};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::envelope::{Actor, Seq};
use crate::head::{ResidentState, TrustState};
use crate::ident::{CanonicalUuid, Digest};

/// A task handle such as `SPICE-aBcD`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskHandle(String);

/// Text that does not name a task.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("task handle must be `<UPPER-STEM>-<key>` with both parts alphanumeric: {0:?}")]
pub struct TaskHandleError(String);

impl TaskHandle {
    /// The handle as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for TaskHandle {
    type Err = TaskHandleError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let refuse = || TaskHandleError(text.to_owned());
        let (stem, key) = text.split_once('-').ok_or_else(refuse)?;
        let stem_ok = !stem.is_empty()
            && stem
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit());
        let key_ok = !key.is_empty() && key.bytes().all(|byte| byte.is_ascii_alphanumeric());
        if stem_ok && key_ok {
            Ok(Self(text.to_owned()))
        } else {
            Err(refuse())
        }
    }
}

impl fmt::Display for TaskHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for TaskHandle {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TaskHandle {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

/// A workflow phase name, spelled as one lower-case word.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Phase(String);

/// Text that does not name a phase.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("phase must be one lower-case word: {0:?}")]
pub struct PhaseError(String);

impl Phase {
    /// The phase as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Phase {
    type Err = PhaseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_lowercase()) {
            Ok(Self(text.to_owned()))
        } else {
            Err(PhaseError(text.to_owned()))
        }
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for Phase {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Phase {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

/// A named rule, taste, or barrier identity, spelled as one lower-case word.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(String);

/// Text that does not spell a name.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("name must be one lower-case word: {0:?}")]
pub struct NameError(String);

impl Name {
    /// The name as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Name {
    type Err = NameError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_lowercase()) {
            Ok(Self(text.to_owned()))
        } else {
            Err(NameError(text.to_owned()))
        }
    }
}

impl fmt::Display for Name {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for Name {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Name {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

/// The retirement state a directive currently sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Disposition {
    /// Pending and awaiting a typed retirement.
    Live,
    /// Retired by a semantic acknowledgment.
    Acked,
    /// Retired by a reasoned refusal.
    Declined,
    /// Reached expiry without a valid disposition.
    Lapsed,
    /// Parked because its lane could not start for delivery.
    DeadLettered,
    /// Withdrawn by the operator.
    Canceled,
}

/// The disposition a consent request currently sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Answer {
    /// Recorded and awaiting a decision.
    Asked,
    /// Allowed by a named rule or the operator.
    Granted,
    /// Denied by a named rule or the operator.
    Denied,
    /// Pended to the operator while reversible work continued.
    Queued,
}

/// One named predicate over a single command subject.
///
/// Every variant binds the subject it constrains, so no variant can express a
/// bare global journal position. That absence is the liveness law: unrelated
/// traffic cannot lose a race because it is not witnessed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Witness {
    /// The task statement head.
    Task {
        /// Subject task.
        task: TaskHandle,
        /// Statement head the command relies on.
        head: Seq,
    },
    /// The workflow head and the phase it currently sits in.
    Workflow {
        /// Subject task.
        task: TaskHandle,
        /// Workflow head the command relies on.
        head: Seq,
        /// Phase the workflow head carries.
        phase: Phase,
    },
    /// The claim head.
    Claim {
        /// Subject task.
        task: TaskHandle,
        /// Claim head the command relies on, absent before the first win.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        head: Option<Seq>,
    },
    /// The lease head.
    Lease {
        /// Subject task.
        task: TaskHandle,
        /// Lease head the command relies on.
        head: Seq,
    },
    /// Lane occupancy under a resident generation.
    Residency {
        /// Bound lane.
        lane: CanonicalUuid,
        /// Live occupant of the lane.
        occupant: CanonicalUuid,
        /// Resident epoch the command relies on.
        epoch: CanonicalUuid,
    },
    /// A handle the command intends to mint does not yet exist.
    Absence {
        /// Handle that must be absent.
        task: TaskHandle,
    },
    /// A task exists, with no constraint on its statement revision.
    ///
    /// Appending to a task proves identity without proving a head, because an
    /// append cannot lose to a concurrent edit the way a replacement can.
    Existence {
        /// Handle that must exist.
        task: TaskHandle,
    },
    /// A dependency edge is absent between two existing tasks.
    Edge {
        /// Dependent task.
        task: TaskHandle,
        /// Task the dependent waits on.
        after: TaskHandle,
    },
    /// The task's current deferral disposition.
    Deferral {
        /// Subject task.
        task: TaskHandle,
        /// Deferral head the command relies on.
        head: Seq,
        /// Whether the task currently carries a deferral.
        deferred: bool,
    },
    /// The named eligibility rule the allocation was decided under.
    Eligibility {
        /// Rule that selected the task.
        rule: Name,
    },
    /// A review assignment and the reviewer it seated.
    Assignment {
        /// Subject task.
        task: TaskHandle,
        /// Assignment head the command relies on.
        head: Seq,
        /// Reviewer seated under the anti-self-review rule.
        reviewer: Actor,
    },
    /// The exact normalized plan digest a document preview produced.
    Preview {
        /// Digest of the normalized plan.
        digest: Digest,
    },
    /// A named board barrier over the heads a preview read.
    ///
    /// The barrier binds those heads by digest rather than by a global journal
    /// position, so applying a document races only the board it actually read.
    Barrier {
        /// Barrier name the command cites.
        name: Name,
        /// Digest over the task and workflow heads the preview read.
        digest: Digest,
    },
    /// A directive's current retirement state.
    Directive {
        /// Subject directive.
        steer: CanonicalUuid,
        /// Disposition the command relies on.
        disposition: Disposition,
    },
    /// The preceding directive a resend carries lineage from.
    Lineage {
        /// Directive this one descends from.
        steer: CanonicalUuid,
    },
    /// A consent request's current disposition.
    Consent {
        /// Subject request.
        request: CanonicalUuid,
        /// Disposition the command relies on.
        answer: Answer,
    },
    /// The named consent rule that answered.
    Rule {
        /// Rule that produced the first match.
        name: Name,
    },
    /// Remaining budget under a named rule.
    Budget {
        /// Rule the budget meters.
        rule: Name,
        /// Grants remaining before one queued renewal.
        remaining: u32,
    },
    /// An executable surface digest and its current trust state.
    Trust {
        /// Subject executable digest.
        digest: Digest,
        /// Trust state the command relies on.
        standing: TrustState,
    },
    /// A taste's word bag and the tracked configuration that carried it.
    Taste {
        /// Subject taste.
        name: Name,
        /// Digest of the tracked taste configuration.
        digest: Digest,
    },
    /// An observation key and the head that makes recognition idempotent.
    Observation {
        /// Key identifying the observed evidence.
        key: Digest,
        /// Recognition head the command relies on.
        head: Seq,
    },
    /// A sealed proof receipt and the canonical manifest it composed.
    Sealed {
        /// Digest of the detached proof receipt.
        proof: Digest,
        /// Digest of the canonical release manifest.
        manifest: Digest,
    },
    /// The resident lifecycle edge the command requests.
    Transition {
        /// State the resident currently occupies.
        from: ResidentState,
        /// State the command drives it to.
        to: ResidentState,
    },
}

/// The discriminant of a [`Witness`], naming a predicate class without its
/// values.
///
/// The command registry states its law in these terms because a required
/// witness is known at compile time while the value it proves is not.
/// [`crate::WitnessClass`] separately classifies the seven durable witness-head
/// rows. The two vocabularies overlap on assignment, directive, consent,
/// budget, and transition, but neither contains the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WitnessKind {
    /// The task statement head.
    Task,
    /// The workflow head and its phase.
    Workflow,
    /// The claim head.
    Claim,
    /// The lease head.
    Lease,
    /// Lane occupancy under a resident generation.
    Residency,
    /// A handle that must not yet exist.
    Absence,
    /// A task that must exist, with no constraint on its revision.
    Existence,
    /// A dependency edge that must be absent.
    Edge,
    /// The current deferral disposition.
    Deferral,
    /// The named eligibility rule.
    Eligibility,
    /// A review assignment and its reviewer.
    Assignment,
    /// A normalized plan digest.
    Preview,
    /// A named board barrier.
    Barrier,
    /// A directive's retirement state.
    Directive,
    /// A preceding directive in a resend lineage.
    Lineage,
    /// A consent request's disposition.
    Consent,
    /// A named consent rule.
    Rule,
    /// Remaining budget under a named rule.
    Budget,
    /// An executable digest and its trust state.
    Trust,
    /// A taste and its tracked configuration digest.
    Taste,
    /// An observation key and its recognition head.
    Observation,
    /// A sealed proof receipt and canonical manifest.
    Sealed,
    /// A resident lifecycle edge.
    Transition,
}

impl WitnessKind {
    /// Every predicate class, in the order [`Witness`] declares them.
    ///
    /// The set is closed, so a caller sweeping it sweeps the whole of what a
    /// command can be asked to re-prove rather than a list it restated.
    pub const ALL: &[Self] = &[
        Self::Task,
        Self::Workflow,
        Self::Claim,
        Self::Lease,
        Self::Residency,
        Self::Absence,
        Self::Existence,
        Self::Edge,
        Self::Deferral,
        Self::Eligibility,
        Self::Assignment,
        Self::Preview,
        Self::Barrier,
        Self::Directive,
        Self::Lineage,
        Self::Consent,
        Self::Rule,
        Self::Budget,
        Self::Trust,
        Self::Taste,
        Self::Observation,
        Self::Sealed,
        Self::Transition,
    ];
}

impl Witness {
    /// The predicate class this witness proves.
    #[must_use]
    pub const fn kind(&self) -> WitnessKind {
        match self {
            Self::Task { .. } => WitnessKind::Task,
            Self::Workflow { .. } => WitnessKind::Workflow,
            Self::Claim { .. } => WitnessKind::Claim,
            Self::Lease { .. } => WitnessKind::Lease,
            Self::Residency { .. } => WitnessKind::Residency,
            Self::Absence { .. } => WitnessKind::Absence,
            Self::Existence { .. } => WitnessKind::Existence,
            Self::Edge { .. } => WitnessKind::Edge,
            Self::Deferral { .. } => WitnessKind::Deferral,
            Self::Eligibility { .. } => WitnessKind::Eligibility,
            Self::Assignment { .. } => WitnessKind::Assignment,
            Self::Preview { .. } => WitnessKind::Preview,
            Self::Barrier { .. } => WitnessKind::Barrier,
            Self::Directive { .. } => WitnessKind::Directive,
            Self::Lineage { .. } => WitnessKind::Lineage,
            Self::Consent { .. } => WitnessKind::Consent,
            Self::Rule { .. } => WitnessKind::Rule,
            Self::Budget { .. } => WitnessKind::Budget,
            Self::Trust { .. } => WitnessKind::Trust,
            Self::Taste { .. } => WitnessKind::Taste,
            Self::Observation { .. } => WitnessKind::Observation,
            Self::Sealed { .. } => WitnessKind::Sealed,
            Self::Transition { .. } => WitnessKind::Transition,
        }
    }
}
