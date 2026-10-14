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
/// Resident-owned reconstruction of the app prose presented for one tape event.
pub type AgentProsePresenter =
    fn(&BusReader, CanonicalUuid, CanonicalUuid, u32, &listen::Prose) -> Result<String, String>;

impl<'a> Resolver<'a> {
    /// Bind quote reads to one admitted tape root and Bus snapshot source.
    #[must_use]
    pub fn new(paths: &'a crate::repository::StatePaths, bus: &'a BusReader) -> Self {
        Self::from_tape(paths.tape(), bus)
    }

    /// Bind quote reads to an already admitted resident tape root.
    #[must_use]
    pub const fn from_tape(tape: &'a Path, bus: &'a BusReader) -> Self {
        Self {
            store: ResidentStore {
                tape,
                bus,
                agent_presenter: None,
            },
        }
    }

    /// Bind quote reads with the resident's authoritative agent-card presentation.
    #[must_use]
    pub const fn from_tape_with_agent_presenter(
        tape: &'a Path,
        bus: &'a BusReader,
        agent_presenter: AgentProsePresenter,
    ) -> Self {
        Self {
            store: ResidentStore {
                tape,
                bus,
                agent_presenter: Some(agent_presenter),
            },
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

    /// Replace every structural quote link with authenticated source text.
    ///
    /// Nested links are followed through the shared depth bound. A refusal is
    /// rendered explicitly in place, so delivery never silently omits source
    /// text and never asks the recipient to resolve authority itself.
    #[must_use]
    pub fn expand(&self, source: &str) -> String {
        expand_with(&self.store, source, 0)
    }
}

/// App presentation of every quote link in `text`, recursively.
///
/// Each resolved node shows its source's referenced media as inline
/// Markdown, exactly as a directive context does, while nested quote
/// discovery reads the raw authored text.
fn present_with(store: &impl QuoteStore, text: &str, depth: u8) -> Vec<QuotePresentation> {
    QuoteMessage::parse(text)
        .quotes()
        .iter()
        .map(|quote| {
            match resolve_source(store, &quote.target, depth).and_then(|source| {
                let (text, packet, result, addressee) = match source.packet {
                    Some(SteeringPacket {
                        kind,
                        result,
                        body,
                        addressee,
                    }) => (body.unwrap_or(source.text), Some(kind), result, addressee),
                    None => (source.text, None, None, None),
                };
                let quotes = present_with(
                    store,
                    &text,
                    depth.checked_add(1).unwrap_or(QUOTE_DEPTH_LIMIT),
                );
                let inlined = wire::media::with_media_payloads(&text, &source.media)
                    .map_err(|_| QuoteUnavailable::Unreadable)?;
                let inlined = match inlined {
                    std::borrow::Cow::Owned(text) => Some(text),
                    std::borrow::Cow::Borrowed(_) => None,
                };
                Ok(QuotePresentation::Resolved {
                    target: quote.target.clone(),
                    text: inlined.unwrap_or(text),
                    author: source.author,
                    disposition: source.disposition,
                    quotes,
                    packet,
                    result,
                    addressee,
                })
            }) {
                Ok(presentation) => presentation,
                Err(reason) => QuotePresentation::Unavailable {
                    target: quote.target.clone(),
                    reason,
                },
            }
        })
        .collect()
}

/// The MSG, ASK, or ANS packet a quoted source or directive was published as.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SteeringPacket {
    /// Protocol kind of the publication.
    pub kind: wire::MessageSendKind,
    /// Result an ANS answers.
    pub result: Option<wire::rooms::QuestionResultRef>,
    /// An operator post's authenticated body, without the header its
    /// recipient's directive spells.
    pub body: Option<String>,
    /// Lane the packet's directive was delivered to; a room post addresses
    /// its room and names none.
    pub addressee: Option<CanonicalUuid>,
}

/// The packet whose room publication opened `row`'s directive.
///
/// The publication's typed operation proves the kind; a peer directive no
/// room publication opened is an MSG. Any other directive is no packet,
/// whatever its words spell.
#[must_use]
pub fn steering_packet(row: &bus::SteeringPresentation) -> Option<SteeringPacket> {
    let addressee = Some(row.head().body().lane);
    let Some(publication) = row.publication() else {
        let created = &row.created().fact;
        let peer = created.kind() == wire::FactKind::SteerMessaged
            || created.kind() == wire::FactKind::SteerQueued;
        return peer.then_some(SteeringPacket {
            kind: wire::MessageSendKind::Msg,
            result: None,
            body: None,
            addressee,
        });
    };
    if let Some(posted) = publication.fact.read::<wire::rooms::RoomPosted>() {
        return Some(room_packet(posted.operation, Some(posted.text), addressee));
    }
    let conversed = publication.fact.read::<wire::rooms::RoomConversed>()?;
    Some(room_packet(conversed.operation, None, addressee))
}

fn room_packet(
    operation: wire::rooms::RoomPublicationKind,
    body: Option<String>,
    addressee: Option<CanonicalUuid>,
) -> SteeringPacket {
    let (kind, result) = match operation {
        wire::rooms::RoomPublicationKind::Msg {} => (wire::MessageSendKind::Msg, None),
        wire::rooms::RoomPublicationKind::Ask { .. } => (wire::MessageSendKind::Ask, None),
        wire::rooms::RoomPublicationKind::Ans { result, .. } => {
            (wire::MessageSendKind::Ans, Some(result))
        }
    };
    SteeringPacket {
        kind,
        result,
        body,
        addressee,
    }
}

/// Bind the resident quote authority into app serialization.
#[must_use]
pub fn presenter(
    tape: PathBuf,
    bus: BusReader,
    agent_presenter: AgentProsePresenter,
) -> serve::stream::QuotePresenter {
    serve::stream::QuotePresenter::new(move |text| {
        Resolver::from_tape_with_agent_presenter(&tape, &bus, agent_presenter).present(text)
    })
}

/// One resolution as a reader receives it: the recorded author, then the
/// exact authenticated text.
///
/// # Errors
///
/// Returns the in-place refusal steering delivery renders when the target
/// names no quotable source.
pub fn render(resolution: &QuoteResolution) -> Result<String, String> {
    match resolution {
        QuoteResolution::Resolved(quote) => {
            let author = match &quote.author {
                QuoteAuthor::Agent { lane, occupant } => {
                    format!("agent occupant {occupant} of lane {lane}")
                }
                QuoteAuthor::Directive { actor } => actor.to_string(),
            };
            Ok(format!("Quoted from {author}:\n\n{}", quote.text))
        }
        QuoteResolution::Unavailable { target, reason } => {
            let mut refusal = String::new();
            render_unavailable(&mut refusal, target, *reason);
            Err(refusal)
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceQuote {
    identity: Option<String>,
    text: String,
    author: QuoteAuthor,
    disposition: Option<wire::Disposition>,
    /// Admitted media the source carries, which its text may reference by
    /// `attachment:` identity.
    media: Vec<wire::Media>,
    /// The packet the source was published as, presented in place of `text`
    /// once `text` authenticates the target.
    packet: Option<SteeringPacket>,
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

    fn room(
        &self,
        room: &wire::TaskStamp,
        publication: &wire::rooms::RoomAuthorityStamp,
    ) -> Result<SourceQuote, SourceError>;
}

struct ResidentStore<'a> {
    tape: &'a Path,
    bus: &'a BusReader,
    agent_presenter: Option<AgentProsePresenter>,
}

impl QuoteStore for ResidentStore<'_> {
    fn tape(
        &self,
        lane: CanonicalUuid,
        source: CanonicalUuid,
        locus: wire::MessageLocus,
    ) -> Result<SourceQuote, SourceError> {
        let record_locus = tape::Locus::new(locus.member, locus.offset, locus.length);
        let session = self
            .bus
            .transcript_session(lane, source, record_locus.member, record_locus.offset)
            .map_err(|_| SourceError::Unreadable)?
            .ok_or(SourceError::Unknown)?;
        let dialect = crate::extension::registry()
            .map_err(|_| SourceError::Unreadable)?
            .vendor(&session.vendor)
            .map(|vendor| vendor.dialect())
            .ok_or(SourceError::Unknown)?;
        let root = self.tape.join(lane.to_string()).join(&session.vendor);
        resolve_tape(
            &root,
            source,
            locus,
            dialect,
            lane,
            session.occupant,
            self.bus,
            self.agent_presenter,
        )
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
        // A peer message names the exact occupant that wrote it, so a later
        // occupant of the sending lane never inherits its authorship.
        let peer = presentation
            .created()
            .fact
            .read::<wire::body::SteerMessaged>()
            .map(|message| (message.from, message.sender))
            .or_else(|| {
                presentation
                    .created()
                    .fact
                    .read::<wire::body::SteerQueued>()
                    .map(|message| (message.from, message.sender))
            });
        let author = peer.map_or_else(
            || QuoteAuthor::Directive {
                actor: presentation.created().actor.clone(),
            },
            |(lane, occupant)| QuoteAuthor::Agent { lane, occupant },
        );
        Ok(SourceQuote {
            identity: None,
            text: head.text.clone(),
            author,
            disposition: Some(head.disposition),
            media: head.attachments.clone(),
            packet: steering_packet(&presentation),
        })
    }

    /// An operator post quotes as its directive-authored text; a lane packet
    /// quotes as the exact transcript hunk its occupant wrote. Coordination,
    /// set-asides, and groomings carry no authored words and quote nothing.
    fn room(
        &self,
        room: &wire::TaskStamp,
        publication: &wire::rooms::RoomAuthorityStamp,
    ) -> Result<SourceQuote, SourceError> {
        let envelope = self
            .bus
            .room_publication(room.clone(), publication.clone())
            .map_err(|_| SourceError::Unreadable)?
            .ok_or(SourceError::Unknown)?;
        let authored = |text, packet| SourceQuote {
            identity: None,
            text,
            author: QuoteAuthor::Directive {
                actor: envelope.actor.clone(),
            },
            disposition: None,
            media: Vec::new(),
            packet,
        };
        if let Some(posted) = envelope.fact.read::<wire::rooms::RoomPosted>() {
            return Ok(authored(
                posted.text,
                Some(room_packet(posted.operation, None, None)),
            ));
        }
        if let Some(published) = envelope.fact.read::<wire::rooms::RoomPublished>()
            && let wire::rooms::RoomPublicationContent::Authored { text } = published.content
        {
            return Ok(authored(text, None));
        }
        let messaged = envelope
            .fact
            .read::<wire::rooms::RoomMessaged>()
            .ok_or(SourceError::Unknown)?;
        let text =
            room_hunk_text(self.tape, &messaged.hunk).map_err(|unavailable| match unavailable {
                wire::rooms::RoomSourceUnavailable::Unreadable {} => SourceError::Unreadable,
                _ => SourceError::Unknown,
            })?;
        Ok(SourceQuote {
            identity: None,
            text,
            author: QuoteAuthor::Agent {
                lane: messaged.hunk.head.lane,
                occupant: messaged.author,
            },
            disposition: None,
            media: Vec::new(),
            packet: None,
        })
    }
}

fn resolve_with(store: &impl QuoteStore, target: &str, depth: u8) -> QuoteResolution {
    match resolve_source(store, target, depth) {
        Ok(source) => QuoteResolution::Resolved(ResolvedQuote {
            text: source.text,
            author: source.author,
            disposition: source.disposition,
        }),
        Err(reason) => unavailable(target, reason),
    }
}

/// The authenticated source one target names, or why it cannot be quoted.
fn resolve_source(
    store: &impl QuoteStore,
    target: &str,
    depth: u8,
) -> Result<SourceQuote, QuoteUnavailable> {
    if depth >= QUOTE_DEPTH_LIMIT {
        return Err(QuoteUnavailable::DepthExceeded);
    }
    let parsed = target
        .parse::<QuoteTarget>()
        .map_err(|_| QuoteUnavailable::Malformed)?;
    let source = match &parsed {
        QuoteTarget::Tape {
            lane,
            source,
            locus,
            ..
        } => store.tape(*lane, *source, *locus),
        QuoteTarget::Steering { key, .. } => {
            if key.parse::<steer::Key>().is_err() {
                return Err(QuoteUnavailable::Malformed);
            }
            store.steering(key)
        }
        QuoteTarget::Room {
            room, publication, ..
        } => store.room(room, publication),
    };
    let resolved = source.map_err(|error| match error {
        SourceError::Unknown => QuoteUnavailable::Unknown,
        SourceError::Unreadable => QuoteUnavailable::Unreadable,
    })?;
    let identity = resolved.identity.as_deref().unwrap_or(&resolved.text);
    if QuoteTarget::text_digest(identity) != *parsed.digest() {
        return Err(QuoteUnavailable::DigestMismatch);
    }
    Ok(resolved)
}

fn expand_with(store: &impl QuoteStore, source: &str, depth: u8) -> String {
    let message = QuoteMessage::parse(source);
    if message.quotes().is_empty() {
        return source.to_owned();
    }
    let mut expanded = String::with_capacity(source.len());
    let mut cursor = 0;
    for quote in message.quotes() {
        expanded.push_str(&source[cursor..quote.range.start]);
        match resolve_with(store, &quote.target, depth) {
            QuoteResolution::Resolved(resolved) => {
                expanded.push_str(&expand_with(store, &resolved.text, depth + 1));
            }
            QuoteResolution::Unavailable { target, reason } => {
                render_unavailable(&mut expanded, &target, reason);
            }
        }
        cursor = quote.range.end;
    }
    expanded.push_str(&source[cursor..]);
    expanded
}

fn render_unavailable(rendered: &mut String, target: &str, reason: QuoteUnavailable) {
    use std::fmt::Write as _;

    let reason = match reason {
        QuoteUnavailable::Malformed => "malformed",
        QuoteUnavailable::Unknown => "unknown",
        QuoteUnavailable::Unreadable => "unreadable",
        QuoteUnavailable::DigestMismatch => "digest-mismatch",
        QuoteUnavailable::DepthExceeded => "depth-exceeded",
    };
    write!(
        rendered,
        "[quote unavailable: reason={reason} source={target:?}]"
    )
    .expect("writing to a String cannot fail");
}

fn unavailable(target: &str, reason: QuoteUnavailable) -> QuoteResolution {
    QuoteResolution::Unavailable {
        target: target.to_owned(),
        reason,
    }
}

/// Why one durable vendor record cannot be decoded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordError {
    /// No tape, source generation, member, or range holds the record.
    Missing,
    /// The record exists but cannot be read or decoded now.
    Unreadable,
}

/// Decode the durable vendor record at `locus` of tape generation `source`
/// under `root`, one lane and vendor's tape directory.
///
/// # Errors
///
/// Returns [`RecordError::Missing`] when the tape now holds another source
/// generation or no record at `locus`, and [`RecordError::Unreadable`] when
/// the bytes cannot be read or decoded.
pub fn decode_record(
    root: &Path,
    source: CanonicalUuid,
    locus: wire::MessageLocus,
    dialect: listen::Dialect,
) -> Result<listen::Decoded, RecordError> {
    let tape = tape::Tape::open_existing(root).map_err(|error| match error {
        tape::Error::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
            RecordError::Missing
        }
        _ => RecordError::Unreadable,
    })?;
    let actual_source = tape.source_id().map_err(|_| RecordError::Unreadable)?;
    if actual_source.as_str() != source.to_string() {
        return Err(RecordError::Missing);
    }
    let record_locus = tape::Locus::new(locus.member, locus.offset, locus.length);
    let bytes = tape.read(record_locus).map_err(|error| {
        if matches!(
            error.refusal(),
            Some(tape::Refusal::UnknownMember { .. } | tape::Refusal::OutOfRange { .. })
        ) {
            RecordError::Missing
        } else {
            RecordError::Unreadable
        }
    })?;
    listen::decode(dialect, &bytes, record_locus).map_err(|_| RecordError::Unreadable)
}

