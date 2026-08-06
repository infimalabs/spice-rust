use std::error::Error;
use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

static APP_ASSETS: [AppAsset; 4] = [
    AppAsset::new("/", "canvas-app.html", "text/html; charset=utf-8"),
    AppAsset::new(
        "/app.module",
        "app.module",
        "text/javascript; charset=utf-8",
    ),
    AppAsset::new("/style.css", "style.css", "text/css; charset=utf-8"),
    AppAsset::new("/app_bg.wasm", "app_bg.wasm", "application/wasm"),
];

/// The one browser app stage, built and served from each worktree.
pub const APP_STAGE: &str = "dist/stage-app";

/// One route in the closed browser app inventory.
#[derive(Clone, Copy)]
pub struct AppAsset {
    path: &'static str,
    file: &'static str,
    content_type: &'static str,
}

impl AppAsset {
    const fn new(path: &'static str, file: &'static str, content_type: &'static str) -> Self {
        Self {
            path,
            file,
            content_type,
        }
    }

    #[must_use]
    pub const fn path(self) -> &'static str {
        self.path
    }

    /// The staged file this route serves.
    #[must_use]
    pub const fn file(self) -> &'static str {
        self.file
    }

    #[must_use]
    pub const fn content_type(self) -> &'static str {
        self.content_type
    }
}

impl fmt::Debug for AppAsset {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppAsset")
            .field("path", &self.path)
            .field("file", &self.file)
            .field("content_type", &self.content_type)
            .finish()
    }
}

/// The closed browser app route inventory.
#[derive(Clone, Copy)]
pub struct App {
    assets: &'static [AppAsset],
}

/// The exact files a lane build must produce.
pub const APP: App = App {
    assets: &APP_ASSETS,
};

impl App {
    /// Every exact staged asset in its closed route order.
    #[must_use]
    pub const fn assets(self) -> &'static [AppAsset] {
        self.assets
    }

    /// Resolve one exact route. Unknown paths have no shell fallback.
    #[must_use]
    pub fn asset(self, path: &str) -> Option<AppAsset> {
        self.assets.iter().copied().find(|asset| asset.path == path)
    }
}

impl fmt::Debug for App {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("App")
            .field("assets", &self.assets)
            .finish()
    }
}

/// The lane-built browser app served by one canvas generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppSource {
    directory: PathBuf,
}

impl AppSource {
    /// Resolve the one app stage this worktree must serve.
    ///
    /// # Errors
    ///
    /// Returns [`AppSourceError::Incomplete`] naming every missing file when
    /// the directory exists without the full app.
    pub fn resolve(worktree: &Path) -> Result<Self, AppSourceError> {
        let directory = worktree.join(APP_STAGE);
        let missing = APP
            .assets()
            .iter()
            .map(|asset| asset.file())
            .filter(|file| !directory.join(file).is_file())
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return Ok(Self { directory });
        }
        Err(AppSourceError::Incomplete { directory, missing })
    }

    /// Serve one exact route. Unknown paths have no shell fallback.
    ///
    /// The directory answer is read per request rather than at startup, so an
    /// app rebuilt underneath a running canvas is served on the next reload
    /// without the resident being restarted.
    ///
    /// # Errors
    ///
    /// Returns [`AppSourceError::Unreadable`] when a directory stage named a
    /// file at startup that cannot be read now.
    pub fn served(&self, route: &str) -> Result<Option<ServedAsset>, AppSourceError> {
        let Some(asset) = APP.asset(route) else {
            return Ok(None);
        };
        let path = self.directory.join(asset.file());
        let bytes =
            std::fs::read(&path).map_err(|source| AppSourceError::Unreadable { path, source })?;
        Ok(Some(ServedAsset {
            bytes,
            content_type: asset.content_type(),
        }))
    }

    /// Identity of the HTML shell this lane build serves.
    ///
    /// # Errors
    ///
    /// Returns [`AppSourceError::Unreadable`] when the staged shell cannot be
    /// read after startup.
    pub fn digest(&self) -> Result<wire::Digest, AppSourceError> {
        self.served("/")?
            .map(|asset| asset.digest())
            .ok_or_else(|| AppSourceError::Inventory("the root route is absent".to_owned()))
    }
}

