use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

static APP_HTML: &[u8] = include_bytes!("../../dist/stage/canvas-app.html");
static APP_JAVASCRIPT: &[u8] = include_bytes!("../../dist/stage/app.module");
static APP_STYLESHEET: &[u8] = include_bytes!("../../dist/stage/style.css");
static APP_WASM: &[u8] = include_bytes!("../../dist/stage/app_bg.wasm");

static APP_ASSETS: [EmbeddedAsset; 4] = [
    EmbeddedAsset::new("/", "canvas-app.html", APP_HTML, "text/html; charset=utf-8"),
    EmbeddedAsset::new(
        "/app.module",
        "app.module",
        APP_JAVASCRIPT,
        "text/javascript; charset=utf-8",
    ),
    EmbeddedAsset::new(
        "/style.css",
        "style.css",
        APP_STYLESHEET,
        "text/css; charset=utf-8",
    ),
    EmbeddedAsset::new("/app_bg.wasm", "app_bg.wasm", APP_WASM, "application/wasm"),
];

/// Where a locally built app stage is read from, relative to the worktree.
///
/// `dist/stage` beside it is a committed build product only the sealed Linux
/// image may write, because Cargo folds the host triple into `-Cmetadata` and
/// no other environment can reproduce those bytes. This directory is the
/// untracked neighbour that any host may write, so a lane can see an app edit
/// without a round trip through that image.
pub const LOCAL_STAGE: &str = "dist/stage-local";

/// One exact browser asset compiled into the native canvas.
#[derive(Clone, Copy)]
pub struct EmbeddedAsset {
    path: &'static str,
    file: &'static str,
    bytes: &'static [u8],
    content_type: &'static str,
}

impl EmbeddedAsset {
    const fn new(
        path: &'static str,
        file: &'static str,
        bytes: &'static [u8],
        content_type: &'static str,
    ) -> Self {
        Self {
            path,
            file,
            bytes,
            content_type,
        }
    }

    #[must_use]
    pub const fn path(self) -> &'static str {
        self.path
    }

    /// The staged file this route is read from, under either source.
    ///
    /// One inventory names both, so a directory stage cannot be read from a
    /// file name the embedded build never had.
    #[must_use]
    pub const fn file(self) -> &'static str {
        self.file
    }

    #[must_use]
    pub const fn bytes(self) -> &'static [u8] {
        self.bytes
    }

    #[must_use]
    pub const fn content_type(self) -> &'static str {
        self.content_type
    }

    #[must_use]
    pub fn digest(self) -> wire::Digest {
        wire::Digest::compute(wire::Domain::DistArtifact, self.bytes)
    }
}

impl fmt::Debug for EmbeddedAsset {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EmbeddedAsset")
            .field("path", &self.path)
            .field("bytes", &self.bytes.len())
            .field("content_type", &self.content_type)
            .field("digest", &self.digest())
            .finish()
    }
}

/// Browser artifact embedded into this native canvas build.
#[derive(Clone, Copy)]
pub struct EmbeddedApp {
    assets: &'static [EmbeddedAsset],
}

/// The exact staged app bytes compiled into this canvas.
pub const EMBEDDED_APP: EmbeddedApp = EmbeddedApp {
    assets: &APP_ASSETS,
};

impl EmbeddedApp {
    /// Exact embedded bytes returned by the loopback endpoint.
    #[must_use]
    pub const fn bytes(self) -> &'static [u8] {
        self.assets[0].bytes
    }

    /// Content identity under the same domain the distribution stage uses.
    #[must_use]
    pub fn digest(self) -> wire::Digest {
        self.assets[0].digest()
    }

    /// Media type for the fallback shell or an opaque staged bundle.
    #[must_use]
    pub fn content_type(self) -> &'static str {
        self.assets[0].content_type
    }

    /// Every exact staged asset in its closed route order.
    #[must_use]
    pub const fn assets(self) -> &'static [EmbeddedAsset] {
        self.assets
    }

    /// Resolve one exact route. Unknown paths have no shell fallback.
    #[must_use]
    pub fn asset(self, path: &str) -> Option<EmbeddedAsset> {
        self.assets.iter().copied().find(|asset| asset.path == path)
    }
}

