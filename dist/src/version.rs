//! The product version every build derives from `dist/VERSION` and its
//! source commit.
//!
//! `dist/VERSION` holds the major number a person decides, `0` on main. The
//! commit's UTC instant supplies the rest: the minor is the two-digit year
//! followed by the day of the year (`26298` on 2026-10-25), and the patch is
//! ten times the second of that day, so every main version ends in 0. A patch
//! line replaces the major with its full release version: its base's version
//! plus 1 through 9, because a patch commit's own instant would move the minor
//! and patch. A build that never passes through release staging is marked
//! `-dev`.

use std::fmt;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use wire::manifest::ProductVersion;

use crate::authority::{AuthorityError, tracked_text_line};

const VERSION_TEXT: &str = ::std::include_str!("../VERSION");
const DEV: &str = "dev";
const SECONDS_PER_DAY: i64 = 86_400;
/// Patch slots per second: the derived patch ends in 0, and a patch line
/// counts its last digit up from 1.
const PATCH_SLOTS: u64 = 10;

/// What `dist/VERSION` tracks: the major number or a patch line's pin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrackedVersion {
    /// Main: the source commit's instant supplies the minor and patch.
    Major(u64),
    /// A patch line's exact release version.
    Pin(ProductVersion),
}

impl TrackedVersion {
    /// Admit one tracked version: a canonical major number, or a canonical
    /// `MAJOR.MINOR.PATCH` without pre-release or build metadata whose patch
    /// is a derived second's patch plus 1 through 9.
    ///
    /// # Errors
    ///
    /// Refuses any other spelling.
    pub fn parse(text: &str) -> Result<Self, TrackedVersionError> {
        let refused = || TrackedVersionError(text.to_owned());
        if !text.contains('.') {
            return canonical_number(text)
                .then(|| text.parse().ok())
                .flatten()
                .map(Self::Major)
                .ok_or_else(refused);
        }
        if text.contains(['-', '+']) {
            return Err(refused());
        }
        let pin = ProductVersion::parse(text).map_err(|_| refused())?;
        let patch: u64 = text
            .rsplit('.')
            .next()
            .and_then(|patch| patch.parse().ok())
            .ok_or_else(refused)?;
        let day_patches = SECONDS_PER_DAY.unsigned_abs() * PATCH_SLOTS;
        if patch.is_multiple_of(PATCH_SLOTS) || patch >= day_patches {
            return Err(refused());
        }
        Ok(Self::Pin(pin))
    }

    /// The product version a build of `source` reports: the major with the
    /// minor and patch of its instant, or the pin verbatim, with `-dev` on a
    /// build outside release staging. A build unbound to any commit reports
    /// `MAJOR.0.0`, below every derived version of its major.
    #[must_use]
    pub fn product(&self, source: Option<SourceInstant>, build: Build) -> ProductVersion {
        let core = match self {
            Self::Major(major) => source.map_or_else(
                || format!("{major}.0.0"),
                |instant| format!("{major}.{}.{}", instant.minor, instant.patch),
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
            Self::Major(major) => write!(formatter, "{major}"),
            Self::Pin(pin) => formatter.write_str(pin.as_str()),
        }
    }
}

/// Why one tracked version was refused.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error(
    "tracked version is neither a canonical major number nor MAJOR.MINOR.PATCH whose patch adds 1 through 9 to a derived patch: {0:?}"
)]
pub struct TrackedVersionError(String);

/// A source commit's UTC instant as a minor and patch: the two-digit year
/// followed by the day of the year, then ten times the second of that day.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SourceInstant {
    minor: u64,
    patch: u64,
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
            minor: ((year - 2000) * 1000 + i64::from(date.day_of_year())).unsigned_abs(),
            patch: seconds.rem_euclid(SECONDS_PER_DAY).unsigned_abs() * PATCH_SLOTS,
        })
    }
}

/// Why a source commit has no version instant.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("committer instant {0}s falls outside 2000 through 2099 UTC")]
pub struct SourceInstantError(i64);

