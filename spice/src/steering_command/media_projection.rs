use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wire::{AgentMediaReference, CanonicalUuid, Digest};

const DIRECTORY_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;
const MAX_PROJECTION_PATH_BYTES: usize = 4096;

#[derive(Clone, Debug)]
pub(crate) struct Projection {
    lane: CanonicalUuid,
    reference: AgentMediaReference,
    path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    directive: CanonicalUuid,
    ordinal: u8,
    digest: Digest,
    bytes: u64,
    content_type: String,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct RenderedProjection<'a> {
    lane: CanonicalUuid,
    directive: CanonicalUuid,
    ordinal: u8,
    digest: &'a Digest,
    bytes: u64,
    content_type: &'a str,
    standing: &'static str,
    path: &'a str,
}

impl Projection {
    pub(crate) const fn reference(&self) -> &AgentMediaReference {
        &self.reference
    }

    pub(crate) fn encoded(&self) -> Result<Vec<u8>, String> {
        wire::encode(&RenderedProjection {
            lane: self.lane,
            directive: self.reference.directive(),
            ordinal: self.reference.ordinal(),
            digest: self.reference.digest(),
            bytes: self.reference.bytes(),
            content_type: self.reference.content_type(),
            standing: "live",
            path: &self.path,
        })
        .map_err(|error| error.to_string())
    }
}

pub(crate) fn reconcile(
    lane: CanonicalUuid,
    pending: &[wire::intent::PendingDirective],
    retained: &mut Vec<Record>,
    root: &Path,
    retire_previous_generation: bool,
) -> Result<BTreeMap<CanonicalUuid, Vec<Projection>>, String> {
    let mut desired = Vec::new();
    let mut projected = BTreeMap::new();
    for directive in pending {
        let references =
            wire::media::agent_media_references(directive.directive, &directive.attachments)
                .map_err(|error| {
                    format!(
                        "directive {} media is invalid: {error}",
                        directive.directive
                    )
                })?;
        let mut items = Vec::with_capacity(references.len());
        for (reference, attachment) in references.into_iter().zip(&directive.attachments) {
            let bytes = attachment.decoded_bytes().map_err(|error| {
                format!(
                    "directive {} attachment {} is undecodable: {error}",
                    directive.directive,
                    reference.ordinal(),
                )
            })?;
            let record = Record::from_reference(&reference);
            let path = projection_path(root, lane, &record)?;
            publish_verified(&path, &bytes)?;
            let path = path
                .to_str()
                .ok_or_else(|| "steering media projection path is not valid UTF-8".to_owned())?;
            if path.len() > MAX_PROJECTION_PATH_BYTES {
                return Err(format!(
                    "steering media projection path is {} bytes over the {} byte limit",
                    path.len(),
                    MAX_PROJECTION_PATH_BYTES,
                ));
            }
            desired.push(record);
            items.push(Projection {
                lane,
                reference,
                path: path.to_owned(),
            });
        }
        projected.insert(directive.directive, items);
    }
    reconcile_retention(
        lane,
        retained,
        root,
        &mut desired,
        retire_previous_generation,
    )?;
    *retained = desired;
    Ok(projected)
}

fn reconcile_retention(
    lane: CanonicalUuid,
    retained: &[Record],
    root: &Path,
    desired: &mut Vec<Record>,
    retire_previous_generation: bool,
) -> Result<(), String> {
    if retire_previous_generation {
        for obsolete in retained.iter().filter(|record| !desired.contains(record)) {
            remove_projection(&projection_path(root, lane, obsolete)?)?;
        }
    } else {
        // A path already advertised into this session remains readable after ACK. The next
        // occupant or compaction generation is the first point that can retire it safely.
        let still_owned = retained
            .iter()
            .filter(|record| !desired.contains(record))
            .cloned()
            .collect::<Vec<_>>();
        desired.extend(still_owned);
    }
    Ok(())
}

/// Retires every projection an earlier occupant left under `lane`.
///
/// A lane has one occupant at a time, so a new occupant owns no path its
/// predecessor advertised.
pub(crate) fn retire_lane(root: &Path, lane: CanonicalUuid) -> Result<(), String> {
    let path = root.join(lane.to_string());
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            fs::remove_dir_all(&path).map_err(|error| {
                format!("earlier occupant steering media could not be retired: {error}")
            })
        }
        Ok(_) => Err(format!(
            "steering media lane {} is not a directory",
            path.display(),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("steering media lane is unreadable: {error}")),
    }
}

impl Record {
    fn from_reference(reference: &AgentMediaReference) -> Self {
        Self {
            directive: reference.directive(),
            ordinal: reference.ordinal(),
            digest: reference.digest().clone(),
            bytes: reference.bytes(),
            content_type: reference.content_type().to_owned(),
        }
    }
}

/// File extension each projectable attachment type is published under.
const EXTENSIONS: [(&str, &str); 4] = [
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/gif", "gif"),
    ("image/webp", "webp"),
];

