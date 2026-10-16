//! The closed cause a task statement was created under.

use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Digest, Origin, TaskHandle, TaskStamp};

/// What caused a task to exist, immutable across statement revisions.
///
/// A creation fact's envelope [`Origin`] is the open root attribution of its
/// causal chain; this is the closed meaning the task statement keeps. Every
/// variant except [`Self::Parentless`] spells the origin its creation fact
/// carried. Parentless names retained work whose recorded cause resolved to
/// neither a steering directive nor a task, and no creation path mints it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TaskProvenance {
    /// An acknowledged steering directive, `ack:<key>`.
    Steering(String),
    /// A parent task by birth stamp, `task:<stamp>`.
    Task(TaskStamp),
    /// An applied task document by preview digest, `doc:<digest>`.
    Document(Digest),
    /// An operator-authored Draft, `operator:draft`.
    OperatorDraft,
    /// No resolvable cause, `parentless`.
    Parentless,
}

const OPERATOR_DRAFT: &str = "operator:draft";
const PARENTLESS: &str = "parentless";
/// Retired classes whose value was a steering key.
const STEERING_CLASSES: [&str; 3] = ["steering", "steer", "directive"];

/// Text that is not a canonical task provenance, with the spelling to use
/// instead when one is known.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[error("{}", self.describe())]
pub struct TaskProvenanceError {
    written: String,
    canonical: Option<String>,
    authored: bool,
}

impl TaskProvenanceError {
    /// The rejected text as written.
    #[must_use]
    pub fn written(&self) -> &str {
        &self.written
    }

    /// The canonical spelling of the same cause, when the text named one.
    #[must_use]
    pub fn canonical(&self) -> Option<&str> {
        self.canonical.as_deref()
    }

    fn describe(&self) -> String {
        match (&self.canonical, self.authored) {
            (Some(canonical), _) => {
                format!(
                    "task origin {:?} is not canonical; use {canonical}",
                    self.written
                )
            }
            (None, true) => format!(
                "task origin must be ack:<steering-key> or task:<parent-born-stamp>: {:?}",
                self.written
            ),
            (None, false) => format!(
                "task provenance must be ack:<steering-key>, task:<born-stamp>, doc:<digest>, {OPERATOR_DRAFT}, or {PARENTLESS}: {:?}",
                self.written
            ),
        }
    }
}

impl TaskProvenance {
    /// Admit an origin an author names for a new task.
    ///
    /// Only a steering directive or a parent task can be named; a parent may be
    /// written as its birth stamp or any handle carrying it. Document and Draft
    /// causes are minted by their own routes, and parentless is never minted.
    pub fn authored(text: &str) -> Result<Self, TaskProvenanceError> {
        let refuse = |canonical: Option<String>| TaskProvenanceError {
            written: text.to_owned(),
            canonical,
            authored: true,
        };
        let (class, value) = text.split_once(':').ok_or_else(|| refuse(None))?;
        match class {
            "ack" if crate::steering_key(value) => Ok(Self::Steering(value.to_owned())),
            "task" => value
                .parse::<TaskStamp>()
                .ok()
                .or_else(|| value.parse::<TaskHandle>().ok()?.born().ok())
                .map(Self::Task)
                .ok_or_else(|| refuse(None)),
            _ if STEERING_CLASSES.contains(&class) && crate::steering_key(value) => {
                Err(refuse(Some(format!("ack:{value}"))))
            }
            _ => Err(refuse(None)),
        }
    }

    /// The envelope origin a creation fact under this cause carries; a
    /// parentless task has none to mint.
    #[must_use]
    pub fn origin(&self) -> Option<Origin> {
        (!matches!(self, Self::Parentless)).then(|| {
            self.to_string()
                .parse()
                .expect("every non-parentless provenance spells an origin")
        })
    }

    /// The parent task, when one caused this task.
    #[must_use]
    pub const fn parent(&self) -> Option<&TaskStamp> {
        match self {
            Self::Task(parent) => Some(parent),
            _ => None,
        }
    }

    /// The steering key, when a directive caused this task.
    #[must_use]
    pub fn steering_key(&self) -> Option<&str> {
        match self {
            Self::Steering(key) => Some(key),
            _ => None,
        }
    }
}

impl FromStr for TaskProvenance {
    type Err = TaskProvenanceError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let refuse = || TaskProvenanceError {
            written: text.to_owned(),
            canonical: None,
            authored: false,
        };
        match text {
            OPERATOR_DRAFT => return Ok(Self::OperatorDraft),
            PARENTLESS => return Ok(Self::Parentless),
            _ => {}
        }
        match text.split_once(':').ok_or_else(refuse)? {
            ("ack", key) if crate::steering_key(key) => Ok(Self::Steering(key.to_owned())),
            ("task", stamp) => stamp.parse().map(Self::Task).map_err(|_| refuse()),
            ("doc", digest) => digest.parse().map(Self::Document).map_err(|_| refuse()),
            _ => Err(refuse()),
        }
    }
}

impl fmt::Display for TaskProvenance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Steering(key) => write!(formatter, "ack:{key}"),
            Self::Task(parent) => write!(formatter, "task:{parent}"),
            Self::Document(digest) => write!(formatter, "doc:{digest}"),
            Self::OperatorDraft => formatter.write_str(OPERATOR_DRAFT),
            Self::Parentless => formatter.write_str(PARENTLESS),
        }
    }
}

impl Serialize for TaskProvenance {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TaskProvenance {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests;
