//! Named predicates over the subjects a command relies on.
//!
//! A command witnesses only its own subjects. A global journal position is
//! forbidden here: unrelated traffic cannot lose a race because it is not
//! witnessed. Whole-snapshot reads are the only place a global `seq` appears.
//!
//! Environmental requirements are not command witnesses. A witness re-proves
//! transactional authority before one proposed mutation; a requirement asks
//! the allocator whether its current observed environment satisfies the
//! compiled predicate on a pending task. Keeping those vocabularies separate
//! prevents a caller from turning an environmental observation into authority
//! or supplying its own evaluator through a command precondition.

use std::fmt;
use std::str::FromStr;

use serde::de::{Deserializer, Error as DeError};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::envelope::{Actor, Seq};
use crate::head::{ResidentState, TrustState};
use crate::ident::{CanonicalUuid, Digest};
use crate::rooms::{ProposalRef, RoomReceiptRef};

pub(crate) const TASK_STAMP_ALPHABET: &[u8; 52] =
    b"0123456789BCDFGHJKLMNPQRSTVWXYZbcdfghjklmnpqrstvwxyz";
const TASK_STAMP_SPACE: u64 = 52_u64.pow(TaskStamp::WIDTH as u32);

/// A task's sole durable identity: its encoded birth instant.
///
/// Current stamps carry nine base-52 Unix-microsecond digits. Retained
/// eight-digit Unix-millisecond stamps remain readable without rewriting
/// immutable task facts. Collision allocation probes earlier microseconds, so
/// a stamp never claims a future instant. Ordering is deterministic within and
/// across widths; workflow chronology remains journal-sequence authority across
/// generations.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskStamp(String);

/// Text that does not spell a canonical task birth stamp.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("task stamp must be eight or nine characters from the vowel-free base-52 alphabet: {0:?}")]
pub struct TaskStampError(String);

/// An epoch moment that cannot be represented by the requested task-stamp codec.
#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
#[error("task birth moment is outside its base-52 space: {0}")]
pub struct TaskStampMintError(i64);

impl TaskStamp {
    /// Characters in every current task stamp.
    pub const WIDTH: usize = 9;
    /// Characters in retained millisecond task stamps.
    pub const LEGACY_WIDTH: usize = 8;

    /// Mint the current order-preserving stamp for one Unix microsecond.
    pub fn for_micros(epoch_micros: i64) -> Result<Self, TaskStampMintError> {
        let value = u64::try_from(epoch_micros).map_err(|_| TaskStampMintError(epoch_micros))?;
        if value >= TASK_STAMP_SPACE {
            return Err(TaskStampMintError(epoch_micros));
        }
        Ok(Self(Self::encode(value, Self::WIDTH)))
    }

    fn encode(mut value: u64, width: usize) -> String {
        let mut bytes = vec![b'0'; width];
        for byte in bytes.iter_mut().rev() {
            *byte = TASK_STAMP_ALPHABET[(value % 52) as usize];
            value /= 52;
        }
        bytes.into_iter().map(char::from).collect()
    }

    /// The canonical stored spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The Unix microsecond this task was created, as the stamp encodes it.
    #[must_use]
    pub fn micros(&self) -> i64 {
        moment_micros(&self.0).expect("a parsed task stamp names a representable moment")
    }

    /// Order two births by the moment each encodes, then by spelling. Text
    /// order is birth order only within one width; this order also holds
    /// across a retained millisecond stamp and a current microsecond stamp,
    /// including two that name one moment.
    #[must_use]
    pub fn birth_cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.micros()
            .cmp(&other.micros())
            .then_with(|| self.0.cmp(&other.0))
    }
}

/// Decode the Unix microsecond a moment-coded key names: nine base-52 digits
/// for microseconds, or a retained eight-digit spelling for milliseconds.
///
/// Task stamps, steering keys, and kept-job keys share this codec. Text that
/// is not one such key names no moment.
pub(crate) fn moment_micros(text: &str) -> Option<i64> {
    if !matches!(text.len(), TaskStamp::LEGACY_WIDTH | TaskStamp::WIDTH) {
        return None;
    }
    let mut value = 0_u64;
    for byte in text.bytes() {
        let digit = TASK_STAMP_ALPHABET
            .iter()
            .position(|candidate| *candidate == byte)?;
        value = value.checked_mul(52)?.checked_add(digit as u64)?;
    }
    let decoded = i64::try_from(value).ok()?;
    if text.len() == TaskStamp::LEGACY_WIDTH {
        decoded.checked_mul(1_000)
    } else {
        Some(decoded)
    }
}

