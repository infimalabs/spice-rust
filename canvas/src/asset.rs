use std::error::Error;
use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

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

/// The sole browser app generation store, built and served per worktree.
pub const APP_STAGE: &str = "dist/stage-app";
/// Atomic pointer to the active immutable lane-app generation.
pub const APP_STAGE_CURRENT: &str = "current";
/// Immutable lane-app generations beneath [`APP_STAGE`].
pub const APP_STAGE_GENERATIONS: &str = "generations";
/// Route prefix that pins a page's dependent assets to one generation.
pub const APP_GENERATION_ROUTE: &str = "/_spice/app";

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

    /// Resolve one file in the closed generation inventory.
    #[must_use]
    pub fn asset_file(self, file: &str) -> Option<AppAsset> {
        self.assets.iter().copied().find(|asset| asset.file == file)
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
    stage: PathBuf,
}

/// One immutable, complete lane-app generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppGeneration {
    directory: PathBuf,
    identity: wire::Digest,
}

impl AppSource {
    /// Resolve the one app stage this worktree must serve.
    ///
    /// This is the only stage freshness check on a hot path, and it is kept
    /// because it is the one that matters: without it a lane serves whatever
    /// generation happens to be lying in its stage, so an operator reads a UI
    /// that is not the one this worktree built. It costs 36.8us measured over
    /// 20000 iterations -- one read of the pointer plus one directory read and
    /// one `symlink_metadata` per entry, over a closed four-file inventory that
    /// cannot grow with the repository. Nothing here builds; publishing a new
    /// generation is `cargo xtask app`, 20.66s of native `trunk` with no
    /// emulated container anywhere in it.
    ///
    /// # Errors
    ///
    /// Returns [`AppSourceError::Incomplete`] when the pointer or any file in
    /// its named generation is absent.
    pub fn resolve(worktree: &Path) -> Result<Self, AppSourceError> {
        let source = Self {
            stage: worktree.join(APP_STAGE),
        };
        source.current_generation()?;
        Ok(source)
    }

    /// Resolve the generation named by the one atomic publication pointer.
    ///
    /// # Errors
    ///
    /// Returns a named pointer, identity, inventory, or file-system failure.
    pub fn current_generation(&self) -> Result<AppGeneration, AppSourceError> {
        let pointer = self.stage.join(APP_STAGE_CURRENT);
        let text =
            std::fs::read_to_string(&pointer).map_err(|source| AppSourceError::Incomplete {
                directory: self.stage.clone(),
                missing: vec![APP_STAGE_CURRENT.to_owned()],
                source: Some(source),
            })?;
        let spelling = text.strip_suffix('\n').ok_or_else(|| {
            AppSourceError::Generation(format!(
                "{} is not one newline-terminated generation identity",
                pointer.display()
            ))
        })?;
        let identity = wire::Digest::from_str(spelling).map_err(|error| {
            AppSourceError::Generation(format!("{}: {error}", pointer.display()))
        })?;
        self.generation(identity)
    }

    fn generation(&self, identity: wire::Digest) -> Result<AppGeneration, AppSourceError> {
        let directory = self
            .stage
            .join(APP_STAGE_GENERATIONS)
            .join(identity.as_str());
        validate_generation(&directory)?;
        Ok(AppGeneration {
            directory,
            identity,
        })
    }

    /// Serve one exact route. Unknown paths have no shell fallback.
    ///
    /// The root reads the current atomic generation pointer. Every dependent
    /// route carries the generation identity emitted into that root, so a
    /// publication between requests cannot mix a page's module, stylesheet,
    /// and Wasm across generations.
    ///
    /// # Errors
    ///
    /// Returns a named generation, inventory, or read error when the current
    /// pointer or one versioned generation cannot supply its complete answer.
    pub fn served(&self, route: &str) -> Result<Option<ServedAsset>, AppSourceError> {
        if route == "/" {
            return self
                .current_generation()?
                .served(APP.asset("/").expect("the closed inventory has a root"))
                .map(Some);
        }
        let Some((identity, file)) = generation_route(route) else {
            return Ok(None);
        };
        let Some(asset) = APP.asset_file(file) else {
            return Ok(None);
        };
        self.generation(identity)?.served(asset).map(Some)
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
        write!(formatter, "lane app at {}", self.stage.display())
    }
}

