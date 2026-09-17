use std::error::Error;
use std::fmt;

use tape::Locus;

use crate::{Dialect, Event};

/// Transcript lifecycle observed on one vendor record.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum TurnMoment {
    /// The record opens a new operator turn.
    Started,
    /// The record belongs to the named current turn without opening it again.
    Observed,
    /// The record closes the current turn.
    Completed,
}

/// Vendor-neutral context shared by every event decoded from one record.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecordContext {
    /// Vendor timestamp spelling, when present.
    pub timestamp: Option<String>,
    /// Vendor turn identity attached to this record, when present.
    pub turn_id: Option<String>,
    /// Lifecycle position represented by this record, when present.
    pub turn: Option<TurnMoment>,
    /// Model context window advertised on this record, when present.
    pub model_context_window: Option<u64>,
    /// The vendor dialog this record opened that waits on an answer, when it
    /// opened one.
    pub dialog: Option<Dialog>,
}

/// A vendor dialog that holds the turn until it is answered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Dialog {
    /// A yes-or-no question, which a consent ruling answers.
    Confirm {
        /// Identity the answer cites.
        id: String,
        /// What the agent is asking permission for.
        title: String,
        /// The question in full.
        message: String,
    },
    /// A choice, text, or editor session nobody away from the lane can compose.
    Prompt {
        /// Identity the answer cites.
        id: String,
    },
}

impl Dialog {
    /// Identity the answer cites.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Confirm { id, .. } | Self::Prompt { id } => id,
        }
    }
}

/// Result of decoding one complete record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Decoded {
    /// Zero or more typed moments supplied by a recognized record.
    Events {
        /// Context shared by the decoded moments.
        context: RecordContext,
        /// Typed moments in vendor order.
        events: Vec<Event>,
    },
    /// The whole record remains untyped and accounted.
    Residue(Residue),
}

impl Decoded {
    /// Typed events, or an empty slice when this record is residue.
    #[must_use]
    pub fn events(&self) -> &[Event] {
        match self {
            Self::Events { events, .. } => events,
            Self::Residue(_) => &[],
        }
    }

    /// Shared record context, when the record was recognized.
    #[must_use]
    pub const fn context(&self) -> Option<&RecordContext> {
        match self {
            Self::Events { context, .. } => Some(context),
            Self::Residue(_) => None,
        }
    }

    /// Accounted residue, when decoding did not type the record.
    #[must_use]
    pub const fn residue(&self) -> Option<&Residue> {
        match self {
            Self::Events { .. } => None,
            Self::Residue(residue) => Some(residue),
        }
    }
}

/// Bytes that one current decoder could not type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Residue {
    /// Decoder that examined the bytes.
    pub dialect: Dialect,
    /// Exact bytes retained by tape.
    pub locus: Locus,
    /// Why no event was emitted.
    pub kind: ResidueKind,
}

impl Residue {
    pub(crate) const fn new(dialect: Dialect, locus: Locus, kind: ResidueKind) -> Self {
        Self {
            dialect,
            locus,
            kind,
        }
    }

    /// Number of source bytes this residue accounts for.
    #[must_use]
    pub const fn accounted_bytes(&self) -> u64 {
        self.locus.length
    }
}

/// Closed reasons a complete input record remains residue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResidueKind {
    /// A required field is absent.
    Absent { field: &'static str },
    /// The record envelope is not a listening record.
    Unknown { discriminator: String },
    /// A field has the wrong representation.
    Malformed {
        field: &'static str,
        expected: &'static str,
    },
    /// JSON ended before one complete value was available.
    Truncated,
    /// A current envelope contains a discriminator this decoder does not own.
    VersionDrift { discriminator: String },
}

/// A locus that does not cite exactly the bytes submitted for decoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LocusLengthError {
    /// Supplied tape locus.
    pub locus: Locus,
    /// Actual submitted byte count.
    pub actual_length: u64,
}

impl fmt::Display for LocusLengthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} does not cover the submitted {} bytes",
            self.locus, self.actual_length
        )
    }
}

impl Error for LocusLengthError {}
