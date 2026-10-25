//! The product version every build derives from its source commit.
//!
//! A version is the commit's calendar instant in UTC: the major is the
//! two-digit year followed by the day of the year (`26298` on 2026-10-25), the
//! minor is the second of that day, and the patch is 0. `dist/VERSION` reads
//! `calendar` on main. A patch line replaces it with its full release version,
//! whose patch counts up from 1, because a patch commit's own instant would
//! move the major and minor. A build that never passes through release staging
//! is marked `-dev`.

use std::fmt;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use wire::manifest::ProductVersion;

use crate::authority::{AuthorityError, tracked_text_line};

const VERSION_TEXT: &str = ::std::include_str!("../VERSION");
const CALENDAR: &str = "calendar";
const DEV: &str = "dev";
const SECONDS_PER_DAY: i64 = 86_400;

/// What `dist/VERSION` tracks: the calendar derivation or a patch line's pin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrackedVersion {
    /// Main: the source commit's instant supplies the whole version.
    Calendar,
    /// A patch line's exact release version.
    Pin(ProductVersion),
}

impl TrackedVersion {
    /// Admit one tracked version: `calendar`, or a canonical
    /// `MAJOR.MINOR.PATCH` with a nonzero patch and neither pre-release nor
    /// build metadata.
    ///
    /// # Errors
    ///
    /// Refuses any other spelling.
    pub fn parse(text: &str) -> Result<Self, TrackedVersionError> {
        if text == CALENDAR {
            return Ok(Self::Calendar);
        }
        let refused = || TrackedVersionError(text.to_owned());
        if text.contains(['-', '+']) || text.ends_with(".0") {
            return Err(refused());
        }
        ProductVersion::parse(text)
            .map(Self::Pin)
            .map_err(|_| refused())
    }

    /// The product version a build of `source` reports: the calendar version
    /// of its instant, or the pin verbatim, with `-dev` on a build outside
    /// release staging. A calendar build unbound to any commit reports
    /// `0.0.0`, below every derived version.
    #[must_use]
    pub fn product(&self, source: Option<SourceInstant>, build: Build) -> ProductVersion {
        let core = match self {
            Self::Calendar => source.map_or_else(
                || "0.0.0".to_owned(),
                |instant| format!("{}.{}.0", instant.major, instant.minor),
            ),
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
            Self::Calendar => formatter.write_str(CALENDAR),
            Self::Pin(pin) => formatter.write_str(pin.as_str()),
        }
    }
}

/// Why one tracked version was refused.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("tracked version is neither calendar nor MAJOR.MINOR.PATCH with a nonzero patch: {0:?}")]
pub struct TrackedVersionError(String);

/// A source commit's calendar instant in UTC: the two-digit year followed by
/// the day of the year, then the second of that day.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SourceInstant {
    major: u64,
    minor: u64,
}

impl SourceInstant {
    /// Place a commit by its committer second.
    ///
    /// # Errors
    ///
    /// Refuses an instant outside 2000 through 2099 UTC, where the two-digit
    /// year would wrap and misorder versions.
    pub fn new(seconds: i64) -> Result<Self, SourceInstantError> {
        let refused = || SourceInstantError(seconds);
        let date = Timestamp::from_second(seconds)
            .map_err(|_| refused())?
            .to_zoned(TimeZone::UTC)
            .date();
        let year = i64::from(date.year());
        if !(2000..=2099).contains(&year) {
            return Err(refused());
        }
        Ok(Self {
            major: ((year - 2000) * 1000 + i64::from(date.day_of_year())).unsigned_abs(),
            minor: seconds.rem_euclid(SECONDS_PER_DAY).unsigned_abs(),
        })
    }
}