impl fmt::Debug for EmbeddedApp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EmbeddedApp")
            .field("assets", &self.assets)
            .field("digest", &self.digest())
            .finish()
    }
}

/// Where one canvas generation reads the browser app it uses.
///
/// Selection is a decision this value settles once, at startup, rather than a
/// compile-time alternative: both answers come out of [`AppSource::resolve`],
/// which reads the worktree it is handed, so a lane and a release differ in
/// what that call returns and not in which code was compiled.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppSource {
    /// The exact bytes this binary was compiled with.
    Embedded,
    /// A locally built stage, re-read from disk on every request.
    Directory(PathBuf),
}

impl AppSource {
    /// Decide which app a canvas rooted at `worktree` uses.
    ///
    /// A worktree with no [`LOCAL_STAGE`] directory uses the embedded bytes,
    /// which is every release and every lane that has not built one. A
    /// directory that exists is taken to be what the operator meant, so one
    /// that cannot supply the whole app refuses here rather than answering some
    /// routes from a local build and the rest from the compiled bytes -- a
    /// mixture is the one outcome nobody could have asked for.
    ///
    /// # Errors
    ///
    /// Returns [`AppSourceError::Incomplete`] naming every missing file when
    /// the directory exists without the full app.
    pub fn resolve(worktree: &Path) -> Result<Self, AppSourceError> {
        let directory = worktree.join(LOCAL_STAGE);
        if !directory.is_dir() {
            return Ok(Self::Embedded);
        }
        let missing = EMBEDDED_APP
            .assets()
            .iter()
            .map(|asset| asset.file())
            .filter(|file| !directory.join(file).is_file())
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return Ok(Self::Directory(directory));
        }
        Err(AppSourceError::Incomplete { directory, missing })
    }

    /// Resolve one exact route. Unknown paths have no shell fallback.
    ///
    /// The directory answer is read per request rather than at startup, so an
    /// app rebuilt underneath a running canvas is returned on the next reload
    /// without the resident being restarted.
    ///
    /// # Errors
    ///
    /// Returns [`AppSourceError::Unreadable`] when a directory stage named a
    /// file at startup that cannot be read now.
    pub fn asset(&self, route: &str) -> Result<Option<ResolvedAsset>, AppSourceError> {
        let Some(asset) = EMBEDDED_APP.asset(route) else {
            return Ok(None);
        };
        let bytes = match self {
            Self::Embedded => Cow::Borrowed(asset.bytes()),
            Self::Directory(directory) => {
                let path = directory.join(asset.file());
                Cow::Owned(
                    std::fs::read(&path)
                        .map_err(|source| AppSourceError::Unreadable { path, source })?,
                )
            }
        };
        Ok(Some(ResolvedAsset {
            bytes,
            content_type: asset.content_type(),
        }))
    }
}

impl fmt::Display for AppSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Embedded => formatter.write_str("compiled app"),
            Self::Directory(directory) => {
                write!(formatter, "local app at {}", directory.display())
            }
        }
    }
}

/// One asset resolved for one request, from whichever source supplied it.
pub struct ResolvedAsset {
    bytes: Cow<'static, [u8]>,
    content_type: &'static str,
}

impl ResolvedAsset {
    /// Media type of the route that produced these bytes.
    #[must_use]
    pub const fn content_type(&self) -> &'static str {
        self.content_type
    }

    /// Content identity under the domain the distribution stage uses.
    #[must_use]
    pub fn digest(&self) -> wire::Digest {
        wire::Digest::compute(wire::Domain::DistArtifact, &self.bytes)
    }

    /// The bytes themselves, still borrowed when they were compiled in.
    #[must_use]
    pub fn into_bytes(self) -> Cow<'static, [u8]> {
        self.bytes
    }
}

