//! One admitted representation for browsable transcript and steering media.
//!
//! The data URL is retained because the browser can render it without a second
//! object store. Its decoded bytes are nevertheless the authority: admission
//! checks the declared type against both the URL and the file signature, fixes
//! one canonical base64 spelling, and derives identity from the decoded bytes.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{CanonicalUuid, Digest, Domain};

/// Maximum media blocks one message or tool result may carry.
///
/// identity-law: non-crossing - local admission bound, not an encoded value.
pub const MAX_MEDIA_ITEMS: usize = 8;

/// Maximum decoded bytes in one media block.
///
/// identity-law: non-crossing - local admission bound, not an encoded value.
pub const MAX_MEDIA_BYTES: usize = 8 * 1_024 * 1_024;

/// Maximum UTF-8 bytes retained for an operator-facing media name.
const MAX_MEDIA_NAME_BYTES: usize = 255;

/// Maximum encoded bytes in one agent-facing media reference.
///
/// The bound covers the largest admitted name after JSON escaping plus every
/// fixed-width identity and scalar field. It is independent of media bytes.
pub const MAX_AGENT_MEDIA_REFERENCE_BYTES: usize = 1_024;

/// Largest canonical base64 spelling that could decode within the byte bound.
const MAX_MEDIA_BASE64_BYTES: usize = MAX_MEDIA_BYTES.div_ceil(3) * 4;

/// One bounded agent-facing reference to journal-backed steering media.
///
/// This value deliberately cannot carry the data URL. The durable directive
/// remains the sole byte authority and callers must resolve this reference
/// against its live steering ledger before consuming the decoded bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentMediaReference {
    /// Durable directive that owns the attachment.
    directive: CanonicalUuid,
    /// Decoded-byte content identity.
    digest: Digest,
    /// Admitted operator-facing name.
    name: String,
    /// Exact admitted media type.
    content_type: String,
    /// Decoded byte count.
    bytes: u64,
    /// Zero-based position in the directive's attachment list.
    ordinal: u8,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentMediaReferenceFields {
    directive: CanonicalUuid,
    digest: Digest,
    name: String,
    content_type: String,
    bytes: u64,
    ordinal: u8,
}

impl<'de> Deserialize<'de> for AgentMediaReference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let fields = AgentMediaReferenceFields::deserialize(deserializer)?;
        let reference = Self {
            directive: fields.directive,
            digest: fields.digest,
            name: fields.name,
            content_type: fields.content_type,
            bytes: fields.bytes,
            ordinal: fields.ordinal,
        };
        reference.validate().map_err(serde::de::Error::custom)?;
        Ok(reference)
    }
}

impl AgentMediaReference {
    /// Durable directive that owns the attachment.
    #[must_use]
    pub const fn directive(&self) -> CanonicalUuid {
        self.directive
    }

    /// Decoded-byte content identity.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }

    /// Admitted operator-facing name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Exact admitted media type.
    #[must_use]
    pub fn content_type(&self) -> &str {
        &self.content_type
    }

    /// Decoded byte count.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Zero-based position in the directive's attachment list.
    #[must_use]
    pub const fn ordinal(&self) -> u8 {
        self.ordinal
    }

    /// Derive one compact reference from admitted durable media.
    ///
    /// # Errors
    ///
    /// Returns a typed error if the authority media or source ordinal is not
    /// admitted, or if the resulting reference exceeds its fixed bound.
    pub fn from_media(
        directive: CanonicalUuid,
        ordinal: usize,
        media: &Media,
    ) -> Result<Self, AgentMediaReferenceError> {
        let ordinal = u8::try_from(ordinal).map_err(|_| AgentMediaReferenceError::Ordinal {
            observed: ordinal,
            maximum: MAX_MEDIA_ITEMS - 1,
        })?;
        let bytes = media
            .decoded_bytes()
            .map_err(AgentMediaReferenceError::Media)?;
        let reference = Self {
            directive,
            digest: Digest::compute(Domain::MediaContent, &bytes),
            name: media.name.clone(),
            content_type: media.content_type.clone(),
            bytes: u64::try_from(bytes.len()).expect("admitted media length fits u64"),
            ordinal,
        };
        reference.validate()?;
        Ok(reference)
    }

