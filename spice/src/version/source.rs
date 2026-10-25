//! The source commit's half of the product version.

use std::fs;
use std::path::Path;

use dist::{Build, SourceInstant};
use wire::manifest::ProductVersion;

/// The build environment variable that carries the derived product version
/// into the compiled executable.
pub const SOURCE_VERSION_ENV: &str = wire::SpiceVariable::ProductVersion.as_str();

/// The product version a `build` of the checkout at `root` reports: its
/// `dist/VERSION` applied to its HEAD commit's calendar instant.
///
/// # Errors
///
/// Refuses an unreadable or malformed `dist/VERSION` and every refusal of
/// [`source_instant`].
pub fn source_version(root: &Path, build: Build) -> Result<ProductVersion, String> {
    let path = root.join("dist/VERSION");
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let tracked = dist::admit_tracked_version(&text).map_err(|error| error.to_string())?;
    Ok(tracked.product(Some(source_instant(root)?), build))
}

/// HEAD's committer instant in the checkout at `root`. Spice task landings
/// keep main's first-parent seconds strictly increasing, so nothing past
/// HEAD is read and a depth-1 checkout suffices.
///
/// # Errors
///
/// Refuses an unreadable repository or commit and an instant outside 2000
/// through 2099 UTC.
pub fn source_instant(root: &Path) -> Result<SourceInstant, String> {
    let repository = gix::open_opts(root, gix::open::Options::isolated())
        .map_err(|error| format!("repository could not open: {error}"))?;
    let head = repository
        .head_commit()
        .map_err(|error| format!("HEAD does not name a readable commit: {error}"))?;
    let seconds = head
        .time()
        .map_err(|error| format!("HEAD has no committer instant: {error}"))?
        .seconds;
    SourceInstant::new(seconds).map_err(|error| error.to_string())
}
