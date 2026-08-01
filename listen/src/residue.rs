use std::error::Error;
use std::fmt;

use tape::Locus;

use crate::{Dialect, Event};

/// Result of decoding one complete record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Decoded {
    /// One or more typed moments supplied by the record.
    Events(Vec<Event>),
    /// The whole record remains untyped and accounted.
    Residue(Residue),
}

impl Decoded {
    /// Typed events, or an empty slice when this record is residue.
    #[must_use]
    pub fn events(&self) -> &[Event] {
        match self {
            Self::Events(events) => events,
            Self::Residue(_) => &[],
        }
    }

    /// Accounted residue, when decoding did not type the record.
    #[must_use]
    pub const fn residue(&self) -> Option<&Residue> {
        match self {
            Self::Events(_) => None,
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