impl fmt::Debug for ResolvedAsset {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedAsset")
            .field("bytes", &self.bytes.len())
            .field("content_type", &self.content_type)
            .field("digest", &self.digest())
            .finish()
    }
}

/// Why a canvas could not settle or use its app source.
#[derive(Debug)]
pub enum AppSourceError {
    /// A local stage directory exists without the whole app in it.
    Incomplete {
        /// The directory that was read.
        directory: PathBuf,
        /// Every staged file it does not hold.
        missing: Vec<&'static str>,
    },
    /// A file the directory named at startup cannot be read now.
    Unreadable {
        /// Exact path that failed.
        path: PathBuf,
        /// Underlying failure.
        source: io::Error,
    },
}

impl fmt::Display for AppSourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Incomplete { directory, missing } => write!(
                formatter,
                "the local app stage {} is missing {}; build it with cargo xtask local-app \
                 or remove the directory to use the compiled app",
                directory.display(),
                missing.join(", ")
            ),
            Self::Unreadable { path, source } => {
                write!(
                    formatter,
                    "cannot read staged app {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl Error for AppSourceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Incomplete { .. } => None,
            Self::Unreadable { source, .. } => Some(source),
        }
    }
}

/// Mode-0600 discovery document for one resident generation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Discovery {
    /// Published loopback origin.
    pub endpoint: String,
    /// Operator capability presented only inside a socket hello.
    pub capability: wire::CanonicalUuid,
    /// Resident generation fenced by the endpoint and capability.
    pub epoch: wire::CanonicalUuid,
}

impl fmt::Debug for Discovery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Discovery")
            .field("endpoint", &self.endpoint)
            .field("capability", &"[redacted]")
            .field("epoch", &self.epoch)
            .finish()
    }
}

impl Discovery {
    /// Atomically publish this discovery document with owner-only permissions.
    pub fn publish(&self, path: impl AsRef<Path>) -> Result<(), PublishError> {
        let bytes = wire::encode(self).map_err(|error| PublishError::Encode(error.to_string()))?;
        let destination = atomic::Destination::new(path).map_err(PublishError::Atomic)?;
        let staged = destination.stage().map_err(PublishError::Atomic)?;
        Self::publish_staged(staged, &bytes, |_| {})
    }

    /// Secures, fills, and atomically publishes one already-created stage.
    fn publish_staged(
        mut staged: atomic::StagedWrite,
        bytes: &[u8],
        before_write: impl FnOnce(&Path),
    ) -> Result<(), PublishError> {
        set_owner_only(staged.temporary_path())?;
        before_write(staged.temporary_path());
        staged.write_all(bytes).map_err(|source| PublishError::Io {
            path: staged.temporary_path().to_path_buf(),
            source,
        })?;
        staged.commit().map_err(PublishError::Atomic)
    }
}