/// The complete assistant words a room packet may cite in `event`: streamed
/// assistant prose, or the intent a tool call declares.
///
/// The packet publisher and the room hunk resolver both select through this,
/// so every published hunk names an event the resolver reads back.
#[must_use]
pub fn packet_text(event: &listen::Event) -> Option<&str> {
    match event {
        listen::Event::StreamedProse(prose) if prose.role == listen::ProseRole::Assistant => {
            Some(prose.text.as_str())
        }
        listen::Event::Tool(listen::Tool {
            moment:
                listen::ToolMoment::Call {
                    intent: Some(intent),
                    ..
                },
            ..
        }) => Some(intent),
        _ => None,
    }
}

/// The exact assistant words one room transcript hunk names, read from its
/// lane's durable tape under `tape`.
///
/// # Errors
///
/// Returns why the tape no longer supplies those bytes.
pub fn room_hunk_text(
    tape: &Path,
    hunk: &wire::rooms::TranscriptHunk,
) -> Result<String, wire::rooms::RoomSourceUnavailable> {
    use wire::rooms::RoomSourceUnavailable;

    let dialect = crate::extension::registry()
        .ok()
        .and_then(|registry| registry.vendor(hunk.head.vendor.as_str()))
        .map(|vendor| vendor.dialect())
        .ok_or(RoomSourceUnavailable::Unreadable {})?;
    let root = tape
        .join(hunk.head.lane.to_string())
        .join(hunk.head.vendor.as_str());
    let decoded =
        decode_record(&root, hunk.head.source, hunk.locus, dialect).map_err(
            |error| match error {
                RecordError::Missing => RoomSourceUnavailable::Missing {},
                RecordError::Unreadable => RoomSourceUnavailable::Unreadable {},
            },
        )?;
    hunk_text(decoded.events(), hunk.locus.event, hunk.text)
}

