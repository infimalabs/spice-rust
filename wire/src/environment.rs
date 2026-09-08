//! Closed environmental facts and the requirements compiled over them.
//!
//! A requirement carries no authored evaluator. Its fact discriminant selects
//! both the value type and the comparison implemented here, so admitting a new
//! fact requires adding its semantics to this crate and recompiling every
//! consumer.

use std::fmt;
use std::num::NonZeroU32;

use serde::de::{Deserializer, Error as _};
use serde::{Deserialize, Serialize};

/// A closed kind of environmental fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnvironmentFact {
    /// Finalized schema of the journal authority visible to the evaluator.
    JournalSchema,
}

impl EnvironmentFact {
    /// Every environmental fact, in canonical collection order.
    pub const ALL: &[Self] = &[Self::JournalSchema];

    /// Stable wire spelling of this fact.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::JournalSchema => "journal-schema",
        }
    }
}

impl fmt::Display for EnvironmentFact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One typed environmental requirement.
///
/// The tagged variant is the fact and its `value` field is the sole typed
/// operand. There is deliberately no operator, expression, evaluator name, or
/// nesting point in this shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "fact", rename_all = "kebab-case", deny_unknown_fields)]
pub enum EnvironmentRequirement {
    /// Require an authority whose observed journal schema is at least `value`.
    JournalSchema {
        /// Inclusive minimum journal schema. Zero is not a schema.
        value: NonZeroU32,
    },
}

impl EnvironmentRequirement {
    /// Construct a minimum journal-schema requirement.
    ///
    /// # Errors
    ///
    /// Refuses zero because journal schemas begin at one.
    pub const fn journal_schema(value: u32) -> Result<Self, EnvironmentValueError> {
        match NonZeroU32::new(value) {
            Some(value) => Ok(Self::JournalSchema { value }),
            None => Err(EnvironmentValueError),
        }
    }

    /// Fact this requirement constrains.
    #[must_use]
    pub const fn fact(self) -> EnvironmentFact {
        match self {
            Self::JournalSchema { .. } => EnvironmentFact::JournalSchema,
        }
    }

    /// Whether one same-kind observation satisfies this requirement.
    ///
    /// The comparison is selected entirely by the fact variant. Callers can
    /// provide observations, but cannot replace or parameterize the evaluator.
    #[must_use]
    pub const fn is_satisfied_by(self, observed: EnvironmentObservation) -> bool {
        match (self, observed) {
            (
                Self::JournalSchema { value: required },
                EnvironmentObservation::JournalSchema { value: observed },
            ) => observed.get() >= required.get(),
        }
    }
}

/// A zero value supplied for an environmental fact whose domain begins at one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvironmentValueError;

impl fmt::Display for EnvironmentValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("environment value must be a positive integer")
    }
}

impl std::error::Error for EnvironmentValueError {}

/// The complete observed environment supplied to requirement evaluation.
///
/// Each fact has one private slot, so callers cannot create duplicate or
/// order-sensitive observations. Extending the vocabulary adds another typed
/// slot and another compiled match arm rather than opening an evaluator API.
///
/// identity-law: non-crossing - process-local input to compiled requirement evaluation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Environment {
    journal_schema: Option<NonZeroU32>,
}

impl Environment {
    /// An environment in which no fact was observed.
    pub const EMPTY: Self = Self {
        journal_schema: None,
    };

    /// Observe one finalized journal schema.
    ///
    /// # Errors
    ///
    /// Refuses zero because journal schemas begin at one.
    pub const fn journal_schema(value: u32) -> Result<Self, EnvironmentValueError> {
        match NonZeroU32::new(value) {
            Some(value) => Ok(Self {
                journal_schema: Some(value),
            }),
            None => Err(EnvironmentValueError),
        }
    }

    const fn observation(&self, fact: EnvironmentFact) -> Option<EnvironmentObservation> {
        match fact {
            EnvironmentFact::JournalSchema => match self.journal_schema {
                Some(value) => Some(EnvironmentObservation::JournalSchema { value }),
                None => None,
            },
        }
    }
}

/// One typed observation from the environment being evaluated.
///
/// identity-law: non-crossing - process-local input to compiled requirement evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvironmentObservation {
    /// Finalized journal schema observed from authority.
    JournalSchema {
        /// Positive observed schema value.
        value: NonZeroU32,
    },
}