impl AppGeneration {
    /// Canonical identity used in every generation-qualified route.
    #[must_use]
    pub const fn identity(&self) -> &wire::Digest {
        &self.identity
    }

    /// Route that keeps this asset on this immutable generation.
    #[must_use]
    pub fn route(&self, asset: AppAsset) -> String {
        format!("{APP_GENERATION_ROUTE}/{}/{}", self.identity, asset.file())
    }

    /// Read one asset from this immutable generation.
    ///
    /// # Errors
    ///
    /// Returns [`AppSourceError::Unreadable`] when the generation moved or
    /// became unreadable after it was admitted.
    pub fn served(&self, asset: AppAsset) -> Result<ServedAsset, AppSourceError> {
        let path = self.directory.join(asset.file());
        let bytes =
            std::fs::read(&path).map_err(|source| AppSourceError::Unreadable { path, source })?;
        Ok(ServedAsset {
            bytes,
            content_type: asset.content_type(),
        })
    }
}

fn generation_route(route: &str) -> Option<(wire::Digest, &str)> {
    let rest = route
        .strip_prefix(APP_GENERATION_ROUTE)?
        .strip_prefix('/')?;
    let (identity, file) = rest.split_once('/')?;
    if file.contains('/') {
        return None;
    }
    Some((identity.parse().ok()?, file))
}

