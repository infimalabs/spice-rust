//! One admitted representation for browsable transcript and steering media.
//!
//! The data URL is retained because the browser can render it without a second
//! object store. Its decoded bytes are nevertheless the authority: admission
//! checks the declared type against both the URL and the file signature, fixes
//! one canonical base64 spelling, and derives identity from the decoded bytes.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};

use crate::{Digest, Domain};

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

/// Largest canonical base64 spelling that could decode within the byte bound.
const MAX_MEDIA_BASE64_BYTES: usize = MAX_MEDIA_BYTES.div_ceil(3) * 4;

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
        self.decoded().map(|_| ())
    }

    /// Canonical content identity derived from decoded bytes.
    ///
    /// # Errors
    ///
    /// Returns an admission error rather than minting identity for invalid
    /// media.
    pub fn identity(&self) -> Result<Digest, MediaError> {
        self.decoded()
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
        self.validate()?;
        let mut remaining = text;
        while let Some(start) = remaining.find("![") {
            remaining = &remaining[start + 2..];
            let Some(label_end) = remaining.find("](") else {
                break;
            };
            let name = &remaining[..label_end];
            remaining = &remaining[label_end + 2..];
            let Some(url_end) = remaining.find(')') else {
                break;
            };
            let url = &remaining[..url_end];
            remaining = &remaining[url_end + 1..];
            if url == self.data_url && Self::admit(name, &self.content_type, url).is_ok() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn decoded(&self) -> Result<Vec<u8>, MediaError> {
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
    fn markdown_escapes_operator_facing_alt_text_without_changing_identity() {
        let media = Media::from_base64(r"capture\].png", "image/png", PNG).unwrap();
        assert_eq!(
            media.markdown().unwrap(),
            format!(r"![capture\\\].png](data:image/png;base64,{PNG})")
        );
        assert_eq!(
            media.identity().unwrap(),
            Media::from_base64("other-name.png", "image/png", PNG)
                .unwrap()
                .identity()
                .unwrap(),
        );
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