impl FromStr for TaskStamp {
    type Err = TaskStampError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if matches!(text.len(), Self::LEGACY_WIDTH | Self::WIDTH)
            && text.bytes().all(|byte| TASK_STAMP_ALPHABET.contains(&byte))
        {
            Ok(Self(text.to_owned()))
        } else {
            Err(TaskStampError(text.to_owned()))
        }
    }
}

impl fmt::Display for TaskStamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for TaskStamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TaskStamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod task_stamp_tests {
    use std::collections::BTreeSet;

    use super::{TaskHandle, TaskStamp, moment_micros};

    #[test]
    fn current_and_retained_stamps_share_one_strict_parser() {
        let retained = "1dzGkSrJ".parse::<TaskStamp>().expect("retained stamp");
        let current =
            TaskStamp::for_micros(1_700_000_000_000_000).expect("current microsecond stamp");
        assert_eq!(retained.as_str().len(), TaskStamp::LEGACY_WIDTH);
        assert_eq!(current.as_str().len(), TaskStamp::WIDTH);
        assert_eq!(retained.as_str().parse::<TaskStamp>(), Ok(retained));
        assert_eq!(current.as_str().parse::<TaskStamp>(), Ok(current));
        for refused in ["", "0000000", "0000000000", "0000000A", "00000000A"] {
            assert!(refused.parse::<TaskStamp>().is_err(), "{refused:?}");
        }
    }

    #[test]
    fn moment_decoding_inverts_minting_and_scales_retained_millis() {
        for micros in [
            0,
            1_700_000_000_123_456,
            52_i64.pow(TaskStamp::WIDTH as u32) - 1,
        ] {
            let stamp = TaskStamp::for_micros(micros).expect("mintable moment");
            assert_eq!(moment_micros(stamp.as_str()), Some(micros));
        }
        assert_eq!(moment_micros("00000001"), Some(1_000));
        for refused in ["", "0000000", "0000000000", "0000000A", "00000000A"] {
            assert_eq!(moment_micros(refused), None, "{refused:?}");
        }
    }

    #[test]
    fn current_microsecond_space_is_exact_and_never_wraps() {
        let space = 52_i64.pow(TaskStamp::WIDTH as u32);
        let last = space - 1;
        assert_eq!(space, 2_779_905_883_635_712);
        assert_eq!(
            52_i64.pow((TaskStamp::WIDTH - 1) as u32),
            53_459_728_531_456
        );
        assert_eq!(TaskStamp::for_micros(0).unwrap().as_str(), "000000000");
        assert_eq!(TaskStamp::for_micros(last).unwrap().as_str(), "zzzzzzzzz");
        assert!(TaskStamp::for_micros(-1).is_err());
        assert!(TaskStamp::for_micros(last + 1).is_err());
        let first_out: jiff::Timestamp = "2058-02-02T20:04:43.635712Z"
            .parse()
            .expect("first out-of-range instant");
        assert_eq!(first_out.as_microsecond(), space);
        assert_eq!(
            TaskStamp::for_micros(1_788_810_000_000_000)
                .unwrap()
                .as_str()
                .chars()
                .next(),
            Some('d'),
            "the current Unix era occupies the expected leading digit",
        );
    }

    #[test]
    fn mixed_width_identity_collections_remain_deterministic() {
        let retained = "1dzGkSrJ".parse::<TaskStamp>().unwrap();
        let current = TaskStamp::for_micros(1_700_000_000_000_000).unwrap();
        let rows = BTreeSet::from([current.clone(), retained.clone()]);
        assert_eq!(rows.len(), 2);
        assert!(rows.contains(&retained));
        assert!(rows.contains(&current));
    }

    #[test]
    fn birth_order_follows_the_encoded_moment_across_widths() {
        let retained = |millis: u64| TaskStamp(TaskStamp::encode(millis, TaskStamp::LEGACY_WIDTH));
        // A retained 2030 birth spells lower than a current 2020 birth.
        let later = retained(1_900_000_000_000);
        let earlier = TaskStamp::for_micros(1_580_000_000_000_000).unwrap();
        assert!(later < earlier, "text order disagrees with chronology");
        assert_eq!(later.birth_cmp(&earlier), std::cmp::Ordering::Greater);
        assert_eq!(earlier.birth_cmp(&later), std::cmp::Ordering::Less);

        // Two widths naming one moment still order one way, by spelling.
        let millis = retained(1_760_000_000_000);
        let micros = TaskStamp::for_micros(1_760_000_000_000_000).unwrap();
        assert_eq!(millis.micros(), micros.micros());
        assert_eq!(
            millis.birth_cmp(&micros),
            millis.as_str().cmp(micros.as_str())
        );
        assert_eq!(
            micros.birth_cmp(&millis),
            millis.birth_cmp(&micros).reverse()
        );
        assert_eq!(millis.birth_cmp(&millis), std::cmp::Ordering::Equal);
    }

