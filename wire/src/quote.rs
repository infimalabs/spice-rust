//! Canonical transcript quote targets and their resolved wire shape.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{Actor, CanonicalUuid, Digest, Disposition, MessageLocus};

const PREFIX: &str = "spice://quote/v1/";
const MARKDOWN_PREFIX: &str = "[quote](";

/// Greatest number of nested quote targets one expansion may follow.
pub const QUOTE_DEPTH_LIMIT: u8 = 8;

/// One durable source of quoted transcript text.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum QuoteTarget {
    /// One typed event in an exact lane tape generation.
    Tape {
        /// Lane whose vendor tape owns the record.
        lane: CanonicalUuid,
        /// Opaque tape generation identity.
        source: CanonicalUuid,
        /// Exact record and event coordinates.
        locus: MessageLocus,
        /// Digest of the resolved UTF-8 prose.
        digest: Digest,
    },
    /// One directive in the journal steering-key index.
    Steering {
        /// Human-sized durable directive key.
        key: String,
        /// Digest of the directive's original UTF-8 text.
        digest: Digest,
    },
}

/// Why text resembling a quote target is not admitted.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum QuoteTargetError {
    /// The fixed scheme, authority, version, or path shape differs.
    #[error("quote target must use the canonical spice://quote/v1 grammar")]
    Shape,
    /// One typed path coordinate is malformed or non-canonical.
    #[error("invalid quote target {field}: {value:?}")]
    Coordinate {
        /// Coordinate whose parser refused the value.
        field: &'static str,
        /// Refused source text.
        value: String,
    },
    /// A steering key cannot cross a URI path segment.
    #[error("invalid quote target steering key: {0:?}")]
    SteeringKey(String),
}

/// Recorded principal that authored resolved quote text.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum QuoteAuthor {
    /// Agent prose emitted by one occupant of a lane.
    Agent {
        /// Logical lane carrying the transcript.
        lane: CanonicalUuid,
        /// Occupant attributed by durable residency authority.
        occupant: CanonicalUuid,
    },
    /// Journal directive created by an authenticated principal.
    Directive {
        /// Actor on the directive's creating fact.
        actor: Actor,
    },
}

/// Successfully resolved quote text and live journal presentation data.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedQuote {
    /// Exact authenticated UTF-8 text.
    pub text: String,
    /// Durable recorded authorship.
    pub author: QuoteAuthor,
    /// Current directive disposition; absent for tape prose.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub disposition: Option<Disposition>,
}

/// Closed refusal reasons exposed instead of invented quote content.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QuoteUnavailable {
    /// The target does not follow the admitted URI grammar.
    Malformed,
    /// The named source, event, or steering key does not exist.
    Unknown,
    /// Durable authority could not be read or decoded.
    Unreadable,
    /// Resolved text differs from the target's digest.
    DigestMismatch,
    /// Nested resolution reached [`QUOTE_DEPTH_LIMIT`].
    DepthExceeded,
}

/// One quote lookup result; unavailable values carry no authored content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum QuoteResolution {
    /// Named authority authenticated and supplied the quote.
    Resolved(ResolvedQuote),
    /// Resolution refused without fabricating text or authorship.
    Unavailable {
        /// Original target spelling that could not be resolved.
        target: String,
        /// Typed refusal class.
        reason: QuoteUnavailable,
    },
}
/// One recursively expanded fixed-label quote link ready for presentation.
///
/// Every node retains the exact original target. Resolved nodes carry only
/// authenticated source data, while unavailable nodes carry one closed refusal
/// class and no invented text or author.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum QuotePresentation {
    /// Durable authority authenticated this node and any nested quote links.
    Resolved {
        /// Original target spelling embedded in the containing message.
        target: String,
        /// Exact authenticated UTF-8 text.
        text: String,
        /// Durable recorded authorship.
        author: QuoteAuthor,
        /// Current directive disposition; absent for tape prose.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::optional::present"
        )]
        disposition: Option<Disposition>,
        /// Quote links found in `text`, in source order.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        quotes: Vec<QuotePresentation>,
    },
    /// Resolution refused without fabricating presentation data.
    Unavailable {
        /// Original target spelling embedded in the containing message.
        target: String,
        /// Typed refusal class.
        reason: QuoteUnavailable,
    },
}

impl QuoteTarget {
    /// Digest the exact text a target authenticates.
    #[must_use]
    pub fn text_digest(text: &str) -> Digest {
        Digest::compute(crate::Domain::QuoteText, text.as_bytes())
    }

