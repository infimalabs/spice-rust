//! The product version every build derives from `dist/VERSION` and its
//! source commit.
//!
//! `dist/VERSION` holds only what a person decides: the major number, or, on a
//! patch line, a full `MAJOR.MINOR.PATCH` pin naming the released minor. The
//! minor otherwise comes from the source commit's instant, and a build that
//! never passes through release staging is marked `-dev`.

use std::fmt;

use wire::manifest::ProductVersion;

use crate::authority::{AuthorityError, tracked_text_line};

const VERSION_TEXT: &str = ::std::include_str!("../VERSION");
/// Same-second first-parent positions one committer second can order.
pub const SOURCE_SECOND_POSITIONS: u64 = 10;
const DEV: &str = "dev";

/// The hand-set part of the product version that `dist/VERSION` tracks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrackedVersion {
    /// The major number; the source commit supplies the minor.
    Major(u64),
    /// A patch line's exact release version.
    Pin(ProductVersion),
}

impl TrackedVersion {
    /// Admit one tracked version: a canonical major number, or a canonical
    /// `MAJOR.MINOR.PATCH` with neither pre-release nor build metadata.
    ///
    /// # Errors
    ///
    /// Refuses any other spelling.
    pub fn parse(text: &str) -> Result<Self, TrackedVersionError> {
        let refused = || TrackedVersionError(text.to_owned());
        if !text.contains('.') {
            if !canonical_number(text) {
                return Err(refused());
            }
            return text.parse().map(Self::Major).map_err(|_| refused());
        }
        if text.contains(['-', '+']) {
            return Err(refused());
        }
        ProductVersion::parse(text)
            .map(Self::Pin)
            .map_err(|_| refused())
    }

    /// The product version a build of `source` reports: `MAJOR.MINOR.0` from
    /// the source minor, or the pin verbatim, with `-dev` on a build outside
    /// release staging. A build unbound to any commit derives minor 0, so it
    /// sorts below every bound build of its major.
    #[must_use]
    pub fn product(&self, source: Option<SourceMinor>, build: Build) -> ProductVersion {
        let core = match self {
            Self::Major(major) => {
                format!("{major}.{}.0", source.map_or(0, |minor| minor.0))
            }
            Self::Pin(pin) => pin.as_str().to_owned(),
        };
        let text = match build {
            Build::Staged => core,
            Build::Dev => format!("{core}-{DEV}"),
        };
        ProductVersion::parse(&text).expect("a derived version is canonical semantic version text")
    }
}

impl fmt::Display for TrackedVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Major(major) => write!(formatter, "{major}"),
            Self::Pin(pin) => formatter.write_str(pin.as_str()),
        }
    }
}

/// Why one tracked version was refused.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("tracked version is neither a canonical major number nor MAJOR.MINOR.PATCH: {0:?}")]
pub struct TrackedVersionError(String);

/// A source commit's minor version: its committer second times
/// [`SOURCE_SECOND_POSITIONS`] plus its position among same-second
/// first-parent ancestors.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SourceMinor(u64);

