//! The frozen `Spice-Origin` grammar at the Git boundary.

use std::error::Error;
use std::fmt;
use std::str::FromStr;

use wire::{CanonicalUuid, TaskHandle};

const PREFIX: &str = "Spice-Origin: v1 repo=";
const TASK_FIELD: &str = " task=";

/// A valid `Spice-Origin` v1 trailer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginTrailer {
    repo: CanonicalUuid,
    task: TaskHandle,
}

impl OriginTrailer {
    /// Build a trailer from identities already admitted by WIRE.
    #[must_use]
    pub const fn new(repo: CanonicalUuid, task: TaskHandle) -> Self {
        Self { repo, task }
    }

    /// Repository named by the trailer.
    #[must_use]
    pub const fn repo(&self) -> CanonicalUuid {
        self.repo
    }

    /// Task named by the trailer.
    #[must_use]
    pub const fn task(&self) -> &TaskHandle {
        &self.task
    }
}

impl FromStr for OriginTrailer {
    type Err = OriginTrailerError;

    fn from_str(line: &str) -> Result<Self, Self::Err> {
        let refuse = || OriginTrailerError::new(line);
        let fields = line.strip_prefix(PREFIX).ok_or_else(refuse)?;
        let (repo, task) = fields.split_once(TASK_FIELD).ok_or_else(refuse)?;

        Ok(Self {
            repo: repo.parse().map_err(|_| refuse())?,
            task: task.parse().map_err(|_| refuse())?,
        })
    }
}

impl fmt::Display for OriginTrailer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{PREFIX}{}{TASK_FIELD}{}", self.repo, self.task)
    }
}

/// A line that is not the one admitted `Spice-Origin` v1 spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginTrailerError {
    evidence: String,
}

impl OriginTrailerError {
    fn new(line: &str) -> Self {
        Self {
            evidence: line.to_owned(),
        }
    }

    /// The unresolved line exactly as encountered.
    #[must_use]
    pub fn evidence(&self) -> &str {
        &self.evidence
    }
}

impl fmt::Display for OriginTrailerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Spice-Origin v1 must be `{PREFIX}<repository-uuid>{TASK_FIELD}<KEY-BORN>`; \
             unresolved evidence: {:?}",
            self.evidence
        )
    }
}

impl Error for OriginTrailerError {}

/// A malformed `Spice-Origin` line attributed to one exact commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitMessageError {
    commit: String,
    line: usize,
    trailer: OriginTrailerError,
}

impl CommitMessageError {
    /// Commit whose message contained the malformed line.
    #[must_use]
    pub fn commit(&self) -> &str {
        &self.commit
    }

    /// One-based line number in the commit message.
    #[must_use]
    pub const fn line(&self) -> usize {
        self.line
    }

    /// The unresolved line exactly as encountered.
    #[must_use]
    pub fn evidence(&self) -> &str {
        self.trailer.evidence()
    }
}

impl fmt::Display for CommitMessageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "commit {} has a malformed Spice-Origin trailer at line {}: {}",
            self.commit, self.line, self.trailer
        )
    }
}

impl Error for CommitMessageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.trailer)
    }
}

/// Validate every `Spice-Origin` line in one commit message.
///
/// Unrelated commit-message lines pass through unexamined. The exact
/// `Spice-Origin` key and version-suffixed historical keys are evidence for
/// this gate and must use the exact v1 grammar; unknown and historical forms
/// are returned intact in the error rather than normalized or rewritten.
///
/// # Errors
///
/// Returns the first malformed trailer with its one-based line number and the
/// exact commit identity supplied by the caller.
pub fn validate_commit_message(
    commit: impl Into<String>,
    message: &str,
) -> Result<Vec<OriginTrailer>, CommitMessageError> {
    let commit = commit.into();
    let mut trailers = Vec::new();

    for (index, line) in message.lines().enumerate() {
        if !names_spice_origin(line) {
            continue;
        }
        let trailer = line.parse().map_err(|trailer| CommitMessageError {
            commit: commit.clone(),
            line: index + 1,
            trailer,
        })?;
        trailers.push(trailer);
    }

    Ok(trailers)
}

fn names_spice_origin(line: &str) -> bool {
    let Some((key, _)) = line.split_once(':') else {
        return false;
    };
    let key = key.trim();
    if key.eq_ignore_ascii_case("Spice-Origin") {
        return true;
    }

    key.get(.."Spice-Origin".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Spice-Origin"))
        && key.as_bytes().get("Spice-Origin".len()) == Some(&b'-')
}