/// The cited bytes of one decoded record's events, or `changed` when the
/// named event no longer carries packet text spanning that range.
fn hunk_text(
    events: &[listen::Event],
    event: u32,
    range: wire::rooms::TextRange,
) -> Result<String, wire::rooms::RoomSourceUnavailable> {
    let text = usize::try_from(event)
        .ok()
        .and_then(|event| events.get(event))
        .and_then(packet_text);
    let range = usize::try_from(range.start)
        .ok()
        .zip(usize::try_from(range.end).ok());
    text.zip(range)
        .and_then(|(text, (start, end))| text.get(start..end))
        .map(str::to_owned)
        .ok_or(wire::rooms::RoomSourceUnavailable::Changed {})
}

fn resolve_tape(
    root: &Path,
    source: CanonicalUuid,
    locus: wire::MessageLocus,
    dialect: listen::Dialect,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    reader: &BusReader,
    agent_presenter: Option<AgentProsePresenter>,
) -> Result<SourceQuote, SourceError> {
    let decoded = decode_record(root, source, locus, dialect).map_err(|error| match error {
        RecordError::Missing => SourceError::Unknown,
        RecordError::Unreadable => SourceError::Unreadable,
    })?;
    let event = usize::try_from(locus.event)
        .ok()
        .and_then(|event| decoded.events().get(event))
        .ok_or(SourceError::Unknown)?;
    let prose = crate::agent_prose::quoteable(event).ok_or(SourceError::Unknown)?;
    let text = agent_presenter
        .map_or_else(
            || Ok(prose.text.clone()),
            |present| present(reader, lane, source, locus.event, &prose),
        )
        .map_err(|_| SourceError::Unreadable)?;
    Ok(SourceQuote {
        identity: Some(prose.text.clone()),
        text,
        author: QuoteAuthor::Agent { lane, occupant },
        disposition: None,
        media: prose.media.clone(),
        packet: None,
    })
}

#[cfg(test)]
mod tests;