impl EnvironmentObservation {
    /// Construct an observed journal schema.
    ///
    /// # Errors
    ///
    /// Refuses zero because journal schemas begin at one.
    pub const fn journal_schema(value: u32) -> Result<Self, EnvironmentValueError> {
        match NonZeroU32::new(value) {
            Some(value) => Ok(Self::JournalSchema { value }),
            None => Err(EnvironmentValueError),
        }
    }

    /// Fact this observation describes.
    #[must_use]
    pub const fn fact(self) -> EnvironmentFact {
        match self {
            Self::JournalSchema { .. } => EnvironmentFact::JournalSchema,
        }
    }
}

/// Canonical AND-composed environmental requirements for one task.
///
/// Entries are strictly ordered by [`EnvironmentFact`]. That simultaneously
/// makes the encoding unique and prevents two requirements for one fact from
/// acquiring accidental precedence semantics.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct EnvironmentRequirements(Vec<EnvironmentRequirement>);

impl EnvironmentRequirements {
    /// No environmental requirements.
    pub const EMPTY: Self = Self(Vec::new());

    /// Build a canonical collection in the exact supplied order.
    ///
    /// # Errors
    ///
    /// Refuses duplicate fact kinds and out-of-order entries. It never sorts or
    /// deduplicates caller input because normalization would admit two wire
    /// documents for one authority value.
    pub fn new(requirements: Vec<EnvironmentRequirement>) -> Result<Self, RequirementsError> {
        for pair in requirements.windows(2) {
            let previous = pair[0].fact();
            let current = pair[1].fact();
            if previous == current {
                return Err(RequirementsError::Duplicate(current));
            }
            if previous > current {
                return Err(RequirementsError::OutOfOrder { previous, current });
            }
        }
        Ok(Self(requirements))
    }

    /// Build the necessarily canonical one-entry collection.
    #[must_use]
    pub fn one(requirement: EnvironmentRequirement) -> Self {
        Self(vec![requirement])
    }

    /// Requirements in canonical fact order.
    #[must_use]
    pub fn as_slice(&self) -> &[EnvironmentRequirement] {
        &self.0
    }

    /// Whether no environment fact constrains this task.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether every requirement has a same-kind observation satisfying its
    /// compiled comparison.
    #[must_use]
    pub fn are_satisfied_by(&self, environment: &Environment) -> bool {
        self.0.iter().all(|requirement| {
            environment
                .observation(requirement.fact())
                .is_some_and(|observation| requirement.is_satisfied_by(observation))
        })
    }

    /// Deserialize a present task field.
    ///
    /// Empty collections are represented only by omission on task facts and
    /// heads, so an explicit empty array is a noncanonical spelling.
    pub(crate) fn present<'de, D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let requirements = Self::deserialize(deserializer)?;
        if requirements.is_empty() {
            Err(D::Error::custom(
                "present task requirements must not be empty",
            ))
        } else {
            Ok(requirements)
        }
    }
}

impl<'de> Deserialize<'de> for EnvironmentRequirements {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let requirements = Vec::<EnvironmentRequirement>::deserialize(deserializer)?;
        Self::new(requirements).map_err(D::Error::custom)
    }
}

impl TryFrom<Vec<EnvironmentRequirement>> for EnvironmentRequirements {
    type Error = RequirementsError;

    fn try_from(requirements: Vec<EnvironmentRequirement>) -> Result<Self, Self::Error> {
        Self::new(requirements)
    }
}

impl AsRef<[EnvironmentRequirement]> for EnvironmentRequirements {
    fn as_ref(&self) -> &[EnvironmentRequirement] {
        self.as_slice()
    }
}

/// Why an environmental requirement collection is not canonical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequirementsError {
    /// One fact kind appeared more than once.
    Duplicate(EnvironmentFact),
    /// Fact kinds were not in their declared canonical order.
    OutOfOrder {
        /// Fact immediately preceding the inversion.
        previous: EnvironmentFact,
        /// Fact that should have appeared earlier.
        current: EnvironmentFact,
    },
}

impl fmt::Display for RequirementsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duplicate(fact) => write!(formatter, "duplicate environment fact {fact}"),
            Self::OutOfOrder { previous, current } => write!(
                formatter,
                "environment fact {current} must sort before {previous}"
            ),
        }
    }
}

impl std::error::Error for RequirementsError {}
