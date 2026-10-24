//! Reads of this executable's spice rack: one member's text, a pointer line
//! handing off a non-text member's bytes, or the members beneath a namespace
//! or directory as a `resources:` footer.
//!
//! A rack URL may name a generation by the short code of the journal fact
//! that admitted the linked rack; this executable serves only that one.

use std::fs;
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serve::rack::{Namespace, Rack, RackMember};
use spice::repository::StatePaths;
use wire::{CanonicalUuid, Digest, Domain, SequenceCode};

use super::{Request, Resource, SCHEME, Selector, push_resources};

/// Largest member a read extracts; a larger one is refused, never streamed.
pub(super) const EXTRACTION_BYTES: u32 = 16 * 1024 * 1024;

/// What a rack read takes from the runtime directory: where it extracts and
/// the short code of the fact admitting the linked rack, if one was appended.
pub(super) struct Runtime {
    pub(super) extractions: PathBuf,
    pub(super) code: Option<SequenceCode>,
}

impl Request {
    /// A rack member read: no query, and a generation only by short code.
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
        let generation = written
            .rsplit_once('@')
            .map(|(_, generation)| generation_code(url, generation))
            .transpose()?;
        let path = member.rsplit_once('@').map_or(member, |(path, _)| path);
        Ok(Self {
            resource: Resource::Rack {
                namespace,
                path: path.to_owned(),
                generation,
            },
            selector,
        })
    }
}

/// The short code a rack URL's `@` segment names: a digest or UUID never
/// names a generation, and other text is no code.
fn generation_code(url: &str, generation: &str) -> Result<SequenceCode, String> {
    if generation.parse::<Digest>().is_ok() || generation.parse::<CanonicalUuid>().is_ok() {
        return Err(format!(
            "{url} names a generation by digest or UUID; a rack URL names a generation only by \
             the journal short code of the fact that admitted it"
        ));
    }
    generation
        .parse()
        .map_err(|_| format!("{url} names generation {generation}, which is no journal short code"))
}

/// Why a rack URL naming `generation` is refused: this executable serves
/// only the rack it links, admitted as `linked` or by no fact at all.
fn generation_refusal(url: &str, generation: SequenceCode, linked: Option<SequenceCode>) -> String {
    match linked {
        Some(linked) => format!(
            "{url} names generation {generation}, but this executable links generation {linked}; \
             read @{linked} or drop the segment"
        ),
        None => format!(
            "{url} names generation {generation}, but no journal fact admits the rack this \
             executable links; drop @{generation}"
        ),
    }
}

/// [`document`] against the rack linked into this executable, reading the
/// runtime directory `state` admits for its extractions and journal.
pub(super) fn linked(
    namespace: Namespace,
    path: &str,
    generation: Option<SequenceCode>,
    url: &str,
    state: impl FnOnce() -> Result<StatePaths, String>,
) -> Result<String, String> {
    let (rack, archive) = match (spice::version::canvas(), spice::version::rack_archive()) {
        (Some(Ok(linked)), Some(archive)) => (linked.rack(), archive),
        (Some(Err(error)), _) => {
            return Err(format!(
                "{url} names a rack member, but this executable's spice rack is refused: {error}"
            ));
        }
        _ => {
            return Err(format!(
                "{url} names a rack member, but this executable links no spice rack"
            ));
        }
    };
    document(rack, namespace, path, generation, url, || {
        let state = state()?;
        Ok(Runtime {
            extractions: state.rack().to_path_buf(),
            code: admission_code(&state, archive, url)?,
        })
    })
}

/// The short code of the fact admitting `archive`: none while the repository
/// has no journal or its journal names no such fact.
fn admission_code(
    state: &StatePaths,
    archive: &Digest,
    url: &str,
) -> Result<Option<SequenceCode>, String> {
    let journal = state.journal();
    let unreadable = |error: &dyn std::fmt::Display| {
        format!("{url} names a generation the journal could not answer: {error}")
    };
    match journal.try_exists() {
        Ok(true) => {}
        Ok(false) => return Ok(None),
        Err(error) => return Err(unreadable(&error)),
    }
    let reader = journal::JournalReader::open(journal).map_err(|error| unreadable(&error))?;
    reader
        .read_with(|read| spice::rack_admission::code(read, archive))
        .map_err(|error| unreadable(&error))
}

/// The text of one rack member, the pointer line handing off a non-text
/// member extracted beneath the runtime's extractions, or, when `path` names
/// a namespace or directory, its members as a `resources:` footer.
///
/// The runtime is resolved only when the read needs it: to check the
/// generation a URL names, or to hand off bytes under the linked code.
pub(super) fn document(
    rack: &Rack<'_>,
    namespace: Namespace,
    path: &str,
    generation: Option<SequenceCode>,
    url: &str,
    runtime: impl FnOnce() -> Result<Runtime, String>,
) -> Result<String, String> {
    let member = namespace.member(path);
    let media_type = serve::rack::media_type(&member);
    let stored = rack.member(&member);
    let handed_off = stored.filter(|_| !serve::rack::is_text(media_type));
    if generation.is_none() && handed_off.is_none() {
        return text_or_listing(rack, namespace, path, stored, media_type, url);
    }
    let runtime = runtime()?;
    if let Some(generation) = generation.filter(|generation| runtime.code != Some(*generation)) {
        return Err(generation_refusal(url, generation, runtime.code));
    }
    match handed_off {
        Some(stored) => {
            let directory = runtime.extractions.join(rack.generation().as_str());
            handoff(&member, stored, media_type, &directory, runtime.code, url)
        }
        None => text_or_listing(rack, namespace, path, stored, media_type, url),
    }
}

/// A text member's bytes as UTF-8, or the `resources:` footer beneath `path`.
fn text_or_listing(
    rack: &Rack<'_>,
    namespace: Namespace,
    path: &str,
    stored: Option<RackMember<'_>>,
    media_type: &str,
    url: &str,
) -> Result<String, String> {
    if let Some(stored) = stored {
        let member = namespace.member(path);
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

/// One line naming a non-text member's media type, size, digest, the
/// generation short code when a fact admitted the rack, and the file in
/// `directory` holding its bytes. The file is named by the bytes' digest and
/// written once: a later read finds it and writes nothing.
fn handoff(
    member: &str,
    stored: RackMember<'_>,
    media_type: &str,
    directory: &Path,
    code: Option<SequenceCode>,
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
    let generation = code.map_or_else(String::new, |code| format!(" generation {code}"));
    Ok(format!(
        "{media_type} {size} bytes digest {digest}{generation} at {}",
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
