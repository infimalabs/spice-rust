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

use crate::envelope::Seq;
use crate::ident::CanonicalUuid;

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

/// One named predicate over a single command subject.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
#[non_exhaustive]
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
        /// Claim head the command relies on.
        head: Seq,
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
}