    /// Validate every independently bounded reference field and its encoding.
    ///
    /// # Errors
    ///
    /// Returns the exact malformed field or encoded-size violation.
    pub fn validate(&self) -> Result<(), AgentMediaReferenceError> {
        if self.name.is_empty()
            || self.name.len() > MAX_MEDIA_NAME_BYTES
            || self.name.chars().any(char::is_control)
        {
            return Err(AgentMediaReferenceError::Name);
        }
        if !supported_type(&self.content_type) {
            return Err(AgentMediaReferenceError::ContentType(
                self.content_type.clone(),
            ));
        }
        if usize::from(self.ordinal) >= MAX_MEDIA_ITEMS {
            return Err(AgentMediaReferenceError::Ordinal {
                observed: usize::from(self.ordinal),
                maximum: MAX_MEDIA_ITEMS - 1,
            });
        }
        let maximum = u64::try_from(MAX_MEDIA_BYTES).expect("media byte bound fits u64");
        if self.bytes == 0 || self.bytes > maximum {
            return Err(AgentMediaReferenceError::ByteCount {
                observed: self.bytes,
                maximum,
            });
        }
        let encoded = serde_json::to_vec(self)
            .expect("an agent media reference contains only infallible JSON values");
        if encoded.len() > MAX_AGENT_MEDIA_REFERENCE_BYTES {
            return Err(AgentMediaReferenceError::EncodedSize {
                observed: encoded.len(),
                maximum: MAX_AGENT_MEDIA_REFERENCE_BYTES,
            });
        }
        Ok(())
    }
}

/// Why an agent media reference could not be derived or admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentMediaReferenceError {
    /// Durable media did not satisfy the shared media admission law.
    Media(MediaError),
    /// The retained media name was not admitted.
    Name,
    /// The retained media type was not admitted.
    ContentType(String),
    /// The source position exceeded the bounded attachment set.
    Ordinal { observed: usize, maximum: usize },
    /// The decoded byte count was empty or exceeded the media bound.
    ByteCount { observed: u64, maximum: u64 },
    /// The complete serialized reference exceeded its independent bound.
    EncodedSize { observed: usize, maximum: usize },
}

impl fmt::Display for AgentMediaReferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Media(error) => write!(formatter, "media authority is invalid: {error}"),
            Self::Name => formatter.write_str(
                "media reference name must be non-empty, control-free, and at most 255 UTF-8 bytes",
            ),
            Self::ContentType(found) => {
                write!(formatter, "media reference type {found:?} is unsupported")
            }
            Self::Ordinal { observed, maximum } => write!(
                formatter,
                "media reference ordinal {observed} exceeds maximum {maximum}",
            ),
            Self::ByteCount { observed, maximum } => write!(
                formatter,
                "media reference byte count {observed} is outside 1..={maximum}",
            ),
            Self::EncodedSize { observed, maximum } => write!(
                formatter,
                "media reference encoding is {observed} bytes, above maximum {maximum}",
            ),
        }
    }
}

impl std::error::Error for AgentMediaReferenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Media(error) => Some(error),
            _ => None,
        }
    }
}

/// One exact image retained in a durable steering or transcript record.
///
/// The surrounding record owns causality: `SteerSent.directive` identifies an
/// operator attachment, while `Tool.call_id` and its tape locus identify tool
/// output. Keeping source authority outside the content value lets the same
/// admitted bytes cross both paths without inventing a second media schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Media {
    /// Operator-facing or tool-supplied label.
    pub name: String,
    /// Exact lower-case supported media type.
    pub content_type: String,
    /// Canonical base64 data URL carrying the exact bytes.
    pub data_url: String,
}

impl Media {
    /// Admit one already encoded data URL.
    ///
    /// # Errors
    ///
    /// Returns a typed error for an invalid name, URL, size, base64 spelling,
    /// unsupported type, or mismatch between declared and observed bytes.
    pub fn admit(
        name: impl Into<String>,
        content_type: impl Into<String>,
        data_url: impl Into<String>,
    ) -> Result<Self, MediaError> {
        let media = Self {
            name: name.into(),
            content_type: content_type.into(),
            data_url: data_url.into(),
        };
        media.validate()?;
        Ok(media)
    }

    /// Build the canonical data URL for one base64 payload.
    ///
    /// # Errors
    ///
    /// Returns the same admission errors as [`Self::admit`].
    pub fn from_base64(
        name: impl Into<String>,
        content_type: impl Into<String>,
        base64: &str,
    ) -> Result<Self, MediaError> {
        let content_type = content_type.into();
        Self::admit(
            name,
            content_type.clone(),
            format!("data:{content_type};base64,{base64}"),
        )
    }