    #[test]
    fn task_handles_admit_only_retained_or_current_stamps() {
        let retained = "1dzGkSrJ".parse::<TaskStamp>().unwrap();
        let current = TaskStamp::for_micros(1_700_000_000_000_000).unwrap();
        for stamp in [&retained, &current] {
            let handle = TaskHandle::for_project("wire.identity", stamp);
            assert_eq!(handle.as_str().parse::<TaskHandle>(), Ok(handle));
        }
        for refused in [
            "WIRE-aBcD",
            "WIRE-0000000",
            "WIRE-0000000000",
            "WIRE-00000000A",
            "wire-00000000",
        ] {
            assert!(refused.parse::<TaskHandle>().is_err(), "{refused:?}");
        }
    }
}

/// A task handle such as `SPICE-dRyDYZqJJ`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskHandle(String);

/// Text that does not name a task.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("task handle must be `<UPPER-STEM>-<stamp>` with a canonical task stamp: {0:?}")]
pub struct TaskHandleError(String);

impl TaskHandle {
    /// The handle as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Render a task name from its public project and filing standing.
    ///
    /// Unfiled drafts present their standing in place of their project; the
    /// birth stamp keeps the identity unchanged.
    #[must_use]
    pub fn for_filing(project: &str, born: &TaskStamp, filing: crate::TaskFiling) -> Self {
        let source = match filing {
            crate::TaskFiling::Filed => project.rsplit('.').next().unwrap_or(project),
            crate::TaskFiling::Draft => filing.as_str(),
        };
        let stem = source
            .bytes()
            .filter(|byte| byte.is_ascii_alphanumeric())
            .map(|byte| char::from(byte.to_ascii_uppercase()))
            .take(7)
            .collect::<String>();
        let stem = if stem.is_empty() { "TASK" } else { &stem };
        Self(format!("{stem}-{born}"))
    }

    /// Render an ordinary filed task name from its public project.
    #[must_use]
    pub fn for_project(project: &str, born: &TaskStamp) -> Self {
        Self::for_filing(project, born, crate::TaskFiling::Filed)
    }

