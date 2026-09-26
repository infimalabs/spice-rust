//! Bounded resolution of authenticated transcript quote targets.

use std::ops::Range;
use std::path::{Path, PathBuf};

use pulldown_cmark::{CowStr, Event as MarkdownEvent, Parser, Tag, TagEnd};

use bus::BusReader;
use wire::{
    CanonicalUuid, QUOTE_DEPTH_LIMIT, QuoteAuthor, QuotePresentation, QuoteResolution, QuoteTarget,
    QuoteUnavailable, ResolvedQuote,
};

/// One fixed-label Markdown quote link and its exact source range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageQuote {
    /// Bytes occupied by the complete link in the original message.
    pub range: Range<usize>,
    /// Destination spelling supplied to [`Resolver::resolve`].
    pub target: String,
}

/// Losslessly parsed message text with its structural quote links.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuoteMessage<'a> {
    source: &'a str,
    quotes: Vec<MessageQuote>,
}

impl<'a> QuoteMessage<'a> {
    /// Recognize ordinary Markdown links whose complete visible label is `quote`.
    ///
    /// Messages without the quote URI authority bypass Markdown parsing and
    /// allocate nothing. Parsing never normalizes the source bytes.
    #[must_use]
    pub fn parse(source: &'a str) -> Self {
        if !source.contains("spice://quote/") {
            return Self {
                source,
                quotes: Vec::new(),
            };
        }
        let mut quotes = Vec::new();
        let mut open: Option<(usize, CowStr<'a>, bool)> = None;
        for (event, range) in Parser::new(source).into_offset_iter() {
            match event {
                MarkdownEvent::Start(Tag::Link { dest_url, .. }) => {
                    open = Some((range.start, dest_url, false));
                }
                MarkdownEvent::Text(label) if open.is_some() => {
                    let Some((_, _, admitted)) = open.as_mut() else {
                        unreachable!("the match guard observed an open link");
                    };
                    if !*admitted && label.as_ref() == "quote" {
                        *admitted = true;
                    } else {
                        open = None;
                    }
                }
                MarkdownEvent::End(TagEnd::Link) => {
                    if let Some((start, target, true)) = open.take()
                        && target.starts_with("spice://quote/")
                    {
                        quotes.push(MessageQuote {
                            range: start..range.end,
                            target: target.to_string(),
                        });
                    }
                }
                _ if open.is_some() => open = None,
                _ => {}
            }
        }
        Self { source, quotes }
    }

    /// Exact original Markdown, unchanged by structural recognition.
    #[must_use]
    pub const fn render(&self) -> &'a str {
        self.source
    }

    /// Quote links in message order.
    #[must_use]
    pub fn quotes(&self) -> &[MessageQuote] {
        &self.quotes
    }
}

/// Resolver over the repository's durable tape and journal authority.
pub struct Resolver<'a> {
    store: ResidentStore<'a>,
}

impl<'a> Resolver<'a> {
    /// Bind quote reads to one admitted tape root and Bus snapshot source.
    #[must_use]
    pub const fn new(tape: &'a Path, bus: &'a BusReader) -> Self {
        Self {
            store: ResidentStore { tape, bus },
        }
    }

    /// Expand exactly one target at the caller's current nesting depth.
    ///
    /// The returned text is never scanned recursively. A caller that finds a
    /// quote link in it invokes this method again with the depth advanced by one.
    #[must_use]
    pub fn resolve(&self, target: &str, depth: u8) -> QuoteResolution {
        resolve_with(&self.store, target, depth)
    }

    /// Resolve every fixed-label quote link into a complete presentation tree.
    #[must_use]
    pub fn present(&self, text: &str) -> Vec<QuotePresentation> {
        present_with(&self.store, text, 0)
    }
}

fn present_with(store: &impl QuoteStore, text: &str, depth: u8) -> Vec<QuotePresentation> {
    QuoteMessage::parse(text)
        .quotes()
        .iter()
        .map(|quote| match resolve_with(store, &quote.target, depth) {
            QuoteResolution::Resolved(resolved) => {
                let quotes = present_with(
                    store,
                    &resolved.text,
                    depth.checked_add(1).unwrap_or(QUOTE_DEPTH_LIMIT),
                );
                QuotePresentation::Resolved {
                    target: quote.target.clone(),
                    text: resolved.text,
                    author: resolved.author,
                    disposition: resolved.disposition,
                    quotes,
                }
            }
            QuoteResolution::Unavailable { target, reason } => {
                QuotePresentation::Unavailable { target, reason }
            }
        })
        .collect()
}

