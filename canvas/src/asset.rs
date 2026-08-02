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
    EmbeddedAsset::new("/", APP_HTML, "text/html; charset=utf-8"),
    EmbeddedAsset::new(
        "/app.module",
        APP_JAVASCRIPT,
        "text/javascript; charset=utf-8",
    ),
    EmbeddedAsset::new("/style.css", APP_STYLESHEET, "text/css; charset=utf-8"),
    EmbeddedAsset::new("/app_bg.wasm", APP_WASM, "application/wasm"),
];

/// One exact browser asset compiled into the native canvas.
#[derive(Clone, Copy)]
pub struct EmbeddedAsset {
    path: &'static str,
    bytes: &'static [u8],
    content_type: &'static str,
}

impl EmbeddedAsset {
    const fn new(path: &'static str, bytes: &'static [u8], content_type: &'static str) -> Self {
        Self {
            path,
            bytes,
            content_type,
        }
    }

    #[must_use]
    pub const fn path(self) -> &'static str {
        self.path
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
    /// Exact embedded bytes served by the loopback endpoint.
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

#[cfg(unix)]
fn set_owner_only(path: &Path) -> Result<(), PublishError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|source| {
        PublishError::Io {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> Result<(), PublishError> {
    Ok(())
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
