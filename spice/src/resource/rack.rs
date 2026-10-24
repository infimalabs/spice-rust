//! Reads of this executable's spice rack: one member's text, or the members
//! beneath a namespace or directory as a `resources:` footer.

use serve::rack::{Namespace, Rack};
use wire::{CanonicalUuid, Digest};

use super::{Request, Resource, SCHEME, Selector, push_resources};

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

/// [`document`] against the rack linked into this executable.
pub(super) fn linked(namespace: Namespace, path: &str, url: &str) -> Result<String, String> {
    match spice::version::canvas() {
        Some(Ok(linked)) => document(linked.rack(), namespace, path, url),
        Some(Err(error)) => Err(format!(
            "{url} names a rack member, but this executable's spice rack is refused: {error}"
        )),
        None => Err(format!(
            "{url} names a rack member, but this executable links no spice rack"
        )),
    }
}

/// The text of one rack member, or, when `path` names a namespace or
/// directory, its members as a `resources:` footer.
pub(super) fn document(
    rack: &Rack<'_>,
    namespace: Namespace,
    path: &str,
    url: &str,
) -> Result<String, String> {
    let member = namespace.member(path);
    if let Some(stored) = rack.member(&member) {
        let bytes = stored.inflate(&member).map_err(|error| error.to_string())?;
        return String::from_utf8(bytes.into_vec())
            .map_err(|_| format!("{url} is not UTF-8 text, and rack members are read as text"));
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