fn set_owner_only(path: &Path) -> Result<(), PublishError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|source| {
            PublishError::Io {
                path: path.to_path_buf(),
                source,
            }
        })
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Discovery encoding or atomic publication failed.
#[derive(Debug)]
pub enum PublishError {
    Encode(String),
    Atomic(atomic::Error),
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for PublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode(error) => write!(formatter, "canvas discovery did not encode: {error}"),
            Self::Atomic(error) => error.fmt(formatter),
            Self::Io { path, source } => {
                write!(
                    formatter,
                    "canvas discovery write failed at {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl Error for PublishError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Encode(_) => None,
            Self::Atomic(error) => Some(error),
            Self::Io { source, .. } => Some(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    fn uuid(text: &str) -> wire::CanonicalUuid {
        text.parse().expect("canonical fixture UUID")
    }

    #[test]
    fn embedded_bytes_have_the_distribution_artifact_identity() {
        assert!(!EMBEDDED_APP.bytes().is_empty());
        assert_eq!(
            EMBEDDED_APP.digest(),
            wire::Digest::compute(wire::Domain::DistArtifact, EMBEDDED_APP.bytes())
        );
    }

    #[test]
    fn embedded_routes_are_closed_typed_and_nonempty() {
        assert_eq!(
            EMBEDDED_APP
                .assets()
                .iter()
                .map(|asset| (asset.path(), asset.content_type()))
                .collect::<Vec<_>>(),
            [
                ("/", "text/html; charset=utf-8"),
                ("/app.module", "text/javascript; charset=utf-8"),
                ("/style.css", "text/css; charset=utf-8"),
                ("/app_bg.wasm", "application/wasm"),
            ]
        );
        assert!(
            EMBEDDED_APP
                .assets()
                .iter()
                .all(|asset| !asset.bytes().is_empty())
        );
        assert!(EMBEDDED_APP.asset("/missing").is_none());
        assert!(
            String::from_utf8_lossy(EMBEDDED_APP.bytes()).contains("/app_bg.wasm"),
            "staged HTML does not load the embedded Wasm"
        );
    }

    /// A worktree with nothing in it yet.
    fn worktree() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "spice-canvas-app-source-{}-{}",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("fixture worktree");
        root
    }

    /// Fill that worktree's development stage with exactly the named files.
    fn staged(root: &Path, files: &[(&str, Vec<u8>)]) -> PathBuf {
        let stage = root.join(LOCAL_STAGE);
        fs::create_dir_all(&stage).expect("fixture stage");
        for (file, bytes) in files {
            fs::write(stage.join(file), bytes).expect("fixture asset");
        }
        stage
    }

    /// Bytes for every staged file, each naming itself, so no route can be
    /// read from another route's file without the assertion seeing it.
    fn local_app() -> Vec<(&'static str, Vec<u8>)> {
        EMBEDDED_APP
            .assets()
            .iter()
            .map(|asset| {
                (
                    asset.file(),
                    format!("locally built {}", asset.file()).into_bytes(),
                )
            })
            .collect()
    }

    #[test]
    fn a_worktree_without_a_local_stage_uses_what_was_compiled_in() {
        let root = worktree();

        let source = AppSource::resolve(&root).expect("an absent stage resolves");

        assert_eq!(source, AppSource::Embedded);
        assert_eq!(source.to_string(), "compiled app");
        let resolved = source
            .asset("/")
            .expect("compiled bytes are always readable")
            .expect("the root route resolves");
        assert_eq!(resolved.content_type(), "text/html; charset=utf-8");
        assert_eq!(resolved.into_bytes().as_ref(), EMBEDDED_APP.bytes());
        assert!(
            source
                .asset("/missing")
                .expect("unknown routes are readable")
                .is_none(),
            "an unknown route gained a shell fallback"
        );
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn a_complete_local_stage_is_read_again_on_every_request() {
        let root = worktree();
        let stage = staged(&root, &local_app());

        let source = AppSource::resolve(&root).expect("a complete stage resolves");

        assert_eq!(source, AppSource::Directory(stage.clone()));
        assert_eq!(
            source.to_string(),
            format!("local app at {}", stage.display())
        );
        let resolved = source
            .asset("/")
            .expect("staged files are readable")
            .expect("the root route resolves");
        assert_eq!(resolved.content_type(), "text/html; charset=utf-8");
        assert_eq!(
            resolved.into_bytes().as_ref(),
            b"locally built canvas-app.html"
        );
        let wasm = source
            .asset("/app_bg.wasm")
            .expect("staged files are readable")
            .expect("the wasm route resolves");
        assert_eq!(wasm.content_type(), "application/wasm");
        assert_eq!(wasm.into_bytes().as_ref(), b"locally built app_bg.wasm");

        // The operator rebuilds under a canvas that is already listening: the
        // next request has to carry the new bytes, because nothing resolves
        // the source a second time.
        fs::write(stage.join("canvas-app.html"), b"rebuilt").expect("fixture rebuild");

        let resolved = source
            .asset("/")
            .expect("staged files are readable")
            .expect("the root route resolves");
        assert_eq!(
            resolved.digest(),
            wire::Digest::compute(wire::Domain::DistArtifact, b"rebuilt")
        );
        assert_eq!(resolved.into_bytes().as_ref(), b"rebuilt");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn a_local_stage_missing_files_refuses_rather_than_mixing_sources() {
        let root = worktree();
        let whole = local_app();
        let stage = staged(&root, &whole[..2]);

        let error = AppSource::resolve(&root).expect_err("a partial stage refuses");

        let AppSourceError::Incomplete { directory, missing } = &error else {
            panic!("a partial stage refused as {error:?}");
        };
        assert_eq!(directory, &stage);
        assert_eq!(missing, &["style.css", "app_bg.wasm"]);
        let message = error.to_string();
        assert!(
            message.contains("style.css") && message.contains("app_bg.wasm"),
            "the refusal does not name what is missing: {message}"
        );
        assert!(
            message.contains("cargo xtask local-app"),
            "the refusal does not say how to build the stage: {message}"
        );
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn a_staged_file_lost_after_startup_names_itself() {
        let root = worktree();
        let stage = staged(&root, &local_app());
        let source = AppSource::resolve(&root).expect("a complete stage resolves");

        fs::remove_file(stage.join("style.css")).expect("fixture removes one staged file");

        let error = source
            .asset("/style.css")
            .expect_err("a vanished staged file refuses");
        let AppSourceError::Unreadable { path, .. } = &error else {
            panic!("a vanished staged file refused as {error:?}");
        };
        assert_eq!(path, &stage.join("style.css"));
        assert!(
            source.asset("/").is_ok(),
            "one unreadable file took the routes that are still there with it"
        );
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn discovery_is_atomic_owner_only_and_never_debugs_its_capability() {
        let root = std::env::temp_dir().join(format!(
            "spice-canvas-discovery-{}-{}",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("fixture directory");
        let path = root.join("canvas");
        let discovery = Discovery {
            endpoint: "http://127.0.0.1:48123".to_owned(),
            capability: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
            epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c4"),
        };

        discovery.publish(&path).expect("discovery publishes");
        let decoded: Discovery = wire::decode(&fs::read(&path).expect("published bytes"))
            .expect("published discovery decodes");

        assert_eq!(decoded, discovery);
        assert!(!format!("{discovery:?}").contains(&discovery.capability.to_string()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path)
                    .expect("published metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn staged_discovery_is_owner_only_before_any_capability_byte_exists() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "spice-canvas-private-stage-{}-{}",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("fixture directory");
        let path = root.join("canvas");
        let discovery = Discovery {
            endpoint: "http://127.0.0.1:48123".to_owned(),
            capability: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
            epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c4"),
        };
        let bytes = wire::encode(&discovery).expect("discovery encodes");
        let destination = atomic::Destination::new(&path).expect("destination resolves");
        let staged = destination.stage().expect("discovery stages");
        fs::set_permissions(staged.temporary_path(), fs::Permissions::from_mode(0o666))
            .expect("fixture starts non-private");

        let mut observed = false;
        Discovery::publish_staged(staged, &bytes, |temporary| {
            let metadata = fs::metadata(temporary).expect("staged metadata");
            assert_eq!(metadata.len(), 0, "capability bytes are not present yet");
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
            observed = true;
        })
        .expect("discovery publishes");

        assert!(observed, "the pre-write stage was observed");
        assert_eq!(
            fs::metadata(&path)
                .expect("published metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::remove_dir_all(root).expect("fixture cleanup");
    }
}