impl SourceMinor {
    /// Order a commit by its committer second and same-second position.
    ///
    /// # Errors
    ///
    /// Refuses a negative second, a position the second cannot order, and a
    /// minor that overflows.
    pub fn new(seconds: i64, position: u64) -> Result<Self, SourceMinorError> {
        let seconds = u64::try_from(seconds).map_err(|_| SourceMinorError::Negative(seconds))?;
        if position >= SOURCE_SECOND_POSITIONS {
            return Err(SourceMinorError::Positions(seconds));
        }
        seconds
            .checked_mul(SOURCE_SECOND_POSITIONS)
            .and_then(|minor| minor.checked_add(position))
            .map(Self)
            .ok_or(SourceMinorError::Overflow(seconds))
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Why a source commit has no minor.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SourceMinorError {
    #[error("committer instant {0}s precedes the Unix epoch")]
    Negative(i64),
    #[error("more than {SOURCE_SECOND_POSITIONS} first-parent commits share committer second {0}s")]
    Positions(u64),
    #[error("committer instant {0}s overflows the minor version")]
    Overflow(u64),
}

/// Whether a build passed through release staging.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Build {
    /// A `stage-target` payload.
    Staged,
    /// Any other build, marked `-dev`.
    Dev,
}

/// The tracked version compiled into this distribution generation.
///
/// # Errors
///
/// Refuses a malformed `dist/VERSION`.
pub fn current_tracked_version() -> Result<TrackedVersion, AuthorityError> {
    admit_tracked_version(VERSION_TEXT)
}

/// Admit the full text of one `dist/VERSION` file.
///
/// # Errors
///
/// Refuses anything but one LF-terminated canonical tracked version.
pub fn admit_tracked_version(text: &str) -> Result<TrackedVersion, AuthorityError> {
    Ok(TrackedVersion::parse(tracked_text_line(
        text,
        "product version",
    )?)?)
}

fn canonical_number(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'))
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::*;

    fn minor(seconds: i64, position: u64) -> SourceMinor {
        SourceMinor::new(seconds, position).expect("orderable instant")
    }

    #[test]
    fn tracked_versions_admit_a_major_or_a_full_release_pin_only() {
        assert_eq!(TrackedVersion::parse("0"), Ok(TrackedVersion::Major(0)));
        assert_eq!(TrackedVersion::parse("12"), Ok(TrackedVersion::Major(12)));
        assert!(matches!(
            TrackedVersion::parse("0.17900000010.1"),
            Ok(TrackedVersion::Pin(_))
        ));
        for refused in [
            "",
            "01",
            "0.2",
            "0.2.0-dev.0",
            "0.2.0+build",
            "v1",
            " 1",
            "1.0.0.0",
        ] {
            assert!(TrackedVersion::parse(refused).is_err(), "{refused:?}");
        }
        assert_eq!(
            current_tracked_version().expect("tracked version admits"),
            TrackedVersion::Major(0)
        );
    }

    #[test]
    fn a_source_minor_is_its_second_and_same_second_position() {
        assert_eq!(minor(1_790_000_001, 0).get(), 17_900_000_010);
        assert_eq!(minor(1_790_000_001, 9).get(), 17_900_000_019);
        assert_eq!(
            SourceMinor::new(1_790_000_001, SOURCE_SECOND_POSITIONS),
            Err(SourceMinorError::Positions(1_790_000_001))
        );
        assert_eq!(SourceMinor::new(-1, 0), Err(SourceMinorError::Negative(-1)));
        assert!(SourceMinor::new(i64::MAX, 0).is_err());
    }

    #[test]
    fn derived_versions_spell_major_minor_and_dev_marker() {
        let major = TrackedVersion::Major(0);
        let source = Some(minor(1_790_000_001, 0));
        assert_eq!(
            major.product(source, Build::Staged).as_str(),
            "0.17900000010.0"
        );
        assert_eq!(
            major.product(source, Build::Dev).as_str(),
            "0.17900000010.0-dev"
        );
        assert_eq!(major.product(None, Build::Dev).as_str(), "0.0.0-dev");
        let pin = TrackedVersion::parse("0.17900000010.1").expect("pin");
        assert_eq!(
            pin.product(source, Build::Staged).as_str(),
            "0.17900000010.1"
        );
        assert_eq!(
            pin.product(Some(minor(1_790_000_999, 3)), Build::Dev)
                .as_str(),
            "0.17900000010.1-dev"
        );
    }

    #[test]
    fn derived_versions_order_dev_before_staged_and_commits_by_instant() {
        let major = TrackedVersion::Major(0);
        let ascending = [
            ProductVersion::parse("0.2.0-dev.0").expect("old"),
            ProductVersion::parse("0.2.0-dev.0.dWwsmCFhc").expect("old stamped"),
            major.product(Some(minor(1_790_000_001, 0)), Build::Dev),
            major.product(Some(minor(1_790_000_001, 0)), Build::Staged),
            major.product(Some(minor(1_790_000_001, 1)), Build::Dev),
            major.product(Some(minor(1_790_000_001, 1)), Build::Staged),
            major.product(Some(minor(1_790_000_002, 0)), Build::Dev),
            TrackedVersion::parse("0.17900000020.1")
                .expect("patch pin")
                .product(None, Build::Staged),
        ];
        for pair in ascending.windows(2) {
            assert_eq!(pair[0].precedence(&pair[1]), Ordering::Less, "{pair:?}");
        }
    }
}