    /// Render this target as the ordinary Markdown link stored in message text.
    #[must_use]
    pub fn markdown(&self) -> String {
        format!("{MARKDOWN_PREFIX}{self})")
    }

    /// Parse one whole fixed-label Markdown quote link.
    pub fn from_markdown(text: &str) -> Result<Self, QuoteTargetError> {
        let target = text
            .strip_prefix(MARKDOWN_PREFIX)
            .and_then(|text| text.strip_suffix(')'))
            .ok_or(QuoteTargetError::Shape)?;
        target.parse()
    }

    /// Digest carried by either target kind.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        match self {
            Self::Tape { digest, .. } | Self::Steering { digest, .. } => digest,
        }
    }
}

impl fmt::Display for QuoteTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(PREFIX)?;
        match self {
            Self::Tape {
                lane,
                source,
                locus,
                digest,
            } => write!(
                formatter,
                "tape/{lane}/{source}/{}/{}/{}/{}/{}",
                locus.member, locus.offset, locus.length, locus.event, digest
            ),
            Self::Steering { key, digest } => {
                write!(formatter, "steer/{key}/{digest}")
            }
        }
    }
}

impl FromStr for QuoteTarget {
    type Err = QuoteTargetError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let path = text.strip_prefix(PREFIX).ok_or(QuoteTargetError::Shape)?;
        let segments = path.split('/').collect::<Vec<_>>();
        match segments.as_slice() {
            ["tape", lane, source, member, offset, length, event, digest] => Ok(Self::Tape {
                lane: coordinate("lane", lane)?,
                source: coordinate("source", source)?,
                locus: MessageLocus {
                    member: number("member", member)?,
                    offset: number("offset", offset)?,
                    length: number("length", length)?,
                    event: number("event", event)?,
                },
                digest: coordinate("digest", digest)?,
            }),
            ["steer", key, digest] if steering_key_segment(key) => Ok(Self::Steering {
                key: (*key).to_owned(),
                digest: coordinate("digest", digest)?,
            }),
            ["steer", key, _] => Err(QuoteTargetError::SteeringKey((*key).to_owned())),
            _ => Err(QuoteTargetError::Shape),
        }
    }
}

fn coordinate<T>(field: &'static str, text: &str) -> Result<T, QuoteTargetError>
where
    T: FromStr + fmt::Display,
{
    let parsed = text
        .parse::<T>()
        .map_err(|_| QuoteTargetError::Coordinate {
            field,
            value: text.to_owned(),
        })?;
    if parsed.to_string() != text {
        return Err(QuoteTargetError::Coordinate {
            field,
            value: text.to_owned(),
        });
    }
    Ok(parsed)
}

fn number<T>(field: &'static str, text: &str) -> Result<T, QuoteTargetError>
where
    T: FromStr + fmt::Display,
{
    coordinate(field, text)
}

fn steering_key_segment(key: &str) -> bool {
    matches!(key.len(), 8 | 9) && key.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02x}")
            .parse()
            .expect("canonical UUID")
    }

    #[test]
    fn both_targets_round_trip_through_canonical_markdown() {
        let text = "the quoted words";
        let targets = [
            QuoteTarget::Tape {
                lane: uuid(1),
                source: uuid(2),
                locus: MessageLocus {
                    member: 3,
                    offset: 5,
                    length: 8,
                    event: 13,
                },
                digest: QuoteTarget::text_digest(text),
            },
            QuoteTarget::Steering {
                key: "dTVLR0qL0".to_owned(),
                digest: QuoteTarget::text_digest(text),
            },
        ];
        for target in targets {
            let markdown = target.markdown();
            assert_eq!(QuoteTarget::from_markdown(&markdown), Ok(target));
        }
    }

    #[test]
    fn parser_refuses_noncanonical_coordinates_and_unknown_shapes() {
        let digest = QuoteTarget::text_digest("quote");
        let lane = uuid(1);
        let source = uuid(2);
        for target in [
            format!("spice://quote/v1/tape/{lane}/{source}/03/5/8/13/{digest}"),
            format!("spice://quote/v2/tape/{lane}/{source}/3/5/8/13/{digest}"),
            format!("spice://other/v1/tape/{lane}/{source}/3/5/8/13/{digest}"),
            format!("spice://quote/v1/steer/not-a-key/{digest}"),
        ] {
            assert!(target.parse::<QuoteTarget>().is_err(), "{target}");
        }
    }
}