/// Bind the resident quote authority into Canvas serialization.
#[must_use]
pub fn presenter(tape: PathBuf, bus: BusReader) -> canvas::stream::QuotePresenter {
    canvas::stream::QuotePresenter::new(move |text| Resolver::new(&tape, &bus).present(text))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceQuote {
    text: String,
    author: QuoteAuthor,
    disposition: Option<wire::Disposition>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SourceError {
    Unknown,
    Unreadable,
}

trait QuoteStore {
    fn tape(
        &self,
        lane: CanonicalUuid,
        source: CanonicalUuid,
        locus: wire::MessageLocus,
    ) -> Result<SourceQuote, SourceError>;

    fn steering(&self, key: &str) -> Result<SourceQuote, SourceError>;
}

struct ResidentStore<'a> {
    tape: &'a Path,
    bus: &'a BusReader,
}

impl QuoteStore for ResidentStore<'_> {
    fn tape(
        &self,
        lane: CanonicalUuid,
        source: CanonicalUuid,
        locus: wire::MessageLocus,
    ) -> Result<SourceQuote, SourceError> {
        let heads = self
            .bus
            .canvas_heads()
            .map_err(|_| SourceError::Unreadable)?;
        let row = heads
            .residency()
            .and_then(|head| head.body().lanes.iter().find(|row| row.lane == lane))
            .ok_or(SourceError::Unknown)?;
        let occupant = row
            .occupant
            .or_else(|| row.session.as_ref().map(|session| session.occupant))
            .or_else(|| row.lineage.last().copied())
            .ok_or(SourceError::Unknown)?;
        let dialect = crate::extension::registry()
            .map_err(|_| SourceError::Unreadable)?
            .vendor(row.vendor.as_str())
            .map(|vendor| vendor.dialect())
            .ok_or(SourceError::Unknown)?;
        let root = self.tape.join(lane.to_string()).join(row.vendor.as_str());
        resolve_tape(&root, source, locus, dialect, lane, occupant)
    }

    fn steering(&self, key: &str) -> Result<SourceQuote, SourceError> {
        let mut presentations = self
            .bus
            .steering_presentations(vec![key.to_owned()])
            .map_err(|_| SourceError::Unreadable)?;
        let presentation = presentations.pop().ok_or(SourceError::Unknown)?;
        let head = presentation.head().body();
        if head.key != key {
            return Err(SourceError::Unknown);
        }
        Ok(SourceQuote {
            text: head.text.clone(),
            author: QuoteAuthor::Directive {
                actor: presentation.created().actor.clone(),
            },
            disposition: Some(head.disposition),
        })
    }
}

fn resolve_with(store: &impl QuoteStore, target: &str, depth: u8) -> QuoteResolution {
    if depth >= QUOTE_DEPTH_LIMIT {
        return unavailable(target, QuoteUnavailable::DepthExceeded);
    }
    let parsed = match target.parse::<QuoteTarget>() {
        Ok(parsed) => parsed,
        Err(_) => return unavailable(target, QuoteUnavailable::Malformed),
    };
    let source = match &parsed {
        QuoteTarget::Tape {
            lane,
            source,
            locus,
            ..
        } => store.tape(*lane, *source, *locus),
        QuoteTarget::Steering { key, .. } => {
            if key.parse::<steer::Key>().is_err() {
                return unavailable(target, QuoteUnavailable::Malformed);
            }
            store.steering(key)
        }
    };
    let resolved = match source {
        Ok(resolved) => resolved,
        Err(SourceError::Unknown) => return unavailable(target, QuoteUnavailable::Unknown),
        Err(SourceError::Unreadable) => return unavailable(target, QuoteUnavailable::Unreadable),
    };
    if QuoteTarget::text_digest(&resolved.text) != *parsed.digest() {
        return unavailable(target, QuoteUnavailable::DigestMismatch);
    }
    QuoteResolution::Resolved(ResolvedQuote {
        text: resolved.text,
        author: resolved.author,
        disposition: resolved.disposition,
    })
}

fn unavailable(target: &str, reason: QuoteUnavailable) -> QuoteResolution {
    QuoteResolution::Unavailable {
        target: target.to_owned(),
        reason,
    }
}