    /// Validate this value at an emitting boundary.
    ///
    /// # Errors
    ///
    /// Returns the exact violated admission law.
    pub fn validate(&self) -> Result<(), MediaError> {
        self.decoded_bytes().map(|_| ())
    }

    /// Canonical content identity derived from decoded bytes.
    ///
    /// # Errors
    ///
    /// Returns an admission error rather than minting identity for invalid
    /// media.
    pub fn identity(&self) -> Result<Digest, MediaError> {
        self.decoded_bytes()
            .map(|bytes| Digest::compute(Domain::MediaContent, &bytes))
    }

    /// Render the admitted value as one canonical Markdown image block.
    ///
    /// # Errors
    ///
    /// Returns an admission error rather than rendering a forged value.
    pub fn markdown(&self) -> Result<String, MediaError> {
        self.validate()?;
        let name = self.name.replace('\\', "\\\\").replace(']', "\\]");
        Ok(format!("![{name}]({})", self.data_url))
    }

    /// Whether authored text already carries these canonical media bytes.
    ///
    /// Admitted media has one canonical data URL spelling and its observed byte
    /// signature fixes its media type, so finding that URL is equivalent to
    /// finding this value's decoded-byte identity in authored Markdown.
    ///
    /// # Errors
    ///
    /// Returns an admission error rather than comparing a forged value.
    pub fn is_embedded_in(&self, text: &str) -> Result<bool, MediaError> {
        let identity = self.identity()?;
        Ok(parse_markdown(text).into_iter().any(|candidate| {
            candidate
                .is_ok_and(|candidate| candidate.identity().is_ok_and(|found| found == identity))
        }))
    }

    /// Decode and revalidate the canonical byte authority.
    ///
    /// # Errors
    ///
    /// Returns the exact shared media admission error.
    pub fn decoded_bytes(&self) -> Result<Vec<u8>, MediaError> {
        if self.name.is_empty()
            || self.name.len() > MAX_MEDIA_NAME_BYTES
            || self.name.chars().any(char::is_control)
        {
            return Err(MediaError::Name);
        }
        if !supported_type(&self.content_type) {
            return Err(MediaError::UnsupportedType(self.content_type.clone()));
        }
        let prefix = format!("data:{};base64,", self.content_type);
        let payload = self
            .data_url
            .strip_prefix(&prefix)
            .ok_or(MediaError::DataUrl)?;
        if payload.is_empty() {
            return Err(MediaError::DataUrl);
        }
        if payload.len() > MAX_MEDIA_BASE64_BYTES {
            return Err(MediaError::Size {
                // Any standard base64 value this long decodes to at least this
                // many bytes; subtracting the maximum padding keeps the
                // refusal truthful without allocating attacker-sized input.
                observed: payload.len() / 4 * 3 - 2,
                maximum: MAX_MEDIA_BYTES,
            });
        }
        let bytes = STANDARD.decode(payload).map_err(|_| MediaError::Base64)?;
        if STANDARD.encode(&bytes) != payload {
            return Err(MediaError::Base64);
        }
        if bytes.is_empty() || bytes.len() > MAX_MEDIA_BYTES {
            return Err(MediaError::Size {
                observed: bytes.len(),
                maximum: MAX_MEDIA_BYTES,
            });
        }
        let observed = observed_type(&bytes).ok_or(MediaError::UnrecognizedBytes)?;
        if observed != self.content_type {
            return Err(MediaError::TypeMismatch {
                declared: self.content_type.clone(),
                observed: observed.to_owned(),
            });
        }
        Ok(bytes)
    }
}

/// Validate the bounded media set carried by one record.
///
/// identity-law: non-crossing - process-local validation of crossing values.
///
/// # Errors
///
/// Returns count admission before validating each member in order.
pub fn admit_media(media: &[Media]) -> Result<(), MediaError> {
    if media.len() > MAX_MEDIA_ITEMS {
        return Err(MediaError::Count {
            observed: media.len(),
            maximum: MAX_MEDIA_ITEMS,
        });
    }
    for item in media {
        item.validate()?;
    }
    Ok(())
}

/// Derive compact references for one directive's admitted media in source order.
///
/// # Errors
///
/// Returns count or member admission before producing a partial reference set.
pub fn agent_media_references(
    directive: CanonicalUuid,
    media: &[Media],
) -> Result<Vec<AgentMediaReference>, AgentMediaReferenceError> {
    admit_media(media).map_err(AgentMediaReferenceError::Media)?;
    media
        .iter()
        .enumerate()
        .map(|(ordinal, media)| AgentMediaReference::from_media(directive, ordinal, media))
        .collect()
}