impl fmt::Display for AppSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "lane app at {}", self.directory.display())
    }
}

/// One lane-built asset resolved for one request.
pub struct ServedAsset {
    bytes: Vec<u8>,
    content_type: &'static str,
}

impl ServedAsset {
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

    /// The bytes read for this request.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl fmt::Debug for ServedAsset {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServedAsset")
            .field("bytes", &self.bytes.len())
            .field("content_type", &self.content_type)
            .field("digest", &self.digest())
            .finish()
    }
}

/// Why a canvas could not settle or serve its app source.
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
    /// The closed route inventory itself is invalid.
    Inventory(String),
}

impl fmt::Display for AppSourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Incomplete { directory, missing } => write!(
                formatter,
                "the lane app stage {} is missing {}; build it with cargo xtask app",
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
            Self::Inventory(reason) => write!(formatter, "canvas app inventory invalid: {reason}"),
        }
    }
}

impl Error for AppSourceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Incomplete { .. } => None,
            Self::Unreadable { source, .. } => Some(source),
            Self::Inventory(_) => None,
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
    fn app_routes_are_closed_and_typed() {
        assert_eq!(
            APP.assets()
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
        assert!(APP.asset("/missing").is_none());
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
        let stage = root.join(APP_STAGE);
        fs::create_dir_all(&stage).expect("fixture stage");
        for (file, bytes) in files {
            fs::write(stage.join(file), bytes).expect("fixture asset");
        }
        stage
    }

    /// Bytes for every staged file, each naming itself, so no route can be
    /// served from another route's file without the assertion seeing it.
    fn local_app() -> Vec<(&'static str, Vec<u8>)> {
        APP.assets()
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
    fn a_worktree_without_a_lane_stage_refuses_and_names_the_builder() {
        let root = worktree();

        let error = AppSource::resolve(&root).expect_err("an absent stage refuses");

        let message = error.to_string();
        assert!(message.contains(APP_STAGE));
        assert!(message.contains("cargo xtask app"));
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn a_complete_local_stage_is_read_again_on_every_request() {
        let root = worktree();
        let stage = staged(&root, &local_app());

        let source = AppSource::resolve(&root).expect("a complete stage resolves");

        assert_eq!(
            source.to_string(),
            format!("lane app at {}", stage.display())
        );
        let served = source
            .served("/")
            .expect("staged files are readable")
            .expect("the root route is served");
        assert_eq!(served.content_type(), "text/html; charset=utf-8");
        assert_eq!(served.into_bytes(), b"locally built canvas-app.html");
        let wasm = source
            .served("/app_bg.wasm")
            .expect("staged files are readable")
            .expect("the wasm route is served");
        assert_eq!(wasm.content_type(), "application/wasm");
        assert_eq!(wasm.into_bytes(), b"locally built app_bg.wasm");

        // The operator rebuilds under a canvas that is already listening: the
        // next request has to carry the new bytes, because nothing resolves
        // the source a second time.
        fs::write(stage.join("canvas-app.html"), b"rebuilt").expect("fixture rebuild");

        let served = source
            .served("/")
            .expect("staged files are readable")
            .expect("the root route is served");
        assert_eq!(
            served.digest(),
            wire::Digest::compute(wire::Domain::DistArtifact, b"rebuilt")
        );
        assert_eq!(
            source.digest().expect("rebuilt shell digests"),
            served.digest()
        );
        assert_eq!(served.into_bytes(), b"rebuilt");
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
            message.contains("cargo xtask app"),
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
            .served("/style.css")
            .expect_err("a vanished staged file refuses");
        let AppSourceError::Unreadable { path, .. } = &error else {
            panic!("a vanished staged file refused as {error:?}");
        };
        assert_eq!(path, &stage.join("style.css"));
        assert!(
            source.served("/").is_ok(),
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