fn resolve_tape(
    root: &Path,
    source: CanonicalUuid,
    locus: wire::MessageLocus,
    dialect: listen::Dialect,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> Result<SourceQuote, SourceError> {
    let tape = tape::Tape::open_existing(root).map_err(|error| match error {
        tape::Error::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
            SourceError::Unknown
        }
        _ => SourceError::Unreadable,
    })?;
    let actual_source = tape.source_id().map_err(|_| SourceError::Unreadable)?;
    if actual_source.as_str() != source.to_string() {
        return Err(SourceError::Unknown);
    }
    let record_locus = tape::Locus::new(locus.member, locus.offset, locus.length);
    let bytes = tape.read(record_locus).map_err(|error| {
        if matches!(
            error.refusal(),
            Some(tape::Refusal::UnknownMember { .. } | tape::Refusal::OutOfRange { .. })
        ) {
            SourceError::Unknown
        } else {
            SourceError::Unreadable
        }
    })?;
    let decoded =
        listen::decode(dialect, &bytes, record_locus).map_err(|_| SourceError::Unreadable)?;
    let event = usize::try_from(locus.event)
        .ok()
        .and_then(|event| decoded.events().get(event))
        .ok_or(SourceError::Unknown)?;
    let listen::Event::Prose(prose) = event else {
        return Err(SourceError::Unknown);
    };
    if prose.role != listen::ProseRole::Assistant {
        return Err(SourceError::Unknown);
    }
    Ok(SourceQuote {
        text: prose.text.clone(),
        author: QuoteAuthor::Agent { lane, occupant },
        disposition: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct MemoryStore {
        tape: BTreeMap<String, SourceQuote>,
        steering: BTreeMap<String, SourceQuote>,
        unreadable: bool,
    }

    impl QuoteStore for MemoryStore {
        fn tape(
            &self,
            lane: CanonicalUuid,
            source: CanonicalUuid,
            locus: wire::MessageLocus,
        ) -> Result<SourceQuote, SourceError> {
            if self.unreadable {
                return Err(SourceError::Unreadable);
            }
            self.tape
                .get(&format!(
                    "{lane}/{source}/{}/{}/{}/{}",
                    locus.member, locus.offset, locus.length, locus.event
                ))
                .cloned()
                .ok_or(SourceError::Unknown)
        }

        fn steering(&self, key: &str) -> Result<SourceQuote, SourceError> {
            if self.unreadable {
                return Err(SourceError::Unreadable);
            }
            self.steering.get(key).cloned().ok_or(SourceError::Unknown)
        }
    }

    fn uuid(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02x}")
            .parse()
            .expect("canonical UUID")
    }

    fn tape_target(text: &str) -> QuoteTarget {
        QuoteTarget::Tape {
            lane: uuid(1),
            source: uuid(2),
            locus: wire::MessageLocus {
                member: 3,
                offset: 5,
                length: 8,
                event: 13,
            },
            digest: QuoteTarget::text_digest(text),
        }
    }

    fn steering_target(key: &str, text: &str) -> QuoteTarget {
        QuoteTarget::Steering {
            key: key.to_owned(),
            digest: QuoteTarget::text_digest(text),
        }
    }

    #[test]
    fn message_parse_render_preserves_text_without_a_quote_uri_byte_for_byte() {
        let source = "  prose\\r\\n[ordinary](https://example.test/a%20b) and `spice://other/v1`  ";
        let parsed = QuoteMessage::parse(source);

        assert!(parsed.quotes().is_empty());
        assert_eq!(parsed.render().as_bytes(), source.as_bytes());
    }

    #[test]
    fn message_parser_names_only_structural_fixed_label_quote_links() {
        let target = steering_target("dTVLR0qL0", "operator text");
        let link = target.markdown();
        let source = format!("before {link} after `[quote]({target})`");
        let parsed = QuoteMessage::parse(&source);

        assert_eq!(
            parsed.quotes(),
            &[MessageQuote {
                range: 7..7 + link.len(),
                target: target.to_string(),
            }]
        );
        assert_eq!(parsed.render(), source);
    }

    #[test]
    fn both_authority_kinds_resolve_with_recorded_authorship() {
        let mut store = MemoryStore::default();
        let tape = tape_target("agent text");
        store.tape.insert(
            format!("{}/{}/3/5/8/13", uuid(1), uuid(2)),
            SourceQuote {
                text: "agent text".to_owned(),
                author: QuoteAuthor::Agent {
                    lane: uuid(1),
                    occupant: uuid(3),
                },
                disposition: None,
            },
        );
        let steering = steering_target("dTVLR0qL0", "operator text");
        store.steering.insert(
            "dTVLR0qL0".to_owned(),
            SourceQuote {
                text: "operator text".to_owned(),
                author: QuoteAuthor::Directive {
                    actor: wire::Actor::Operator,
                },
                disposition: Some(wire::Disposition::Acked),
            },
        );

        assert_eq!(
            resolve_with(&store, &tape.to_string(), 0),
            QuoteResolution::Resolved(ResolvedQuote {
                text: "agent text".to_owned(),
                author: QuoteAuthor::Agent {
                    lane: uuid(1),
                    occupant: uuid(3),
                },
                disposition: None,
            })
        );
        assert_eq!(
            resolve_with(&store, &steering.to_string(), 0),
            QuoteResolution::Resolved(ResolvedQuote {
                text: "operator text".to_owned(),
                author: QuoteAuthor::Directive {
                    actor: wire::Actor::Operator,
                },
                disposition: Some(wire::Disposition::Acked),
            })
        );
    }

    #[test]
    fn refusals_name_the_original_target_without_synthesized_values() {
        let store = MemoryStore::default();
        let unknown = tape_target("missing").to_string();
        assert_eq!(
            resolve_with(&store, &unknown, 0),
            unavailable(&unknown, QuoteUnavailable::Unknown)
        );

        let unknown_key = steering_target("dTVLR0qL0", "missing").to_string();
        assert_eq!(
            resolve_with(&store, &unknown_key, 0),
            unavailable(&unknown_key, QuoteUnavailable::Unknown)
        );

        let unreadable_store = MemoryStore {
            unreadable: true,
            ..MemoryStore::default()
        };
        assert_eq!(
            resolve_with(&unreadable_store, &unknown, 0),
            unavailable(&unknown, QuoteUnavailable::Unreadable)
        );

        let malformed = unknown.replacen("/3/", "/03/", 1);
        assert_eq!(
            resolve_with(&store, &malformed, 0),
            unavailable(&malformed, QuoteUnavailable::Malformed)
        );

        let mut store = MemoryStore::default();
        let target = steering_target("dTVLR0qL0", "expected");
        store.steering.insert(
            "dTVLR0qL0".to_owned(),
            SourceQuote {
                text: "forged".to_owned(),
                author: QuoteAuthor::Directive {
                    actor: wire::Actor::Operator,
                },
                disposition: Some(wire::Disposition::Live),
            },
        );
        assert_eq!(
            resolve_with(&store, &target.to_string(), 0),
            unavailable(&target.to_string(), QuoteUnavailable::DigestMismatch)
        );
    }

    #[test]
    fn presentation_expands_nested_quotes_and_preserves_the_depth_boundary_target() {
        let mut store = MemoryStore::default();
        let mut returned_text = "terminal text".to_owned();
        let mut targets = Vec::new();
        for index in 0..9 {
            let key = format!("0000000{index}");
            let target = steering_target(&key, &returned_text);
            store.steering.insert(
                key,
                SourceQuote {
                    text: returned_text,
                    author: QuoteAuthor::Directive {
                        actor: wire::Actor::Operator,
                    },
                    disposition: Some(if index == 4 {
                        wire::Disposition::Acked
                    } else {
                        wire::Disposition::Live
                    }),
                },
            );
            returned_text = target.markdown();
            targets.push(target);
        }
        let unknown = steering_target("dTVLR0qL1", "missing");
        let presented = present_with(
            &store,
            &format!(
                "{} and {}",
                targets
                    .last()
                    .expect("chain has an outer target")
                    .markdown(),
                unknown.markdown()
            ),
            0,
        );

        assert_eq!(presented.len(), 2);
        let mut node = &presented[0];
        for depth in 0..usize::from(QUOTE_DEPTH_LIMIT) {
            let QuotePresentation::Resolved {
                target,
                author,
                disposition,
                quotes,
                ..
            } = node
            else {
                panic!("depth {depth} should resolve");
            };
            let source_index = 8 - depth;
            assert_eq!(target, &targets[source_index].to_string());
            assert_eq!(
                author,
                &QuoteAuthor::Directive {
                    actor: wire::Actor::Operator,
                }
            );
            assert_eq!(
                *disposition,
                Some(if source_index == 4 {
                    wire::Disposition::Acked
                } else {
                    wire::Disposition::Live
                })
            );
            assert_eq!(quotes.len(), 1);
            node = &quotes[0];
        }
        assert_eq!(
            node,
            &QuotePresentation::Unavailable {
                target: targets[0].to_string(),
                reason: QuoteUnavailable::DepthExceeded,
            }
        );
        assert_eq!(
            presented[1],
            QuotePresentation::Unavailable {
                target: unknown.to_string(),
                reason: QuoteUnavailable::Unknown,
            }
        );
    }

    #[test]
    fn presentation_reloads_current_directive_disposition() {
        let key = "dTVLR0qL0";
        let text = "operator text";
        let target = steering_target(key, text);
        let mut store = MemoryStore::default();
        store.steering.insert(
            key.to_owned(),
            SourceQuote {
                text: text.to_owned(),
                author: QuoteAuthor::Directive {
                    actor: wire::Actor::Operator,
                },
                disposition: Some(wire::Disposition::Live),
            },
        );
        let message = target.markdown();
        let live = present_with(&store, &message, 0);
        store
            .steering
            .get_mut(key)
            .expect("directive fixture exists")
            .disposition = Some(wire::Disposition::Acked);
        let refreshed = present_with(&store, &message, 0);

        assert!(matches!(
            &live[0],
            QuotePresentation::Resolved {
                disposition: Some(wire::Disposition::Live),
                ..
            }
        ));
        assert!(matches!(
            &refreshed[0],
            QuotePresentation::Resolved {
                disposition: Some(wire::Disposition::Acked),
                ..
            }
        ));
    }
}
