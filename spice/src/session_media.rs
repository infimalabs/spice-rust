//! Source-aware resolution of provider-owned transcript media.

use std::ffi::OsStr;
use std::fmt;
use std::fs::File;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

/// Resolve every external media reference in one decoded session record.
///
/// `listen` remains a pure parser. This source boundary owns the filesystem
/// lookup because the transcript path identifies the vendor state root.
pub(crate) fn resolve(
    decoded: &mut listen::Decoded,
    transcript: &Path,
) -> Result<(), ResolveError> {
    let listen::Decoded::Events { events, .. } = decoded else {
        return Ok(());
    };
    if !events.iter().any(has_external_media) {
        return Ok(());
    }
    let root = media_root(transcript).ok_or_else(|| ResolveError::Root {
        transcript: transcript.to_path_buf(),
    })?;
    for event in events {
        match event {
            listen::Event::Prose(prose) | listen::Event::ProseEvidence(prose) => {
                resolve_set(&root, &mut prose.media, &mut prose.external_media)?;
            }
            listen::Event::Tool(listen::Tool {
                moment: listen::ToolMoment::Result(outcome),
                ..
            }) => resolve_set(&root, &mut outcome.media, &mut outcome.external_media)?,
            _ => {}
        }
    }
    Ok(())
}

fn has_external_media(event: &listen::Event) -> bool {
    match event {
        listen::Event::Prose(prose) | listen::Event::ProseEvidence(prose) => {
            !prose.external_media.is_empty()
        }
        listen::Event::Tool(listen::Tool {
            moment: listen::ToolMoment::Result(outcome),
            ..
        }) => !outcome.external_media.is_empty(),
        _ => false,
    }
}

fn resolve_set(
    root: &Path,
    media: &mut Vec<wire::Media>,
    external: &mut Vec<listen::ExternalMedia>,
) -> Result<(), ResolveError> {
    let observed = media
        .len()
        .checked_add(external.len())
        .unwrap_or(usize::MAX);
    if observed > wire::media::MAX_MEDIA_ITEMS {
        return Err(ResolveError::Invalid {
            digest: external
                .first()
                .map_or_else(|| "-".to_owned(), |reference| reference.sha256().to_owned()),
            source: wire::media::MediaError::Count {
                observed,
                maximum: wire::media::MAX_MEDIA_ITEMS,
            },
        });
    }
    let mut resolved = Vec::with_capacity(external.len());
    for reference in external.iter() {
        resolved.push(resolve_one(root, reference)?);
    }
    media.extend(resolved);
    external.clear();
    Ok(())
}

fn resolve_one(
    root: &Path,
    reference: &listen::ExternalMedia,
) -> Result<wire::Media, ResolveError> {
    let digest = reference.sha256();
    let path = root.join(digest);
    let metadata = std::fs::symlink_metadata(&path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            ResolveError::Missing {
                digest: digest.to_owned(),
                path: path.clone(),
            }
        } else {
            ResolveError::Unreadable {
                digest: digest.to_owned(),
                path: path.clone(),
                source,
            }
        }
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ResolveError::NotRegular {
            digest: digest.to_owned(),
            path,
        });
    }
    let maximum = wire::media::MAX_MEDIA_BYTES;
    if metadata.len() == 0 || metadata.len() > maximum as u64 {
        return Err(ResolveError::Size {
            digest: digest.to_owned(),
            observed: metadata.len(),
            maximum,
        });
    }
    let file = File::open(&path).map_err(|source| ResolveError::Unreadable {
        digest: digest.to_owned(),
        path: path.clone(),
        source,
    })?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((maximum + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|source| ResolveError::Unreadable {
            digest: digest.to_owned(),
            path: path.clone(),
            source,
        })?;
    if bytes.is_empty() || bytes.len() > maximum {
        return Err(ResolveError::Size {
            digest: digest.to_owned(),
            observed: bytes.len() as u64,
            maximum,
        });
    }
    let observed = Sha256::digest(&bytes);
    if !digest_matches(digest, &observed) {
        return Err(ResolveError::Digest {
            expected: digest.to_owned(),
            observed: format!("{observed:x}"),
        });
    }
    wire::Media::from_bytes(reference.name(), reference.content_type(), &bytes).map_err(|source| {
        ResolveError::Invalid {
            digest: digest.to_owned(),
            source,
        }
    })
}

fn digest_matches(expected: &str, observed: &[u8]) -> bool {
    expected.len() == observed.len() * 2
        && expected
            .as_bytes()
            .chunks_exact(2)
            .zip(observed)
            .all(|(expected, observed)| {
                hex_nibble(expected[0])
                    .zip(hex_nibble(expected[1]))
                    .is_some_and(|(high, low)| ((high << 4) | low) == *observed)
            })
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn media_root(transcript: &Path) -> Option<PathBuf> {
    let layout = vendor::OMP.transcript_layout();
    let directory = layout.media_directory?;
    transcript
        .ancestors()
        .skip(1)
        .find(|ancestor| ancestor.file_name() == Some(OsStr::new(layout.sessions_directory)))
        .and_then(Path::parent)
        .map(|root| root.join(directory))
}

#[derive(Debug)]
pub(crate) enum ResolveError {
    Root {
        transcript: PathBuf,
    },
    Missing {
        digest: String,
        path: PathBuf,
    },
    Unreadable {
        digest: String,
        path: PathBuf,
        source: std::io::Error,
    },
    NotRegular {
        digest: String,
        path: PathBuf,
    },
    Size {
        digest: String,
        observed: u64,
        maximum: usize,
    },
    Digest {
        expected: String,
        observed: String,
    },
    Invalid {
        digest: String,
        source: wire::media::MediaError,
    },
}

impl ResolveError {
    pub(crate) const fn kind(&self) -> &'static str {
        match self {
            Self::Root { .. } => "media-blob-root-unavailable",
            Self::Missing { .. } => "media-blob-missing",
            Self::Unreadable { .. } => "media-blob-unreadable",
            Self::NotRegular { .. } => "media-blob-not-regular",
            Self::Size { .. } => "media-blob-size-invalid",
            Self::Digest { .. } => "media-blob-digest-mismatch",
            Self::Invalid { .. } => "media-blob-content-invalid",
        }
    }

    pub(crate) const fn recovery(&self) -> &'static str {
        "restore the exact regular OMP blob beneath its session state root; surrounding typed history remains available"
    }
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root { transcript } => write!(
                formatter,
                "transcript {} is not beneath the declared OMP sessions directory",
                transcript.display()
            ),
            Self::Missing { digest, path } => {
                write!(
                    formatter,
                    "OMP media blob {digest} is absent at {}",
                    path.display()
                )
            }
            Self::Unreadable {
                digest,
                path,
                source,
            } => write!(
                formatter,
                "OMP media blob {digest} at {} is unreadable: {source}",
                path.display()
            ),
            Self::NotRegular { digest, path } => write!(
                formatter,
                "OMP media blob {digest} at {} is not a regular non-symbolic file",
                path.display()
            ),
            Self::Size {
                digest,
                observed,
                maximum,
            } => write!(
                formatter,
                "OMP media blob {digest} is {observed} bytes, outside 1..={maximum}"
            ),
            Self::Digest { expected, observed } => write!(
                formatter,
                "OMP media blob digest is {observed}; reference named {expected}"
            ),
            Self::Invalid { digest, source } => {
                write!(
                    formatter,
                    "OMP media blob {digest} is not admitted media: {source}"
                )
            }
        }
    }
}

impl std::error::Error for ResolveError {}