/// Why a source commit has no calendar instant.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("committer instant {0}s falls outside 2000 through 2099 UTC")]
pub struct SourceInstantError(i64);

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

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::*;

    fn second(utc: &str) -> i64 {
        utc.parse::<Timestamp>().expect("UTC instant").as_second()
    }

    fn at(utc: &str) -> SourceInstant {
        SourceInstant::new(second(utc)).expect("supported instant")
    }

    fn calendar(utc: &str, build: Build) -> ProductVersion {
        TrackedVersion::Calendar.product(Some(at(utc)), build)
    }

    #[test]
    fn tracked_versions_admit_calendar_or_a_nonzero_patch_pin_only() {
        assert_eq!(
            TrackedVersion::parse("calendar"),
            Ok(TrackedVersion::Calendar)
        );
        assert!(matches!(
            TrackedVersion::parse("26298.34704.1"),
            Ok(TrackedVersion::Pin(_))
        ));
        for refused in [
            "",
            "0",
            "Calendar",
            " calendar",
            "26298.34704",
            "26298.34704.0",
            "26298.34704.1-dev",
            "26298.34704.1+build",
            "v26298.34704.1",
            "26298.34704.1.0",
        ] {
            assert!(TrackedVersion::parse(refused).is_err(), "{refused:?}");
        }
        assert_eq!(
            current_tracked_version().expect("tracked version admits"),
            TrackedVersion::Calendar
        );
    }

    #[test]
    fn calendar_versions_spell_year_day_second_and_dev_marker() {
        for (utc, staged) in [
            ("2026-10-25T09:38:24Z", "26298.34704.0"),
            ("2027-01-05T00:00:00Z", "27005.0.0"),
            ("2028-12-31T23:59:59Z", "28366.86399.0"),
            ("2000-01-01T00:00:00Z", "1.0.0"),
            ("2099-12-31T23:59:59Z", "99365.86399.0"),
        ] {
            assert_eq!(calendar(utc, Build::Staged).as_str(), staged, "{utc}");
            assert_eq!(
                calendar(utc, Build::Dev).as_str(),
                format!("{staged}-dev"),
                "{utc}"
            );
        }
        assert_eq!(
            TrackedVersion::Calendar
                .product(None, Build::Dev)
                .as_str(),
            "0.0.0-dev"
        );
        let pin = TrackedVersion::parse("26298.34704.1").expect("pin");
        assert_eq!(
            pin.product(Some(at("2026-11-02T12:00:00Z")), Build::Staged)
                .as_str(),
            "26298.34704.1"
        );
        assert_eq!(
            pin.product(None, Build::Dev).as_str(),
            "26298.34704.1-dev"
        );
        for refused in ["1999-12-31T23:59:59Z", "2100-01-01T00:00:00Z"] {
            let seconds = second(refused);
            assert_eq!(
                SourceInstant::new(seconds),
                Err(SourceInstantError(seconds)),
                "{refused}"
            );
        }
    }

    #[test]
    fn calendar_versions_order_by_instant_with_dev_before_staged() {
        let ascending = [
            TrackedVersion::Calendar.product(None, Build::Dev),
            ProductVersion::parse("0.1792921104.0").expect("shipped committer-second version"),
            calendar("2026-10-25T09:38:24Z", Build::Dev),
            calendar("2026-10-25T09:38:24Z", Build::Staged),
            TrackedVersion::parse("26298.34704.1")
                .expect("patch pin")
                .product(None, Build::Staged),
            calendar("2026-10-25T09:38:25Z", Build::Dev),
            calendar("2026-10-25T09:38:25Z", Build::Staged),
            calendar("2026-10-25T23:59:59Z", Build::Staged),
            calendar("2026-10-26T00:00:00Z", Build::Staged),
            calendar("2026-12-31T23:59:59Z", Build::Staged),
            calendar("2027-01-01T00:00:00Z", Build::Staged),
        ];
        for pair in ascending.windows(2) {
            assert_eq!(pair[0].precedence(&pair[1]), Ordering::Less, "{pair:?}");
        }
    }
}