/// Remove authoritative media payloads from one disposable text projection.
///
/// The compact references emitted beside the returned text retain attachment
/// identity and order. Durable authored text is not changed.
#[must_use]
pub fn without_media_payloads(text: &str, media: &[Media]) -> String {
    media.iter().fold(text.to_owned(), |projected, item| {
        projected.replace(&item.data_url, "")
    })
}

/// Extract every data-backed image block from Markdown in source order.
///
/// Each candidate retains its own admission result so transcript ingestion can
/// refuse malformed media while duplicate detection can continue looking for
/// a later valid identity. Delimiter recognition honors canonical Markdown
/// escapes, and labels are restored to their original admitted spelling.
#[must_use]
pub fn parse_markdown(text: &str) -> Vec<Result<Media, MediaError>> {
    let mut parsed = Vec::new();
    let mut cursor = 0usize;
    while let Some(relative_start) = find_unescaped(&text[cursor..], "![") {
        let start = cursor + relative_start;
        let label_start = start + 2;
        let Some(relative_label_end) = find_unescaped(&text[label_start..], "](") else {
            cursor = label_start;
            continue;
        };
        let label_end = label_start + relative_label_end;
        let url_start = label_end + 2;
        let Some(relative_url_end) = find_unescaped(&text[url_start..], ")") else {
            cursor = label_start;
            continue;
        };
        let url_end = url_start + relative_url_end;
        let url = &text[url_start..url_end];
        if !url.starts_with("data:image/") {
            cursor = label_start;
            continue;
        }
        let content_type = url
            .strip_prefix("data:")
            .and_then(|rest| rest.split_once(";base64,").map(|(kind, _)| kind));
        let candidate = content_type.map_or(Err(MediaError::DataUrl), |content_type| {
            Media::admit(
                unescape_markdown_label(&text[label_start..label_end]),
                content_type,
                url,
            )
        });
        cursor = if candidate.is_ok() {
            url_end + 1
        } else {
            label_start
        };
        parsed.push(candidate);
    }
    parsed
}

fn find_unescaped(text: &str, needle: &str) -> Option<usize> {
    text.match_indices(needle)
        .map(|(index, _)| index)
        .find(|index| {
            text.as_bytes()[..*index]
                .iter()
                .rev()
                .take_while(|byte| **byte == b'\\')
                .count()
                % 2
                == 0
        })
}

fn unescape_markdown_label(label: &str) -> String {
    let mut restored = String::with_capacity(label.len());
    let mut characters = label.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            restored.push(character);
            continue;
        }
        match characters.next() {
            Some('\\') => restored.push('\\'),
            Some(']') => restored.push(']'),
            Some(other) => {
                restored.push('\\');
                restored.push(other);
            }
            None => restored.push('\\'),
        }
    }
    restored
}

fn supported_type(content_type: &str) -> bool {
    matches!(
        content_type,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    )
}

fn observed_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Why one media value was not admitted.
///
/// identity-law: non-crossing - local rejection converted at the emitting door.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaError {
    /// A record carried too many media blocks.
    Count { observed: usize, maximum: usize },
    /// A media name was empty, too long, or contained control characters.
    Name,
    /// The declared type is outside the raster-image vocabulary.
    UnsupportedType(String),
    /// The URL did not exactly bind its declared type to canonical base64.
    DataUrl,
    /// Base64 was invalid or had a non-canonical spelling.
    Base64,
    /// Decoded bytes were empty or exceeded the per-block bound.
    Size { observed: usize, maximum: usize },
    /// The bytes carried no supported image signature.
    UnrecognizedBytes,
    /// The declared and signature-observed media types differed.
    TypeMismatch { declared: String, observed: String },
}

impl fmt::Display for MediaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Count { observed, maximum } => {
                write!(
                    formatter,
                    "media count {observed} exceeds maximum {maximum}"
                )
            }
            Self::Name => formatter.write_str(
                "media name must be non-empty, control-free, and at most 255 UTF-8 bytes",
            ),
            Self::UnsupportedType(found) => write!(
                formatter,
                "media type {found:?} is not one of image/png, image/jpeg, image/gif, image/webp",
            ),
            Self::DataUrl => formatter.write_str(
                "media data URL must exactly spell data:<declared-type>;base64,<payload>",
            ),
            Self::Base64 => formatter.write_str("media payload must be canonical standard base64"),
            Self::Size { observed, maximum } => {
                write!(
                    formatter,
                    "media size {observed} is outside 1..={maximum} bytes"
                )
            }
            Self::UnrecognizedBytes => {
                formatter.write_str("media bytes have no supported image signature")
            }
            Self::TypeMismatch { declared, observed } => write!(
                formatter,
                "declared media type {declared:?} does not match observed {observed:?}",
            ),
        }
    }
}