    /// Recover the birth stamp carried by a rendered handle.
    pub fn born(&self) -> Result<TaskStamp, TaskStampError> {
        self.0
            .rsplit_once('-')
            .map_or(self.0.as_str(), |(_, stamp)| stamp)
            .parse()
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
        let key_ok = key.parse::<TaskStamp>().is_ok();
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

impl Disposition {
    /// The canonical stored spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Acked => "acked",
            Self::Declined => "declined",
            Self::Lapsed => "lapsed",
            Self::DeadLettered => "deadlettered",
            Self::Canceled => "canceled",
        }
    }
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
        task: TaskStamp,
        /// Statement head the command relies on.
        head: Seq,
    },
    /// The workflow head and the phase it currently sits in.
    Workflow {
        /// Subject task.
        task: TaskStamp,
        /// Workflow head the command relies on.
        head: Seq,
        /// Phase the workflow head carries.
        phase: Phase,
    },
    /// The claim head.
    Claim {
        /// Subject task.
        task: TaskStamp,
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
        task: TaskStamp,
        /// Lease head the command relies on.
        head: Seq,
    },
    /// Lane occupancy under a resident generation: who sits in the lane, or
    /// that nobody does.
    Residency {
        /// Bound lane.
        lane: CanonicalUuid,
        /// Live occupant of the lane; absent for a lane nobody sits in, whose
        /// direction waits for its next occupant.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        occupant: Option<CanonicalUuid>,
        /// Resident epoch the command relies on.
        epoch: CanonicalUuid,
    },
    /// A birth stamp the command intends to mint does not yet exist.
    Absence {
        /// Stamp that must be absent.
        task: TaskStamp,
    },
    /// A task exists, with no constraint on its statement revision.
    ///
    /// Appending to a task proves identity without proving a head, because an
    /// append cannot lose to a concurrent edit the way a replacement can.
    Existence {
        /// Stamp that must exist.
        task: TaskStamp,
    },
    /// A dependency edge is absent between two existing tasks.
    Edge {
        /// Dependent task.
        task: TaskStamp,
        /// Task the dependent waits on.
        after: TaskStamp,
    },
    /// The task's current deferral disposition.
    Deferral {
        /// Subject task.
        task: TaskStamp,
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
        task: TaskStamp,
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
    /// A directive is parked in one of the states operator recovery admits.
    Recoverable {
        /// Subject directive.
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
    /// Exact source authority referenced by a room mutation exists.
    Source {
        /// Digest of the canonical [`crate::rooms::RoomSource`] link.
        source: Digest,
    },
    /// The working-group partition's current revision; zero before any
    /// grouping write.
    Group {
        /// Revision the change was observed at.
        revision: u64,
    },
    /// The repository OOPS generation's exact projected head and Design.
    #[serde(rename = "oops-generation")]
    OopsGeneration {
        /// Fact that established the current generation, absent before the
        /// first generation.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        head: Option<Seq>,
        /// Sole current open opportunistic Design, absent between generations.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        design: Option<TaskStamp>,
    },
    /// One proposal-authority subject's current head, or witnessed absence.
    Proposal {
        /// Exact candidate, decision, or withdrawal subject.
        proposal: ProposalRef,
        /// Current authority head, absent before its first recording.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        head: Option<Seq>,
    },
    /// One room's newest publication position, or witnessed absence, so a
    /// command assembled from the room refuses when anything lands after.
    Room {
        /// Room anchor task.
        room: TaskStamp,
        /// Newest publication position, absent before the room's first.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        head: Option<Seq>,
    },
    /// A selected output's latest authorization head, or witnessed absence.
    Authorization {
        /// Gated output whose exact selection is constrained.
        output: TaskStamp,
        /// Latest authorization head, absent before the first decision.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        head: Option<Seq>,
    },
    /// One operator item's current head.
    Delivery {
        /// Exact operator item whose disposition is guarded.
        receipt: RoomReceiptRef,
        /// Current item head, absent before its publication opens it.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        head: Option<Seq>,
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
    /// A birth stamp that must not yet exist.
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
    /// A directive parked in a recoverable failure state.
    Recoverable,
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
    /// Exact source authority referenced by a room mutation.
    Source,
    /// The working-group partition's current revision.
    Group,
    /// The repository OOPS generation's exact head and current Design.
    #[serde(rename = "oops-generation")]
    OopsGeneration,
    /// One candidate, critique, coverage, or decision authority head.
    Proposal,
    /// One room's newest publication position.
    Room,
    /// A selected output's latest authorization head.
    Authorization,
    /// One directed route's semantic receipt head.
    Delivery,
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
        Self::Recoverable,
        Self::Consent,
        Self::Rule,
        Self::Budget,
        Self::Trust,
        Self::Taste,
        Self::Observation,
        Self::Sealed,
        Self::Source,
        Self::Group,
        Self::OopsGeneration,
        Self::Proposal,
        Self::Room,
        Self::Authorization,
        Self::Delivery,
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
            Self::Recoverable { .. } => WitnessKind::Recoverable,
            Self::Consent { .. } => WitnessKind::Consent,
            Self::Rule { .. } => WitnessKind::Rule,
            Self::Budget { .. } => WitnessKind::Budget,
            Self::Trust { .. } => WitnessKind::Trust,
            Self::Taste { .. } => WitnessKind::Taste,
            Self::Observation { .. } => WitnessKind::Observation,
            Self::Sealed { .. } => WitnessKind::Sealed,
            Self::Source { .. } => WitnessKind::Source,
            singleton @ (Self::Group { .. } | Self::OopsGeneration { .. }) => {
                singleton.singleton_kind()
            }
            Self::Proposal { .. } => WitnessKind::Proposal,
            Self::Room { .. } => WitnessKind::Room,
            Self::Authorization { .. } => WitnessKind::Authorization,
            Self::Delivery { .. } => WitnessKind::Delivery,
            Self::Transition { .. } => WitnessKind::Transition,
        }
    }

    const fn singleton_kind(&self) -> WitnessKind {
        match self {
            Self::Group { .. } => WitnessKind::Group,
            Self::OopsGeneration { .. } => WitnessKind::OopsGeneration,
            _ => panic!("only singleton witnesses reach this helper"),
        }
    }
}