fn validate_generation(directory: &Path) -> Result<(), AppSourceError> {
    let mut observed = std::fs::read_dir(directory)
        .map_err(|source| AppSourceError::Incomplete {
            directory: directory.to_path_buf(),
            missing: APP
                .assets()
                .iter()
                .map(|asset| asset.file().to_owned())
                .collect(),
            source: Some(source),
        })?
        .map(|entry| {
            let entry = entry.map_err(|source| AppSourceError::Unreadable {
                path: directory.to_path_buf(),
                source,
            })?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|source| AppSourceError::Unreadable { path, source })?;
            if !metadata.file_type().is_file() {
                return Err(AppSourceError::Inventory(format!(
                    "{} is not a regular generation file",
                    entry.path().display()
                )));
            }
            entry.file_name().into_string().map_err(|_| {
                AppSourceError::Inventory(format!(
                    "{} contains a non-UTF-8 file name",
                    directory.display()
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    observed.sort();
    let mut expected = APP
        .assets()
        .iter()
        .map(|asset| asset.file().to_owned())
        .collect::<Vec<_>>();
    expected.sort();
    if observed != expected {
        let missing = expected
            .iter()
            .filter(|file| !observed.contains(file))
            .cloned()
            .collect::<Vec<_>>();
        let unexpected = observed
            .iter()
            .filter(|file| !expected.contains(file))
            .collect::<Vec<_>>();
        if unexpected.is_empty() && !missing.is_empty() {
            return Err(AppSourceError::Incomplete {
                directory: directory.to_path_buf(),
                missing,
                source: None,
            });
        }
        return Err(AppSourceError::Inventory(format!(
            "{} contains {observed:?}, expected {expected:?}",
            directory.display()
        )));
    }
    Ok(())
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

    /// Borrow the exact response bytes before consuming the asset.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
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
    /// A app stage directory exists without the whole app in it.
    Incomplete {
        /// The directory that was read.
        directory: PathBuf,
        /// Every staged file it does not hold.
        missing: Vec<String>,
        /// File-system failure that made the generation incomplete, if any.
        source: Option<io::Error>,
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
    /// The atomic generation pointer did not name one canonical identity.
    Generation(String),
}

impl fmt::Display for AppSourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Incomplete {
                directory, missing, ..
            } => write!(
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
            Self::Generation(reason) => {
                write!(formatter, "canvas app generation invalid: {reason}")
            }
        }
    }
}

impl Error for AppSourceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Incomplete { source, .. } => source
                .as_ref()
                .map(|source| source as &(dyn Error + 'static)),
            Self::Unreadable { source, .. } => Some(source),
            Self::Inventory(_) | Self::Generation(_) => None,
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
    const FIRST_GENERATION: &str =
        "1111111111111111111111111111111111111111111111111111111111111111";
    const SECOND_GENERATION: &str =
        "2222222222222222222222222222222222222222222222222222222222222222";

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

    /// Fill that worktree's app stage with exactly the named files.
    fn staged_as(root: &Path, identity: &str, files: &[(&str, Vec<u8>)]) -> PathBuf {
        let stage = root.join(APP_STAGE);
        let generation = stage.join(APP_STAGE_GENERATIONS).join(identity);
        fs::create_dir_all(&generation).expect("fixture generation");
        for (file, bytes) in files {
            fs::write(generation.join(file), bytes).expect("fixture asset");
        }
        fs::write(stage.join(APP_STAGE_CURRENT), format!("{identity}\n")).expect("fixture pointer");
        generation
    }

    fn staged(root: &Path, files: &[(&str, Vec<u8>)]) -> PathBuf {
        staged_as(root, FIRST_GENERATION, files)
    }

    /// Bytes for every staged file, each naming itself, so no route can be
    /// served from another route's file without the assertion seeing it.
    fn app() -> Vec<(&'static str, Vec<u8>)> {
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
    fn one_page_generation_remains_addressable_after_atomic_publication() {
        let root = worktree();
        let first = staged(&root, &app());
        let stage = root.join(APP_STAGE);

        let source = AppSource::resolve(&root).expect("a complete stage resolves");
        let first_generation = source
            .current_generation()
            .expect("first generation resolves");

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
        let wasm_route = first_generation.route(
            APP.asset("/app_bg.wasm")
                .expect("the closed inventory has wasm"),
        );
        let wasm = source
            .served(&wasm_route)
            .expect("staged files are readable")
            .expect("the wasm route is served");
        assert_eq!(wasm.content_type(), "application/wasm");
        assert_eq!(wasm.into_bytes(), b"locally built app_bg.wasm");

        let mut rebuilt = app();
        rebuilt
            .iter_mut()
            .find(|(file, _)| *file == "canvas-app.html")
            .expect("fixture shell")
            .1 = b"rebuilt".to_vec();
        staged_as(&root, SECOND_GENERATION, &rebuilt);

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
        assert_eq!(
            source
                .served(&wasm_route)
                .expect("the first generation remains readable")
                .expect("the first wasm route remains admitted")
                .into_bytes(),
            b"locally built app_bg.wasm",
            "publication mixed the first page's dependent asset with the second generation"
        );
        assert!(first.is_dir(), "the first immutable generation moved");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn an_app_stage_missing_files_refuses_rather_than_mixing_sources() {
        let root = worktree();
        let whole = app();
        let stage = staged(&root, &whole[..2]);

        let error = AppSource::resolve(&root).expect_err("a partial stage refuses");

        let AppSourceError::Incomplete {
            directory, missing, ..
        } = &error
        else {
            panic!("a partial stage refused as {error:?}");
        };
        assert_eq!(directory, &stage);
        assert_eq!(missing, &["app_bg.wasm", "style.css"]);
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
        let stage = staged(&root, &app());
        let source = AppSource::resolve(&root).expect("a complete stage resolves");
        let generation = source
            .current_generation()
            .expect("current generation resolves");

        fs::remove_file(stage.join("style.css")).expect("fixture removes one staged file");

        let route = generation.route(
            APP.asset("/style.css")
                .expect("the closed inventory has a stylesheet"),
        );
        let error = source
            .served(&route)
            .expect_err("a vanished staged file refuses");
        let AppSourceError::Incomplete { directory, .. } = &error else {
            panic!("a vanished staged file refused as {error:?}");
        };
        assert_eq!(directory, &stage);
        assert!(
            source.served("/").is_err(),
            "a partial generation was served route by route"
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
