//! Product identity shared by native version surfaces.

use std::fs::File;
use std::sync::LazyLock;

use serve::canvas::EmbeddedArchive;
use serve::rack::RackError;
use wire::manifest::ProductVersion;
use wire::{Digest, Domain, Fault, FaultClass, TaskStamp};

/// Base-52 source clock with first-parent timestamp ties ordered, injected only by
/// `spice dev xtask -- install`. Every other build is unbound to a commit.
const SOURCE_STAMP: Option<&str> = option_env!("SPICE_SOURCE_STAMP"); // env-policy: allow

/// Identity of the executable this process is running.
#[derive(Debug)]
pub struct Running {
    /// Digest of the executable bytes.
    pub binary: Digest,
    /// Tracked release version, extended by the source stamp.
    pub product: ProductVersion,
}

static PRODUCT: LazyLock<ProductVersion> = LazyLock::new(|| {
    let stamp = SOURCE_STAMP.map(|stamp| {
        stamp
            .parse::<TaskStamp>()
            .expect("xtask injects a canonical source stamp")
    });
    qualify(
        &dist::current_product_version()
            .expect("the compiled product version is repository-validated"),
        stamp.as_ref(),
    )
});

static RUNNING: LazyLock<Result<Running, Fault>> = LazyLock::new(capture);

static CANVAS: LazyLock<Option<Result<EmbeddedArchive, RackError>>> = LazyLock::new(|| {
    #[cfg(feature = "canvas")]
    let linked = Some(::canvas::archive());
    #[cfg(not(feature = "canvas"))]
    let linked: Option<&'static [u8]> = None;
    linked.map(EmbeddedArchive::admit)
});

/// The product version this executable was built as.
///
/// Fixed at compile time, so naming it does not hash the executable the way
/// [`running`] must.
pub fn product() -> &'static ProductVersion {
    &PRODUCT
}

/// The canvas archive linked into this executable, admitted once per process.
///
/// `None` when the build linked no archive: only the release target build and
/// `spice dev xtask -- install` enable the `canvas` feature and link one.
pub fn canvas() -> Option<&'static Result<EmbeddedArchive, RackError>> {
    CANVAS.as_ref()
}

/// The version report: the product version and the app generation it links.
pub fn report() -> String {
    match canvas() {
        Some(Ok(archive)) => format!("{} app {}", product(), archive.generation()),
        _ => product().to_string(),
    }
}

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
    Ok(Running {
        binary,
        product: product().clone(),
    })
}

/// Extend a pre-release with the source stamp.
///
/// A release version is ordered by its core alone: a stamp would make it a
/// pre-release of itself, so a stamp extends only an existing pre-release.
/// The executable digest stays out of the version; `Running::binary` carries it.
fn qualify(version: &ProductVersion, stamp: Option<&TaskStamp>) -> ProductVersion {
    let text = version.as_str();
    let (base, build) = text
        .split_once('+')
        .map_or((text, None), |(base, build)| (base, Some(build)));
    let Some(stamp) = stamp.filter(|_| base.contains('-')) else {
        return version.clone();
    };
    let mut qualified = format!("{base}.{stamp}");
    if let Some(build) = build {
        qualified.push('+');
        qualified.push_str(build);
    }
    ProductVersion::parse(&qualified)
        .expect("a canonical version extended by a source stamp stays canonical")
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
        let earlier = TaskStamp::for_micros(1_790_000_000_000_000).expect("stamp");
        let later = TaskStamp::for_micros(1_790_000_001_000_000).expect("stamp");
        let dev = version("0.2.0-dev.0+vendor.01");
        let unbound = qualify(&dev, None);
        let first = qualify(&dev, Some(&earlier));
        let second = qualify(&dev, Some(&later));
        assert_eq!(first.as_str(), format!("0.2.0-dev.0.{earlier}+vendor.01"));
        assert_eq!(unbound.precedence(&first), Ordering::Less);
        assert_eq!(first.precedence(&second), Ordering::Less);

        let release = version("0.2.0");
        let stamped = qualify(&release, Some(&later));
        assert_eq!(stamped.as_str(), "0.2.0");
        assert_eq!(second.precedence(&stamped), Ordering::Less);
    }
}
