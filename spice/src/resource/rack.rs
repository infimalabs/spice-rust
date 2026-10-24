//! Reads of this executable's spice rack: one member's text, a pointer line
//! handing off a non-text member's bytes, or the members beneath a namespace
//! or directory as a `resources:` footer.

use std::fs;
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serve::rack::{Namespace, Rack, RackMember};
use wire::{CanonicalUuid, Digest, Domain};

use super::{Request, Resource, SCHEME, Selector, push_resources};

/// Largest member a read extracts; a larger one is refused, never streamed.
pub(super) const EXTRACTION_BYTES: u32 = 16 * 1024 * 1024;

impl Request {
    /// A rack member read: no query, and no generation but the linked one.
    pub(super) fn rack(
        namespace: Namespace,
        member: &str,
        written: &str,
        query: &str,
        selector: Option<Selector>,
        url: &str,
    ) -> Result<Self, String> {
        if !query.is_empty() {
            return Err(format!(
                "{url} takes no query; use a trailing line selector to bound the read"
            ));
        }
        if let Some((_, generation)) = written.rsplit_once('@') {
            return Err(generation_refusal(url, generation));
        }
        Ok(Self {
            resource: Resource::Rack {
                namespace,
                path: member.to_owned(),
            },
            selector,
        })
    }
}

/// Why a rack URL naming a generation is refused: a digest or UUID never
/// names one, and this executable serves only the rack it links.
fn generation_refusal(url: &str, generation: &str) -> String {
    if generation.parse::<Digest>().is_ok() || generation.parse::<CanonicalUuid>().is_ok() {
        format!(
            "{url} names a generation by digest or UUID; a rack URL names a generation only by \
             the journal short code of the release or install fact that admitted it"
        )
    } else if generation.parse::<wire::SequenceCode>().is_ok() {
        format!(
            "{url} names generation {generation}, but this executable serves only the rack it \
             links; drop @{generation}"
        )
    } else {
        format!("{url} names generation {generation}, which is no journal short code")
    }
}

/// [`document`] against the rack linked into this executable, extracting
/// beneath the directory `extractions` answers.
pub(super) fn linked(
    namespace: Namespace,
    path: &str,
    url: &str,
    extractions: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<String, String> {
    match spice::version::canvas() {
        Some(Ok(linked)) => document(linked.rack(), namespace, path, url, extractions),
        Some(Err(error)) => Err(format!(
            "{url} names a rack member, but this executable's spice rack is refused: {error}"
        )),
        None => Err(format!(
            "{url} names a rack member, but this executable links no spice rack"
        )),
    }
}

/// The text of one rack member, the pointer line handing off a non-text
/// member extracted beneath `extractions`, or, when `path` names a namespace
/// or directory, its members as a `resources:` footer.
pub(super) fn document(
    rack: &Rack<'_>,
    namespace: Namespace,
    path: &str,
    url: &str,
    extractions: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<String, String> {
    let member = namespace.member(path);
    if let Some(stored) = rack.member(&member) {
        let media_type = serve::rack::media_type(&member);
        if !serve::rack::is_text(media_type) {
            let directory = extractions()?.join(rack.generation().as_str());
            return handoff(&member, stored, media_type, &directory, url);
        }
        let bytes = stored.inflate(&member).map_err(|error| error.to_string())?;
        return String::from_utf8(bytes.into_vec())
            .map_err(|_| format!("{url} declares {media_type} but is not UTF-8 text"));
    }
    let directory = if path.is_empty() {
        String::new()
    } else {
        format!("{path}/")
    };
    let children = rack
        .namespace(namespace)
        .filter(|(file, _)| file.starts_with(directory.as_str()))
        .map(|(file, _)| format!("{SCHEME}{}", namespace.member(file)))
        .collect::<Vec<_>>();
    if children.is_empty() {
        return Err(format!(
            "{url} names no member of this executable's spice rack"
        ));
    }
    let mut document = format!(
        "{SCHEME}{} spice rack members: {}",
        namespace.member(&directory),
        children.len()
    );
    push_resources(&mut document, &children);
    Ok(document)
}

/// One line naming a non-text member's media type, size, digest, and the
/// file in `directory` holding its bytes. The file is named by the bytes'
/// digest and written once: a later read finds it and writes nothing.
fn handoff(
    member: &str,
    stored: RackMember<'_>,
    media_type: &str,
    directory: &Path,
    url: &str,
) -> Result<String, String> {
    let size = stored.size();
    if size > EXTRACTION_BYTES {
        return Err(format!(
            "{url} is {size} bytes of {media_type}, above the {EXTRACTION_BYTES} byte rack \
             extraction bound; it is not handed off"
        ));
    }
    let bytes = stored.inflate(member).map_err(|error| error.to_string())?;
    let digest = Digest::compute(Domain::DistArtifact, &bytes);
    let name = match member
        .rsplit('/')
        .next()
        .and_then(|file| file.rsplit_once('.'))
    {
        Some((_, extension)) => format!("{digest}.{extension}"),
        None => digest.to_string(),
    };
    let file = directory.join(name);
    let refused = |error: std::io::Error| {
        format!(
            "{url} could not be extracted to {}: {error}",
            file.display()
        )
    };
    match fs::symlink_metadata(&file) {
        Ok(metadata) if metadata.is_file() && metadata.len() == u64::from(size) => {}
        Ok(_) => {
            return Err(refused(std::io::Error::new(
                ErrorKind::AlreadyExists,
                "another entry holds the digest's name",
            )));
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            extract(directory, &file, &bytes).map_err(refused)?;
        }
        Err(error) => return Err(refused(error)),
    }
    Ok(format!(
        "{media_type} {size} bytes digest {digest} at {}",
        file.display()
    ))
}

/// Write `bytes` to `file` through a private sibling renamed into place, so
/// the digest-named file is never seen partial and never rewritten.
fn extract(directory: &Path, file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static PARTIALS: AtomicU64 = AtomicU64::new(0);
    fs::create_dir_all(directory)?;
    let partial = directory.join(format!(
        ".partial-{}-{}",
        std::process::id(),
        PARTIALS.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)?;
        output.write_all(bytes)?;
        output.sync_all()
    })()
    .and_then(|()| fs::rename(&partial, file));
    match written {
        Ok(()) => Ok(()),
        // A racing read renamed the same digest's bytes into place first.
        Err(_) if fs::symlink_metadata(file).is_ok_and(|metadata| metadata.is_file()) => {
            let _ = fs::remove_file(&partial);
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&partial);
            Err(error)
        }
    }
}
