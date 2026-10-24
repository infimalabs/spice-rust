//! The one boundary the resident serves browser app routes through.
//!
//! A running resident answers from exactly one of two sources: the staged
//! generation directory of a staged `spice up`, which hot-swaps as new
//! generations are published, or the admitted canvas archive an installed
//! executable carries, which holds one generation for the process lifetime.
//! A route qualified with a generation the source does not hold is gone; the
//! page answers that by reloading once into the generation now served.

use std::collections::BTreeMap;
use std::sync::Arc;

use ::axum::body::Bytes;
use ::axum::http::HeaderValue;

use crate::asset::{APP, AppSource, AppSourceError, generation_route};
use crate::rack::{Namespace, Rack, RackError};

/// The browser app source a resident serves.
#[derive(Clone, Debug)]
pub enum CanvasAssets {
    /// The staged generation directory with its hot swap and two-entry
    /// generation cache, selected only for a staged `spice up`.
    DevelopmentDirectory(AppSource),
    /// The admitted archive linked into an installed executable; it reads no
    /// app file from disk.
    EmbeddedArchive(EmbeddedArchive),
}

/// What one app route answers.
#[derive(Debug)]
pub(crate) enum CanvasAnswer {
    Asset(CanvasAsset),
    /// The route names a generation this source no longer holds; it carries
    /// the generation served now.
    Gone(wire::Digest),
    NotFound,
}

/// One response body with the metadata its headers need.
#[derive(Debug)]
pub(crate) struct CanvasAsset {
    pub(crate) body: Bytes,
    pub(crate) content_type: &'static str,
    pub(crate) etag: HeaderValue,
    /// The body is one stored gzip stream, served as `Content-Encoding: gzip`.
    pub(crate) gzip: bool,
    /// The inflated HTML document a content security policy is derived from.
    pub(crate) document: Option<Arc<[u8]>>,
}

impl CanvasAssets {
    /// The generation the shell route serves now.
    ///
    /// # Errors
    ///
    /// Returns the staged directory's pointer, inventory, or read failure.
    pub fn generation(&self) -> Result<wire::Digest, AppSourceError> {
        match self {
            Self::DevelopmentDirectory(source) => {
                Ok(source.current_generation()?.identity().clone())
            }
            Self::EmbeddedArchive(archive) => Ok(archive.0.generation.clone()),
        }
    }

    /// Answer one exact route. Unknown paths have no shell fallback.
    pub(crate) fn answer(&self, route: &str) -> Result<CanvasAnswer, AppSourceError> {
        match self {
            Self::DevelopmentDirectory(source) => {
                if let Some((identity, _)) = generation_route(route)
                    && !source.holds(&identity)
                {
                    return Ok(CanvasAnswer::Gone(
                        source.current_generation()?.identity().clone(),
                    ));
                }
                Ok(source
                    .served(route)?
                    .map_or(CanvasAnswer::NotFound, |asset| {
                        let etag = HeaderValue::from_str(&format!("\"{}\"", asset.digest()))
                            .expect("a canonical digest is an admitted ETag");
                        let content_type = asset.content_type();
                        let bytes = asset.into_shared_bytes();
                        CanvasAnswer::Asset(CanvasAsset {
                            document: is_document(content_type).then(|| Arc::clone(&bytes)),
                            body: Bytes::from_owner(bytes),
                            content_type,
                            etag,
                            gzip: false,
                        })
                    }))
            }
            Self::EmbeddedArchive(archive) => Ok(archive.answer(route)),
        }
    }
}

impl std::fmt::Display for CanvasAssets {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DevelopmentDirectory(source) => source.fmt(formatter),
            Self::EmbeddedArchive(archive) => {
                write!(formatter, "embedded app {}", archive.0.generation)
            }
        }
    }
}

/// The admitted rack of an installed executable, its app members indexed once.
#[derive(Clone)]
pub struct EmbeddedArchive(Arc<Embedded>);

struct Embedded {
    generation: wire::Digest,
    members: BTreeMap<&'static str, EmbeddedMember>,
    rack: Rack<'static>,
}

struct EmbeddedMember {
    gzip: &'static [u8],
    content_type: &'static str,
    etag: HeaderValue,
    document: Option<Arc<[u8]>>,
}

impl EmbeddedArchive {
    /// Admit the executable's rack and index every app member once: its
    /// stored gzip slice, its inventory MIME type, and its strong ETag. HTML
    /// members are inflated here, once, for their content security policies.
    ///
    /// # Errors
    ///
    /// Refuses everything [`Rack::admit`] refuses and an HTML member that is
    /// not exactly one gzip stream.
    pub fn admit(bytes: &'static [u8]) -> Result<Self, RackError> {
        let rack = Rack::admit(bytes)?;
        let generation = rack.generation().clone();
        let members = rack
            .namespace(Namespace::App)
            .map(|(path, member)| {
                let content_type =
                    inventory_content_type(path).ok_or_else(|| RackError::Member {
                        path: Namespace::App.member(path),
                        reason: "is outside the shell inventory",
                    })?;
                let document = if !is_document(content_type) {
                    None
                } else if path == wire::manifest::APP_HTML {
                    Some(Arc::from(rack.shell()))
                } else {
                    Some(Arc::from(member.inflate(&Namespace::App.member(path))?))
                };
                let etag = HeaderValue::from_str(&format!("\"{generation}/{path}\""))
                    .expect("a digest and inventory path are an admitted ETag");
                Ok((
                    path,
                    EmbeddedMember {
                        gzip: member.gzip(),
                        content_type,
                        etag,
                        document,
                    },
                ))
            })
            .collect::<Result<_, RackError>>()?;
        Ok(Self(Arc::new(Embedded {
            generation,
            members,
            rack,
        })))
    }

    /// The whole admitted rack: every namespace, not only the app it serves.
    #[must_use]
    pub fn rack(&self) -> &Rack<'static> {
        &self.0.rack
    }

    /// The immutable generation this embedded source serves.
    #[must_use]
    pub fn generation(&self) -> &wire::Digest {
        &self.0.generation
    }

    fn answer(&self, route: &str) -> CanvasAnswer {
        let path = if route == "/" {
            wire::manifest::APP_HTML
        } else {
            let Some((identity, file)) = generation_route(route) else {
                return CanvasAnswer::NotFound;
            };
            if identity != self.0.generation {
                return CanvasAnswer::Gone(self.0.generation.clone());
            }
            file
        };
        self.0
            .members
            .get(path)
            .map_or(CanvasAnswer::NotFound, |member| {
                CanvasAnswer::Asset(CanvasAsset {
                    body: Bytes::from_static(member.gzip),
                    content_type: member.content_type,
                    etag: member.etag.clone(),
                    gzip: true,
                    document: member.document.clone(),
                })
            })
    }
}

impl std::fmt::Debug for EmbeddedArchive {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EmbeddedArchive")
            .field("generation", &self.0.generation)
            .field("members", &self.0.members.len())
            .finish()
    }
}

fn inventory_content_type(path: &str) -> Option<&'static str> {
    APP.asset_file(path)
        .map(crate::asset::AppAsset::content_type)
        .or_else(|| {
            wire::manifest::renderer::File::parse_filename(path)
                .map(|file| file.kind().content_type())
        })
}

fn is_document(content_type: &str) -> bool {
    content_type == wire::manifest::renderer::Kind::Document.content_type()
}