/// Whether a build carries the unmarked release version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Build {
    /// A `stage-target` payload, or the local Windows appliance built from a
    /// checkout equal to HEAD.
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

    fn second(utc: &str) -> i64 {
        utc.parse::<Timestamp>().expect("UTC instant").as_second()
    }

    fn at(utc: &str) -> SourceInstant {
        SourceInstant::new(second(utc)).expect("supported instant")
    }

    fn main(utc: &str, build: Build) -> ProductVersion {
        TrackedVersion::Major(0).product(Some(at(utc)), build)
    }

    fn pin(text: &str) -> ProductVersion {
        match TrackedVersion::parse(text) {
            Ok(TrackedVersion::Pin(pin)) => pin,
            other => panic!("{text} is a pin: {other:?}"),
        }
    }

    #[test]
    fn tracked_versions_admit_a_major_or_a_patch_slot_pin_only() {
        assert_eq!(TrackedVersion::parse("0"), Ok(TrackedVersion::Major(0)));
        assert_eq!(TrackedVersion::parse("12"), Ok(TrackedVersion::Major(12)));
        for admitted in [
            "0.26298.347041",
            "0.26298.347049",
            "1.27005.1",
            "0.26298.863999",
        ] {
            assert_eq!(pin(admitted).as_str(), admitted);
        }
        for refused in [
            "",
            "01",
            "calendar",
            " 0",
            "0.26298",
            "0.26298.347040",
            "0.26298.0",
            "0.26298.864001",
            "0.26298.347041-dev",
            "0.26298.347041+build",
            "v0.26298.347041",
            "0.26298.347041.0",
            "0.026298.347041",
        ] {
            assert!(TrackedVersion::parse(refused).is_err(), "{refused:?}");
        }
        assert_eq!(
            current_tracked_version().expect("tracked version admits"),
            TrackedVersion::Major(0)
        );
    }

    #[test]
    fn main_versions_spell_major_year_day_ten_times_second_and_dev_marker() {
        for (utc, staged) in [
            ("2026-10-25T09:38:24Z", "0.26298.347040"),
            ("2027-01-05T00:00:00Z", "0.27005.0"),
            ("2028-12-31T23:59:59Z", "0.28366.863990"),
            ("2000-01-01T00:00:00Z", "0.1.0"),
            ("2099-12-31T23:59:59Z", "0.99365.863990"),
        ] {
            assert_eq!(main(utc, Build::Staged).as_str(), staged, "{utc}");
            assert_eq!(
                main(utc, Build::Dev).as_str(),
                format!("{staged}-dev"),
                "{utc}"
            );
        }
        assert_eq!(
            TrackedVersion::Major(1)
                .product(Some(at("2026-10-25T09:38:24Z")), Build::Staged)
                .as_str(),
            "1.26298.347040"
        );
        assert_eq!(
            TrackedVersion::Major(0).product(None, Build::Dev).as_str(),
            "0.0.0-dev"
        );
        let patch = TrackedVersion::Pin(pin("0.26298.347041"));
        assert_eq!(
            patch
                .product(Some(at("2026-11-02T12:00:00Z")), Build::Staged)
                .as_str(),
            "0.26298.347041"
        );
        assert_eq!(
            patch.product(None, Build::Dev).as_str(),
            "0.26298.347041-dev"
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
    fn versions_order_by_instant_with_patches_between_their_base_and_the_next_second() {
        let ascending = [
            TrackedVersion::Major(0).product(None, Build::Dev),
            main("2026-10-25T09:38:24Z", Build::Dev),
            main("2026-10-25T09:38:24Z", Build::Staged),
            pin("0.26298.347041"),
            pin("0.26298.347049"),
            main("2026-10-25T09:38:25Z", Build::Dev),
            main("2026-10-25T09:38:25Z", Build::Staged),
            main("2026-10-25T23:59:59Z", Build::Staged),
            main("2026-10-26T00:00:00Z", Build::Staged),
            main("2026-12-31T23:59:59Z", Build::Staged),
            main("2027-01-01T00:00:00Z", Build::Staged),
            TrackedVersion::Major(1).product(None, Build::Staged),
        ];
        for pair in ascending.windows(2) {
            assert_eq!(pair[0].precedence(&pair[1]), Ordering::Less, "{pair:?}");
        }
    }
}
