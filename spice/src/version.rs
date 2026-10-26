//! Product identity shared by native version surfaces.

mod source;

use std::fs::File;
use std::path::Path;
use std::sync::LazyLock;

use serve::canvas::EmbeddedArchive;
use serve::rack::RackError;
use wire::manifest::ProductVersion;
use wire::{Digest, Domain, Fault, FaultClass};

pub use source::{SOURCE_VERSION_ENV, source_instant, source_version};

/// The product version derived from `dist/VERSION` and the source commit,
/// injected by every repository build path. A calendar build unbound to any
/// commit reports `0.0.0`, marked `-dev`.
const SOURCE_VERSION: Option<&str> = option_env!("SPICE_PRODUCT_VERSION"); // env-policy: allow

/// Identity of the executable this process is running.
#[derive(Debug)]
pub struct Running {
    /// Digest of the executable bytes.
    pub binary: Digest,
    /// Product version derived from the tracked version and source commit.
    pub product: ProductVersion,
}

static PRODUCT: LazyLock<ProductVersion> = LazyLock::new(|| {
    SOURCE_VERSION.map_or_else(
        || {
            dist::current_tracked_version()
                .expect("the compiled tracked version is repository-validated")
                .product(None, dist::Build::Dev)
        },
        |version| {
            ProductVersion::parse(version).expect("the build injects a canonical product version")
        },
    )
});

static RUNNING: LazyLock<Result<Running, Fault>> = LazyLock::new(capture);

static CANVAS: LazyLock<Option<Result<EmbeddedArchive, RackError>>> =
    LazyLock::new(|| linked_bytes().map(EmbeddedArchive::admit));

static RACK_ARCHIVE: LazyLock<Option<Digest>> =
    LazyLock::new(|| linked_bytes().map(|bytes| Digest::compute(Domain::DistArtifact, bytes)));

/// The rack archive bytes linked into this executable, if the build linked one.
fn linked_bytes() -> Option<&'static [u8]> {
    #[cfg(feature = "canvas")]
    let linked = Some(::canvas::archive());
    #[cfg(not(feature = "canvas"))]
    let linked: Option<&'static [u8]> = None;
    linked
}

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

/// The digest of the rack archive bytes this executable links, hashed once
/// per process: the identity a `rack::admitted` fact names.
pub fn rack_archive() -> Option<&'static Digest> {
    RACK_ARCHIVE.as_ref()
}

/// The version report: the product version and the app generation it links.
pub fn report() -> String {
    match canvas() {
        Some(Ok(archive)) => format!("{} app {}", product(), archive.generation()),
        _ => product().to_string(),
    }
}

/// The app generation `spice version` reports.
///
/// An executable that links an archive names that archive's generation and
/// reads nothing from the worktree, as `--version` and the installed resident
/// do. Only a build that links none, which is a development build, names the
/// generation staged in `worktree`.
///
/// # Errors
///
/// Refuses a linked archive that failed admission and, for a development
/// build, an absent or malformed staged app.
pub fn app_generation(worktree: &Path) -> Result<Digest, String> {
    match canvas() {
        Some(Ok(archive)) => Ok(archive.generation().clone()),
        Some(Err(error)) => Err(error.to_string()),
        None => serve::AppSource::resolve(worktree)
            .and_then(|source| source.current_generation())
            .map(|generation| generation.identity().clone())
            .map_err(|error| error.to_string()),
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
