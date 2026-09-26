//! Build-qualified product identity shared by native version surfaces.

use std::fs::File;
use std::sync::LazyLock;

use wire::manifest::ProductVersion;
use wire::{Digest, Domain, Fault, FaultClass, TaskStamp};

/// Base-52 committer instant of the source commit, injected only by
/// `spice dev xtask -- install`. Every other build is unbound to a commit.
const SOURCE_STAMP: Option<&str> = option_env!("SPICE_SOURCE_STAMP"); // env-policy: allow

/// Identity of the executable this process is running.
#[derive(Debug)]
pub struct Running {
    /// Digest of the executable bytes.
    pub binary: Digest,
    /// Tracked release version, stamped and qualified by `binary`.
    pub product: ProductVersion,
}

static RUNNING: LazyLock<Result<Running, Fault>> = LazyLock::new(capture);

/// The running executable's identity, captured once per process.
///
/// The resident captures it before serving, so an install that later
/// replaces the executable file cannot change what this process reports.
///
/// # Errors
///
/// Reports an environment fault when the running executable could not be
/// resolved or read at capture. No unqualified version is substituted for
/// missing build evidence.
pub fn running() -> Result<&'static Running, Fault> {
    RUNNING.as_ref().map_err(Clone::clone)
}

fn capture() -> Result<Running, Fault> {
    let binary = binary_digest()?;
    let stamp = SOURCE_STAMP.map(|stamp| {
        stamp
            .parse::<TaskStamp>()
            .expect("xtask injects a canonical source stamp")
    });
    let product = qualify(
        &dist::current_product_version()
            .expect("the compiled product version is repository-validated"),
        stamp.as_ref(),
        &binary,
    );
    Ok(Running { binary, product })
}

/// Extend a pre-release with the source stamp, then name the artifact in
/// build metadata.
///
/// A release version is ordered by its core alone: a stamp would make it a
/// pre-release of itself, so a stamp extends only an existing pre-release.
fn qualify(version: &ProductVersion, stamp: Option<&TaskStamp>, binary: &Digest) -> ProductVersion {
    let (base, build) = version
        .as_str()
        .split_once('+')
        .map_or((version.as_str(), None), |(base, build)| {
            (base, Some(build))
        });
    let base = match stamp {
        Some(stamp) if base.contains('-') => format!("{base}.{stamp}"),
        _ => base.to_owned(),
    };
    let build = build.map_or_else(
        || format!("bin.{binary}"),
        |build| format!("{build}.bin.{binary}"),
    );
    ProductVersion::parse(&format!("{base}+{build}"))
        .expect("a canonical version, source stamp, and artifact digest form a canonical version")
}

/// Hash the executable without consulting a repository or retaining its bytes.
fn binary_digest() -> Result<Digest, Fault> {
    let executable = std::env::current_exe().map_err(|error| Fault {
        class: FaultClass::Environment,
        reason: "version::binary-unresolved".to_owned(),
        detail: error.to_string(),
    })?;
    let read = || {
        let file = File::open(&executable)?;
        let limit = file.metadata()?.len();
        Digest::compute_reader(Domain::DistArtifact, file, limit).map(|(_, digest)| digest)
    };
    read().map_err(|error: std::io::Error| Fault {
        class: FaultClass::Environment,
        reason: "version::binary-unreadable".to_owned(),
        detail: format!("{}: {error}", executable.display()),
    })
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::*;

    fn version(text: &str) -> ProductVersion {
        ProductVersion::parse(text).expect("fixture version")
    }

    #[test]
    fn a_source_stamp_orders_prereleases_and_never_demotes_a_release() {
        let binary = Digest::compute(Domain::DistArtifact, b"build");
        let earlier = TaskStamp::for_micros(1_790_000_000_000_000).expect("stamp");
        let later = TaskStamp::for_micros(1_790_000_001_000_000).expect("stamp");
        let dev = version("0.2.0-dev.0+vendor.01");
        let unbound = qualify(&dev, None, &binary);
        let first = qualify(&dev, Some(&earlier), &binary);
        let second = qualify(&dev, Some(&later), &binary);
        assert_eq!(
            first.as_str(),
            format!("0.2.0-dev.0.{earlier}+vendor.01.bin.{binary}")
        );
        assert_eq!(unbound.precedence(&first), Ordering::Less);
        assert_eq!(first.precedence(&second), Ordering::Less);

        let release = version("0.2.0");
        let stamped = qualify(&release, Some(&later), &binary);
        assert_eq!(stamped.as_str(), format!("0.2.0+bin.{binary}"));
        assert_eq!(second.precedence(&stamped), Ordering::Less);
    }
}