/// Whether `path` names a file in `lane`'s projection layout.
///
/// The layout is `<root>/<lane>/<directive>/<ordinal>-<digest>.<extension>`,
/// so a lane reading one of these files is looking at an attachment the
/// operator sent it, not at media the lane produced.
pub(crate) fn names_projection(path: &str, lane: CanonicalUuid) -> bool {
    fn name(path: Option<&Path>) -> Option<&str> {
        path.and_then(Path::file_name)
            .and_then(|name| name.to_str())
    }
    let path = Path::new(path);
    let directive = path.parent();
    let owner = directive.and_then(Path::parent);
    let file = name(Some(path)).and_then(|file| {
        let (ordinal, rest) = file.split_once('-')?;
        let (digest, extension) = rest.rsplit_once('.')?;
        Some((ordinal, digest, extension))
    });
    file.is_some_and(|(ordinal, digest, extension)| {
        ordinal.parse::<u8>().is_ok()
            && digest.parse::<Digest>().is_ok()
            && EXTENSIONS.iter().any(|(_, known)| *known == extension)
    }) && name(directive).is_some_and(|directive| directive.parse::<CanonicalUuid>().is_ok())
        && name(owner) == Some(lane.to_string().as_str())
}

fn projection_path(root: &Path, lane: CanonicalUuid, record: &Record) -> Result<PathBuf, String> {
    let Some((_, extension)) = EXTENSIONS
        .iter()
        .find(|(content_type, _)| *content_type == record.content_type)
    else {
        return Err(format!(
            "steering media projection type {:?} is unsupported",
            record.content_type
        ));
    };
    let lane = root.join(lane.to_string());
    let directive = lane.join(record.directive.to_string());
    ensure_private_directory(root)?;
    ensure_private_directory(&lane)?;
    ensure_private_directory(&directive)?;
    Ok(directive.join(format!(
        "{}-{}.{}",
        record.ordinal, record.digest, extension,
    )))
}

fn publish_verified(path: &Path, bytes: &[u8]) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(format!(
                "steering media projection {} is not a regular file",
                path.display(),
            ));
        }
        Ok(metadata) if private_mode(&metadata, FILE_MODE) => {
            let observed = fs::read(path).map_err(|error| {
                format!("published steering media bytes are unreadable: {error}")
            })?;
            if observed == bytes {
                return Ok(());
            }
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("steering media projection is unreadable: {error}")),
    }
    atomic::Destination::new(path)
        .map_err(|error| format!("steering media projection is unavailable: {error}"))?
        .publish(|staged| staged.write_all(bytes))
        .map_err(|error| format!("steering media projection is unpublished: {error}"))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("published steering media is unreadable: {error}"))?;
    if !metadata.file_type().is_file() || !private_mode(&metadata, FILE_MODE) {
        return Err(format!(
            "published steering media {} is not a private regular file",
            path.display(),
        ));
    }
    let observed = fs::read(path)
        .map_err(|error| format!("published steering media bytes are unreadable: {error}"))?;
    if observed != bytes {
        return Err(format!(
            "published steering media {} failed exact byte verification",
            path.display(),
        ));
    }
    Ok(())
}

fn remove_projection(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => fs::remove_file(path)
            .map_err(|error| format!("obsolete steering media could not be removed: {error}")),
        Ok(_) => Err(format!(
            "obsolete steering media {} is not a regular file",
            path.display(),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("obsolete steering media is unreadable: {error}")),
    }
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    create_private_directory(path).map_err(|error| {
        format!(
            "steering media directory {} is unavailable: {error}",
            path.display(),
        )
    })?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("steering media directory is unreadable: {error}"))?;
    if !metadata.file_type().is_dir() || !private_mode(&metadata, DIRECTORY_MODE) {
        return Err(format!(
            "steering media directory {} is not a private directory",
            path.display(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;

    let mut builder = fs::DirBuilder::new();
    builder.mode(DIRECTORY_MODE);
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(not(unix))]
fn create_private_directory(path: &Path) -> io::Result<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn private_mode(metadata: &fs::Metadata, expected: u32) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    metadata.permissions().mode() & 0o777 == expected
}

#[cfg(not(unix))]
fn private_mode(_: &fs::Metadata, _: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const LANE: &str = "0a2f5b6c-1d3e-4f50-8a91-b2c3d4e5f607";
    const DIRECTIVE: &str = "1b3f6c7d-2e4f-4051-9b02-c3d4e5f60718";
    const PNG_DATA_URL: &str = "data:image/png;base64,iVBORw0KGgo=";

    #[test]
    fn non_regular_projection_substitution_refuses_without_replacement() {
        let root = std::env::temp_dir().join(format!(
            "spice-steering-media-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        ));
        let lane: CanonicalUuid = LANE.parse().expect("fixture lane");
        let pending = [directive()];
        let mut retained = Vec::new();
        let projected = reconcile(lane, &pending, &mut retained, &root, false)
            .expect("initial projection publishes");
        let path =
            PathBuf::from(&projected[&DIRECTIVE.parse().expect("fixture directive")][0].path);
        fs::remove_file(&path).expect("remove initial projection");
        fs::create_dir(&path).expect("substitute a directory");

        reconcile(lane, &pending, &mut retained, &root, false)
            .expect_err("non-regular projection must refuse");
        assert!(path.is_dir(), "refusal replaced the substituted path");
        fs::remove_dir_all(root).expect("remove projection fixture");
    }

    fn directive() -> wire::intent::PendingDirective {
        serde_json::from_value(serde_json::json!({
            "directive": DIRECTIVE,
            "key": "1kMedia1",
            "text": "inspect the capture",
            "attachments": [{
                "name": "capture.png",
                "content_type": "image/png",
                "data_url": PNG_DATA_URL,
            }],
            "authored": "2026-08-30T09:00:00.000Z",
            "stage": "published",
            "expires": "2026-08-30T10:00:00.000Z",
        }))
        .expect("projection directive fixture admits")
    }
}