impl std::error::Error for MediaError {}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &str = "iVBORw0KGgo=";

    #[test]
    fn canonical_png_has_one_byte_identity() {
        let media = Media::from_base64("capture.png", "image/png", PNG).unwrap();
        assert_eq!(media.identity().unwrap(), media.identity().unwrap());
        assert_eq!(media.data_url, format!("data:image/png;base64,{PNG}"));
        assert_eq!(
            media.markdown().unwrap(),
            format!("![capture.png](data:image/png;base64,{PNG})")
        );
        assert!(media.is_embedded_in(&media.markdown().unwrap()).unwrap());
        assert!(
            !media
                .is_embedded_in(&format!("raw URL only: {}", media.data_url))
                .unwrap(),
            "a bare URL is not browsable authored Markdown",
        );
    }

    #[test]
    fn canonical_markdown_round_trips_every_admitted_label_character() {
        for name in [
            r"capture](detail).png",
            r"slash\](detail).png",
            "雪](図).png",
            r"punctuation !#$%&'()*+,-.;=@^_`{}~.png",
        ] {
            let media = Media::from_base64(name, "image/png", PNG).unwrap();
            let markdown = media.markdown().unwrap();
            let parsed = parse_markdown(&markdown);

            assert_eq!(parsed.len(), 1, "one block expected for {name:?}");
            let parsed = parsed.into_iter().next().unwrap().unwrap();
            assert_eq!(parsed.name, name);
            assert_eq!(parsed.data_url, media.data_url);
            assert_eq!(parsed.identity().unwrap(), media.identity().unwrap());
            assert!(media.is_embedded_in(&markdown).unwrap());
        }
    }

    #[test]
    fn broken_image_candidate_cannot_hide_later_canonical_media() {
        let media = Media::from_base64("capture.png", "image/png", PNG).unwrap();
        let canonical = media.markdown().unwrap();
        for broken in [
            "![broken](not-an-image ",
            "![broken](data:image/png;base64,%%% ",
        ] {
            let text = format!("{broken}{canonical}");
            let parsed = parse_markdown(&text);

            assert!(
                parsed
                    .into_iter()
                    .any(|candidate| { candidate.is_ok_and(|candidate| candidate == media) })
            );
            assert!(media.is_embedded_in(&text).unwrap());
        }
    }

    #[test]
    fn declared_type_must_match_observed_bytes() {
        let error = Media::from_base64("capture.jpg", "image/jpeg", PNG).unwrap_err();
        assert_eq!(
            error,
            MediaError::TypeMismatch {
                declared: "image/jpeg".to_owned(),
                observed: "image/png".to_owned(),
            }
        );
    }

    #[test]
    fn count_is_bounded_before_members_are_read() {
        let media = Media::from_base64("capture.png", "image/png", PNG).unwrap();
        let error = admit_media(&vec![media; MAX_MEDIA_ITEMS + 1]).unwrap_err();
        assert_eq!(
            error,
            MediaError::Count {
                observed: MAX_MEDIA_ITEMS + 1,
                maximum: MAX_MEDIA_ITEMS,
            }
        );
    }

    #[test]
    fn malformed_unsupported_and_oversized_payloads_are_distinct_refusals() {
        assert_eq!(
            Media::admit("capture.png", "image/png", "not-a-data-url").unwrap_err(),
            MediaError::DataUrl,
        );
        assert_eq!(
            Media::admit("capture.png", "image/png", "data:image/png;base64,%%%",).unwrap_err(),
            MediaError::Base64,
        );
        assert_eq!(
            Media::admit(
                "capture.txt",
                "text/plain",
                "data:text/plain;base64,aGVsbG8=",
            )
            .unwrap_err(),
            MediaError::UnsupportedType("text/plain".to_owned()),
        );

        let mut bytes = b"RIFF\0\0\0\0WEBP".to_vec();
        bytes.resize(MAX_MEDIA_BYTES + 1, 0);
        let oversized = Media::from_base64("oversized.webp", "image/webp", &STANDARD.encode(bytes))
            .unwrap_err();
        assert_eq!(
            oversized,
            MediaError::Size {
                observed: MAX_MEDIA_BYTES + 1,
                maximum: MAX_MEDIA_BYTES,
            }
        );
    }
}
